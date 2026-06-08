//! Crawl run state.
//!
//! Owns the frontier, visited tracking, result accumulation, and run limits.
//! This keeps the main `crawl` loop clean and prevents it from becoming
//! a junk drawer of mutable local variables.

use std::collections::HashSet;

use crate::{
    config::CrawlLimits,
    frontier::{FrontierItem, FrontierQueue},
    input::CrawlRequest,
    output::{CrawlPageResult, CrawlRunResult},
    url::{NormalizedUrl, UrlNormalizer},
};

#[derive(Debug)]
pub struct CrawlRunState<P> {
    frontier: FrontierQueue<P>,
    /// Normalized URLs we have already fetched (or decided to fetch) in this run.
    /// Used to avoid redundant browser work for the same document.
    visited_fetch_keys: HashSet<NormalizedUrl>,

    /// Results collected so far.
    pages: Vec<CrawlPageResult<P>>,

    limits: CrawlLimits,
}

impl<P> CrawlRunState<P>
where
    P: Clone + Send + Sync + 'static,
{
    pub fn new(limits: CrawlLimits, seeds: Vec<CrawlRequest<P>>) -> Self {
        let mut frontier = FrontierQueue::new(limits.max_frontier_items);

        for request in seeds {
            frontier.push(FrontierItem::new(request));
        }

        Self {
            frontier,
            visited_fetch_keys: HashSet::new(),
            pages: Vec::new(),
            limits,
        }
    }

    pub fn should_continue(&self) -> bool {
        self.pages.len() < self.limits.max_pages && !self.frontier.is_empty()
    }

    pub fn pop_next(&mut self) -> Option<FrontierItem<P>> {
        self.frontier.pop()
    }

    /// Returns true if we have already decided to visit (or visited) this
    /// normalized URL + profile combination in the current run.
    pub fn has_seen_fetch_key(&self, request: &CrawlRequest<P>) -> bool {
        // For v1 we normalize only the URL. Later we can incorporate
        // profile_key / cache namespace into the fetch key if needed.
        let identity = UrlNormalizer::normalize_for_frontier(&request.requested_url);
        self.visited_fetch_keys.contains(&identity.normalized)
    }

    pub fn mark_visited(&mut self, request: &CrawlRequest<P>) {
        let identity = UrlNormalizer::normalize_for_frontier(&request.requested_url);
        self.visited_fetch_keys.insert(identity.normalized);
    }

    pub fn record_page(&mut self, result: CrawlPageResult<P>) {
        self.pages.push(result);
    }

    pub fn pages_len(&self) -> usize {
        self.pages.len()
    }

    pub fn finish(self) -> CrawlRunResult<P> {
        CrawlRunResult { pages: self.pages }
    }

    /// Simple frontier expansion hook. Real implementation will live in engine
    /// or a policy helper later.
    pub fn expand_from_anchors(
        &mut self,
        _parent_request: &CrawlRequest<P>,
        _anchors: &[web_browser_driver::ExtractedAnchor],
    ) {
        // TODO in next iteration: create child CrawlRequest<P>, respect scope + depth,
        // and push to frontier while updating enqueued set.
    }
}
