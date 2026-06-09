//! Crawl run state.
//!
//! This module owns the mutable state for a single crawl invocation.
//!
//! It keeps the engine loop small by managing:
//!
//! - the frontier,
//! - queued URL tracking,
//! - visited URL tracking,
//! - page result accumulation,
//! - crawl limits,
//! - expansion from discovered anchors.
//!
//! ## URL dedupe
//!
//! The state tracks normalized frontier/fetch keys so the same crawl run does
//! not repeatedly visit equivalent URLs.
//!
//! This is intentionally run-local. Durable cross-run reuse belongs to the
//! SQLite cache.
//!
//! ## Tag inheritance
//!
//! Seed/request tags must flow into discovered pages. This is what lets callers
//! later query:
//!
//! ```text
//! all cached pages reached for entity X
//! all cached pages reached for category Y
//! all cached pages reached during debug run Z
//! ```
//!
//! This module therefore does not manually rebuild child requests field by
//! field. It uses [`CrawlRequest::discovered_from`] so inheritance is centralized
//! in the input model.
//!
//! The key invariant is:
//!
//! ```text
//! parent request tags == child request inherited tags
//! ```
//!
//! unless a future policy explicitly adds or removes tags.

use std::collections::HashSet;

use colored_json::Paint;
use web_browser_driver::ExtractedAnchor;

use crate::{
    config::CrawlLimits,
    frontier::{
        FrontierItem,
        FrontierQueue,
    },
    input::CrawlRequest,
    output::{
        CrawlPageResult,
        CrawlRunResult,
    },
    policy::{
        CrawlPolicy,
        ScopeDecision,
    },
    url::{
        NormalizedUrl,
        UrlNormalizer,
    },
};

#[derive(Debug)]
pub struct CrawlRunState<P> {
    /// Candidate requests waiting to be visited.
    frontier: FrontierQueue<P>,

    /// Normalized URLs currently queued.
    ///
    /// This prevents the same normalized URL from being enqueued many times
    /// before it is popped and visited.
    queued_fetch_keys: HashSet<NormalizedUrl>,

    /// Normalized URLs already popped and committed for crawl work.
    ///
    /// This prevents repeated browser/cache work for the same normalized URL
    /// inside one crawl run.
    visited_fetch_keys: HashSet<NormalizedUrl>,

    /// Results collected so far.
    pages: Vec<CrawlPageResult<P>>,

    /// Run-local crawl limits.
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

    /// Return true if this normalized URL has already been visited/committed
    /// for crawl work in the current run.
    pub fn has_visited_fetch_key(&self, request: &CrawlRequest<P>) -> bool {
        let identity = UrlNormalizer::normalize_for_frontier(&request.requested_url);
        self.visited_fetch_keys.contains(&identity.normalized)
    }

    pub fn mark_visited(&mut self, request: &CrawlRequest<P>) {
        let identity = UrlNormalizer::normalize_for_frontier(&request.requested_url);
        self.visited_fetch_keys.insert(identity.normalized);
    }

    /// Attempt to enqueue a request.
    ///
    /// Returns true when the request was actually added to the frontier.
    /// Returns false when it was already queued, already visited, or rejected
    /// because the frontier is full.
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
            // Roll back the queued marker if the frontier refused the item.
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

    /// Expand the frontier from anchors discovered on a successfully crawled
    /// page.
    ///
    /// Semantics:
    ///
    /// - only expands if still under `max_hop_depth`,
    /// - respects scope policy,
    /// - avoids URLs already queued or visited in this run,
    /// - preserves seed/request context through `CrawlRequest::discovered_from`,
    /// - sets `discovered_from` to the parent's requested URL,
    /// - increments hop depth by one.
    pub fn expand_from_anchors(
        &mut self,
        parent: &CrawlRequest<P>,
        anchors: &[ExtractedAnchor],
        policy: &CrawlPolicy,
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
                ScopeDecision::InScope
            ) {
                continue;
            }

            let new_request = CrawlRequest::discovered_from(parent, href.clone());

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

