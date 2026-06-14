//! Crawl run state.
//!
//! Owns the frontier, visited tracking, result accumulation, per-seed budgets,
//! and frontier expansion.
//!
//! This keeps the main crawl loop focused on orchestration rather than
//! bookkeeping.
//!
//! # Per-seed crawl budgets
//!
//! The primary page budget is per original seed, not global.
//!
//! A caller may pipe 100,000 seeds into the engine. Each seed should receive its
//! own crawl budget:
//!
//! ```text
//! max_pages_per_seed = 5
//! seed A may open up to 5 pages
//! seed B may open up to 5 pages
//! seed C may open up to 5 pages
//! ...
//! ```
//!
//! `max_total_pages` remains only as an optional global emergency brake.
//!
//! # Reservation model
//!
//! The state tracks both:
//!
//! - opened page count per seed,
//! - in-flight reserved page slots per seed.
//!
//! The in-flight lane matters because the coordinator dispatches concurrent page
//! jobs. Without reservations, the coordinator could accidentally launch more
//! work for a seed than its budget allows before completed results come back.
//!
//! # Counting semantics
//!
//! Only `Opened` page results consume seed page budget.
//!
//! This includes cache hits that replay usable page evidence, because they still
//! produce page facts and may expand the frontier.
//!
//! Skipped and failed results are recorded, but they do not consume page budget.
//!
//! # Run-local fetch dedupe
//!
//! The reusable artifact cache may dedupe artifacts by URL-level cache identity,
//! but the crawl run must not erase caller associations before they reach the
//! cache.
//!
//! Therefore queued/visited fetch keys are not merely normalized URLs. They also
//! include seed context and inherited request tags. This allows two callers,
//! entities, categories, or runs to point at the same URL and still flow through
//! the cache path so their tags can be merged onto the shared artifact.
//!
//! Within one seed/tag context, normalized URL dedupe still prevents loops and
//! repeated frontier spam.
//!
//! # Online frontier scoring
//!
//! Frontier scoring belongs here because this module is where extracted anchors
//! become follow-up crawl requests.
//!
//! Scoring is deliberately a scheduling hint only:
//!
//! - scope policy still decides whether a discovered URL is internal enough,
//! - visit policy still decides whether a queued request may be opened,
//! - cache identity remains based on requested URL,
//! - low score never means "exclude forever."
//!
//! As pages complete, their anchors are expanded. Each in-scope discovered URL
//! can be scored before entering the frontier. The frontier remains seed-fair
//! while using scores to prioritize work inside each seed bucket.

use std::collections::{
    HashMap,
    HashSet,
};

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;
use web_browser_driver::ExtractedAnchor;
use web_crawler_db::CacheTag;

use crate::{
    config::CrawlLimits,
    frontier::{
        FrontierItem,
        FrontierQueue,
        FrontierScore,
        FrontierScoreEvidence,
    },
    input::{
        CrawlRequest,
        SeedGroupId,
    },
    output::{
        CrawlPageOutcome,
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
    url_score::FrontierUrlScorer,
};

/// Seed-local crawl budget identity.
///
/// `seed_url` alone is often enough, but `seed_group_id` prevents accidental
/// budget merging when the same seed URL appears in distinct caller-supplied
/// groups.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SeedBudgetKey {
    pub seed_group_id: Option<SeedGroupId>,
    pub seed_url: Url,
}

impl SeedBudgetKey {
    pub fn from_request<P>(request: &CrawlRequest<P>) -> Self {
        Self {
            seed_group_id: request.seed_group_id,
            seed_url: request.seed_url.clone(),
        }
    }
}

/// Run-local fetch identity.
///
/// This is intentionally broader than a normalized URL and narrower than a
/// request ID.
///
/// The cache artifact may be shared by URL, but the crawl path must preserve
/// caller associations. If two seed requests point at the same URL with
/// different tags, both should be allowed to reach the cache path so both tag
/// sets can be merged onto the shared artifact.
///
/// Within the same seed/tag context, normalized URL dedupe still prevents loops,
/// repeated anchor spam, and duplicate queued work.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FetchVisitKey {
    seed_group_id: Option<SeedGroupId>,
    seed_url: Url,
    tags: Vec<CacheTag>,
    normalized_url: NormalizedUrl,
}

impl FetchVisitKey {
    fn from_request<P>(request: &CrawlRequest<P>) -> Self {
        let identity = UrlNormalizer::normalize_for_frontier(&request.requested_url);

        Self {
            seed_group_id: request.seed_group_id,
            seed_url: request.seed_url.clone(),
            tags: request.tags.clone(),
            normalized_url: identity.normalized,
        }
    }
}

#[derive(Debug)]
pub struct CrawlRunState<P> {
    /// Candidate requests waiting to be visited.
    frontier: FrontierQueue<P>,

    /// Seed/tag-scoped fetch keys currently queued.
    ///
    /// This prevents repeated enqueueing before an item is popped and marked
    /// visited, while still allowing distinct caller associations to reach the
    /// shared cache artifact.
    queued_fetch_keys: HashSet<FetchVisitKey>,

    /// Seed/tag-scoped fetch keys already committed for crawl work in this run.
    visited_fetch_keys: HashSet<FetchVisitKey>,

    /// Number of opened page results per seed.
    opened_pages_by_seed: HashMap<SeedBudgetKey, usize>,

    /// Number of reserved/in-flight page slots per seed.
    inflight_pages_by_seed: HashMap<SeedBudgetKey, usize>,

    /// Total opened page results for the whole invocation.
    ///
    /// This is only used for the optional global emergency brake.
    total_opened_pages: usize,

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
            let fetch_key = FetchVisitKey::from_request(&request);

            if queued_fetch_keys.insert(fetch_key) {
                frontier.push(FrontierItem::new(request));
            }
        }

        Self {
            frontier,
            queued_fetch_keys,
            visited_fetch_keys: HashSet::new(),
            opened_pages_by_seed: HashMap::new(),
            inflight_pages_by_seed: HashMap::new(),
            total_opened_pages: 0,
            pages: Vec::new(),
            limits,
        }
    }

    /// Returns true while there is frontier work and the optional global
    /// emergency brake has not fired.
    ///
    /// Per-seed budgets are checked when reserving work.
    pub fn should_continue(&self) -> bool {
        !self.global_page_budget_exhausted() && !self.frontier.is_empty()
    }

    pub fn pop_next(&mut self) -> Option<FrontierItem<P>> {
        self.frontier.pop()
    }

    pub fn frontier_len(&self) -> usize {
        self.frontier.len()
    }

    pub fn pages_len(&self) -> usize {
        self.pages.len()
    }

    pub fn total_opened_pages(&self) -> usize {
        self.total_opened_pages
    }

    pub fn global_page_budget_exhausted(&self) -> bool {
        self.limits
            .max_total_pages
            .is_some_and(|max| self.total_opened_pages >= max)
    }

    pub fn seed_budget_key(request: &CrawlRequest<P>) -> SeedBudgetKey {
        SeedBudgetKey::from_request(request)
    }

    pub fn opened_pages_for_seed(&self, request: &CrawlRequest<P>) -> usize {
        let key = Self::seed_budget_key(request);

        self.opened_pages_by_seed
            .get(&key)
            .copied()
            .unwrap_or(0)
    }

    pub fn inflight_pages_for_seed(&self, request: &CrawlRequest<P>) -> usize {
        let key = Self::seed_budget_key(request);

        self.inflight_pages_by_seed
            .get(&key)
            .copied()
            .unwrap_or(0)
    }

    /// Return true if this request's seed still has page budget available.
    ///
    /// This checks opened + in-flight slots. That makes it safe for concurrent
    /// dispatch.
    pub fn can_reserve_seed_slot(&self, request: &CrawlRequest<P>) -> bool {
        if self.global_page_budget_exhausted() {
            return false;
        }

        let opened = self.opened_pages_for_seed(request);
        let inflight = self.inflight_pages_for_seed(request);

        opened + inflight < self.limits.max_pages_per_seed
    }

    /// Reserve one in-flight page slot for this request's seed.
    ///
    /// Returns false if the seed or global budget is exhausted.
    pub fn reserve_seed_slot(&mut self, request: &CrawlRequest<P>) -> bool {
        if !self.can_reserve_seed_slot(request) {
            return false;
        }

        let key = Self::seed_budget_key(request);

        *self.inflight_pages_by_seed.entry(key).or_insert(0) += 1;

        true
    }

    /// Release one reserved/in-flight page slot for this request's seed.
    ///
    /// Call this when a reserved job completes, regardless of whether it opened,
    /// failed, or skipped.
    pub fn release_seed_slot(&mut self, request: &CrawlRequest<P>) {
        let key = Self::seed_budget_key(request);

        let should_remove = match self.inflight_pages_by_seed.get_mut(&key) {
            Some(count) => {
                *count = count.saturating_sub(1);
                *count == 0
            }

            None => false,
        };

        if should_remove {
            self.inflight_pages_by_seed.remove(&key);
        }
    }

    fn commit_opened_page_for_seed(&mut self, request: &CrawlRequest<P>) {
        let key = Self::seed_budget_key(request);

        *self.opened_pages_by_seed.entry(key).or_insert(0) += 1;
        self.total_opened_pages += 1;
    }

    /// Return true if this request has already been visited/committed for crawl
    /// work in the current run.
    ///
    /// The key is seed/tag-scoped, so the same normalized URL may still be
    /// processed for another caller association. That lets warm-cache requests
    /// merge their tags onto the shared artifact.
    pub fn has_visited_fetch_key(&self, request: &CrawlRequest<P>) -> bool {
        let fetch_key = FetchVisitKey::from_request(request);

        self.visited_fetch_keys.contains(&fetch_key)
    }

    pub fn mark_visited(&mut self, request: &CrawlRequest<P>) {
        let fetch_key = FetchVisitKey::from_request(request);

        self.visited_fetch_keys.insert(fetch_key);
    }

    /// Attempt to enqueue a request with a neutral frontier score.
    ///
    /// Returns true when the request was actually added to the frontier.
    pub fn enqueue_request(&mut self, request: CrawlRequest<P>) -> bool {
        self.enqueue_item(FrontierItem::new(request))
    }

    /// Attempt to enqueue a fully constructed frontier item.
    ///
    /// This is the lower-level insertion point used by scored frontier
    /// expansion. It preserves all existing budget and dedupe semantics while
    /// allowing the caller to attach priority and optional score evidence.
    ///
    /// Returns true when the item was actually added to the frontier.
    pub fn enqueue_item(&mut self, item: FrontierItem<P>) -> bool {
        if self.global_page_budget_exhausted() {
            return false;
        }

        // Early budget backpressure. The final gate is still reservation time.
        if !self.can_reserve_seed_slot(&item.request) {
            return false;
        }

        let fetch_key = FetchVisitKey::from_request(&item.request);

        if self.visited_fetch_keys.contains(&fetch_key) {
            return false;
        }

        if !self.queued_fetch_keys.insert(fetch_key.clone()) {
            return false;
        }

        if self.frontier.push(item) {
            true
        } else {
            self.queued_fetch_keys.remove(&fetch_key);
            false
        }
    }

    /// Record a completed page result.
    ///
    /// Only `Opened` results consume page budgets.
    pub fn record_page(&mut self, result: CrawlPageResult<P>) {
        if matches!(result.outcome, CrawlPageOutcome::Opened { .. }) {
            self.commit_opened_page_for_seed(&result.request);
        }

        self.pages.push(result);
    }

    /// Complete a previously reserved page job and record its result.
    ///
    /// This is the method the concurrent coordinator should use.
    pub fn complete_reserved_page(&mut self, result: CrawlPageResult<P>) {
        self.release_seed_slot(&result.request);
        self.record_page(result);
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
    /// - preserves seed/request context via `CrawlRequest::discovered_from`,
    /// - optionally scores each discovered URL before enqueue,
    /// - avoids adding work for seeds whose page budget is already consumed.
    ///
    /// Scoring is online: each in-scope internal URL is scored as it is
    /// uncovered. The frontier then decides scheduling order inside the seed
    /// bucket. Low-scoring URLs are not filtered here.
    pub fn expand_from_anchors(
        &mut self,
        parent: &CrawlRequest<P>,
        anchors: &[ExtractedAnchor],
        policy: &CrawlPolicy,
        scorer: Option<&FrontierUrlScorer>,
        retain_score_evidence: bool,
    ) {
        let next_depth = parent.hop_depth + 1;

        if next_depth > self.limits.max_hop_depth {
            return;
        }

        if self.global_page_budget_exhausted() {
            return;
        }

        if !self.can_reserve_seed_slot(parent) {
            return;
        }

        let mut considered = 0usize;
        let mut in_scope = 0usize;
        let mut enqueued = 0usize;

        for anchor in anchors {
            considered += 1;

            let Some(href) = &anchor.href else {
                continue;
            };

            if !matches!(
                policy.evaluate_scope(&parent.seed_url, href),
                ScopeDecision::InScope
            ) {
                continue;
            }

            in_scope += 1;

            let new_request = CrawlRequest::discovered_from(parent, href.clone());

            let item = build_frontier_item(
                new_request,
                href,
                scorer,
                retain_score_evidence,
            );

            if self.enqueue_item(item) {
                enqueued += 1;
            }

            // Avoid flooding the frontier for a seed that no longer has
            // theoretical capacity.
            if !self.can_reserve_seed_slot(parent) {
                break;
            }

            if self.global_page_budget_exhausted() {
                break;
            }
        }

        tracing::debug!(
            parent_url = %parent.requested_url,
            next_depth,
            considered,
            in_scope,
            enqueued,
            frontier_len = self.frontier_len(),
            "expanded anchors into frontier"
        );
    }
}

fn build_frontier_item<P>(
    request: CrawlRequest<P>,
    href: &Url,
    scorer: Option<&FrontierUrlScorer>,
    retain_score_evidence: bool,
) -> FrontierItem<P> {
    let Some(scorer) = scorer else {
        return FrontierItem::new(request);
    };

    let scored = scorer.score_url(href);
    let score = FrontierScore::from_float(scored.score);

    if retain_score_evidence {
        let evidence = FrontierScoreEvidence {
            score,
            raw_score: scored.score,
            profile: scored.profile,
            labels: scored.labels.into_iter().collect(),
            reasons: scored.reasons,
        };

        FrontierItem::new(request).with_score_evidence(evidence)
    } else {
        FrontierItem::new(request).with_score(score)
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::url_score::{
//         BuiltinUrlScoringProfile,
//         FrontierUrlScorer,
//     };

//     fn request(url: &str) -> CrawlRequest {
//         CrawlRequest::seed(Url::parse(url).unwrap())
//     }

//     fn anchor(href: &str) -> ExtractedAnchor {
//         ExtractedAnchor {
//             index: 0,
//             raw_href: Some(href.to_string()),
//             resolved_href: Some(Url::parse(href).unwrap()),
//             text: None,
//             label: None,
//             attributes: Default::default(),
//             position: None,
//         }
//     }

//     #[test]
//     fn scored_anchor_enters_frontier_before_lower_scored_anchor_for_same_seed() {
//         let limits = CrawlLimits {
//             max_pages_per_seed: 3,
//             max_hop_depth: 1,
//             max_frontier_items: 100,
//             max_total_pages: None,
//         };

//         let seed = request("https://example.com/");
//         let mut state = CrawlRunState::new(limits, vec![seed.clone()]);
//         let _ = state.pop_next();

//         let opened = CrawlPageResult {
//             request: seed.clone(),
//             cache_key: None,
//             cache_decision: None,
//             outcome: CrawlPageOutcome::Opened {
//                 resolution: web_browser_driver::UrlResolution {
//                     requested_url: Url::parse("https://example.com/").unwrap(),
//                     final_url: Url::parse("https://example.com/").unwrap(),
//                     redirects: Vec::new(),
//                 },
//                 status_code: Some(200),
//                 telemetry: Default::default(),
//                 non_critical_errors: Vec::new(),
//                 page_info: None,
//                 anchors: Vec::new(),
//                 snapshot: crate::output::SnapshotDecision::NotRequested,
//             },
//         };

//         state.record_page(opened);

//         let scorer = FrontierUrlScorer::builtin(BuiltinUrlScoringProfile::Careers);

//         state.expand_from_anchors(
//             &seed,
//             &[
//                 anchor("https://example.com/privacy"),
//                 anchor("https://example.com/careers"),
//             ],
//             &CrawlPolicy::default(),
//             Some(&scorer),
//             false,
//         );

//         let next = state.pop_next().unwrap();

//         assert_eq!(
//             next.request.requested_url.as_str(),
//             "https://example.com/careers"
//         );
//     }
// }
