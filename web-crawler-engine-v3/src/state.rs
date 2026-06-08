//! Crawl run state.
//!
//! Owns the frontier, queued/visited tracking, result accumulation, and run
//! limits. This keeps the main `crawl` loop clean and prevents it from becoming
//! a junk drawer of mutable local variables.

use std::collections::HashSet;

use colored_json::Paint;

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

    /// Normalized URLs already scheduled into the frontier.
    ///
    /// This prevents duplicate anchors on the same page, or across several
    /// already-crawled pages, from flooding the frontier before the first copy
    /// is visited.
    queued_fetch_keys: HashSet<NormalizedUrl>,

    /// Normalized URLs already popped and committed for crawl work.
    ///
    /// This prevents repeated browser/cache work for the same normalized URL
    /// inside one crawl run.
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
        let mut queued_fetch_keys = HashSet::new();

        for request in seeds {
            let identity = UrlNormalizer::normalize_for_frontier(&request.requested_url);

            if queued_fetch_keys.insert(identity.normalized) {
                frontier.push(FrontierItem::new(request));
            }
        }

        Self {
            frontier,
            queued_fetch_keys,
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

    /// Returns true if this normalized URL has already been visited / committed
    /// for crawl work in the current run.
    pub fn has_visited_fetch_key(&self, request: &CrawlRequest<P>) -> bool {
        let identity = UrlNormalizer::normalize_for_frontier(&request.requested_url);
        self.visited_fetch_keys.contains(&identity.normalized)
    }

    pub fn mark_visited(&mut self, request: &CrawlRequest<P>) {
        let identity = UrlNormalizer::normalize_for_frontier(&request.requested_url);
        self.visited_fetch_keys.insert(identity.normalized);
    }

    /// Attempts to enqueue a request.
    ///
    /// Returns true when the request was actually added to the frontier.
    /// Returns false when it was already queued, already visited, or the frontier
    /// refused it because of capacity limits.
    pub fn enqueue_request(&mut self, request: CrawlRequest<P>) -> bool {
        let identity = UrlNormalizer::normalize_for_frontier(&request.requested_url);
        let normalized = identity.normalized;

        if self.visited_fetch_keys.contains(&normalized) {
            return false;
        }

        if !self.queued_fetch_keys.insert(normalized.clone()) {
            return false;
        }

        if self.frontier.push(FrontierItem::new(request)) {
            true
        } else {
            // Roll back the queued marker if the frontier was full.
            self.queued_fetch_keys.remove(&normalized);
            false
        }
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

    /// Expands the frontier from anchors discovered on a successfully crawled
    /// page.
    ///
    /// Semantics:
    /// - Only expands if still under `max_hop_depth`.
    /// - Respects `ScopePolicy`.
    /// - Avoids adding URLs already queued or visited in this run.
    /// - Preserves provenance from the parent request.
    /// - Sets `discovered_from` to the parent's requested URL.
    pub fn expand_from_anchors(
        &mut self,
        parent: &CrawlRequest<P>,
        anchors: &[web_browser_driver::ExtractedAnchor],
        policy: &crate::policy::CrawlPolicy,
    ) {
        let next_depth = parent.hop_depth + 1;

        if next_depth > self.limits.max_hop_depth {
            return;
        }

        for anchor in anchors {
            let Some(href) = &anchor.href else {
                continue;
            };

            if !matches!(
                policy.evaluate_scope(&parent.seed_url, href),
                crate::policy::ScopeDecision::InScope
            ) {
                continue;
            }

            let new_request = CrawlRequest {
                id: crate::input::CrawlRequestId::new(),
                seed_group_id: parent.seed_group_id,
                seed_url: parent.seed_url.clone(),
                requested_url: href.clone(),
                discovered_from: Some(parent.requested_url.clone()),
                hop_depth: next_depth,
                profile_key: parent.profile_key.clone(),
                provenance: parent.provenance.clone(),
            };

            if self.enqueue_request(new_request) {
                eprintln!(
                    "{}",
                    format!(
                        "➕ frontier depth={} {} <- {}",
                        next_depth,
                        href.as_str().magenta(),
                        parent.requested_url.as_str().green(),
                    )
                    .cyan()
                );
            }
        }
    }
}
