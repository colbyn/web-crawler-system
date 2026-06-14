//! Crawl frontier.
//!
//! The frontier is the engine's queue of candidate URLs to visit.
//!
//! This implementation is seed-aware round-robin across original seeds, while
//! using priority ordering inside each seed bucket.
//!
//! Why both:
//!
//! - A crawl invocation may contain tens or hundreds of thousands of seeds.
//! - Page budgets are per seed.
//! - One noisy seed may discover thousands of links.
//! - Plain FIFO lets that seed crawl low-value links before better links
//!   discovered later.
//! - Plain global priority lets one seed monopolize the crawl.
//!
//! The desired behavior is closer to:
//!
//! ```text
//! seed A best known page
//! seed B best known page
//! seed C best known page
//! seed A next best known page
//! seed B next best known page
//! seed C next best known page
//! ```
//!
//! rather than:
//!
//! ```text
//! seed A page 0
//! seed A page 1
//! seed A page 2
//! ...
//! seed B eventually
//! ```
//!
//! or:
//!
//! ```text
//! every globally high-scored URL from one noisy seed first
//! everyone else later
//! ```
//!
//! ## Online frontier scoring
//!
//! URLs are uncovered incrementally as page results complete. Cache replay and
//! live browser capture both produce normal opened-page evidence with anchors.
//! The coordinator expands those anchors into follow-up requests, and that is
//! where URL scoring should attach to a `FrontierItem`.
//!
//! This module does not know what a score means. It only knows how to order
//! queued work:
//!
//! ```text
//! higher score first
//! shallower hop depth first
//! earlier discovery sequence first
//! URL string as final stable tiebreaker
//! ```
//!
//! ## Cache boundary
//!
//! Frontier scores are scheduling hints only. They must not participate in cache
//! identity. The same requested URL should map to the same reusable cache entry
//! regardless of scoring profile.

use std::{
    cmp::Ordering,
    collections::{
        BinaryHeap,
        VecDeque,
    },
};

use indexmap::IndexMap;
use serde::{
    Deserialize,
    Serialize,
};
use url::Url;
use uuid::Uuid;

use crate::{
    input::{
        CrawlRequest,
        SeedGroupId,
    },
    url_score::UrlScoreReason,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FrontierItemId(pub Uuid);

impl FrontierItemId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for FrontierItemId {
    fn default() -> Self {
        Self::new()
    }
}

/// Numeric frontier priority.
///
/// Higher values are popped first inside a seed bucket. This remains an integer
/// so heap ordering is stable and cheap. Floating-point profile scores can be
/// converted using `FrontierScore::from_float`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FrontierScore(pub i64);

impl FrontierScore {
    /// Convert a profile score into a frontier score.
    ///
    /// The multiplier keeps one decimal/fractional scoring profile from being
    /// crushed into tiny integer differences while still avoiding floating-point
    /// ordering inside the hot frontier path.
    pub fn from_float(score: f32) -> Self {
        if !score.is_finite() {
            return Self::default();
        }

        let scaled = (score as f64 * 1000.0).round();
        let clamped = scaled.clamp(i64::MIN as f64, i64::MAX as f64);

        Self(clamped as i64)
    }
}

impl Default for FrontierScore {
    fn default() -> Self {
        Self(0)
    }
}

/// Run-local discovery sequence used for deterministic tie-breaking inside a
/// frontier queue.
///
/// This is not durable identity. It only preserves stable queue behavior when
/// two candidate URLs have the same score and hop depth within one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FrontierDiscoverySeq(pub u64);

impl FrontierDiscoverySeq {
    fn next(counter: &mut u64) -> Self {
        let current = *counter;
        *counter = counter.saturating_add(1);
        Self(current)
    }
}

/// Optional score evidence attached to frontier items.
///
/// Normal scheduling only needs `score`. This evidence exists for diagnostics,
/// sink output, or later persistence. Keep it optional so high-throughput crawls
/// do not have to retain verbose explanation data for every queued URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrontierScoreEvidence {
    pub score: FrontierScore,
    pub raw_score: f32,
    pub profile: String,
    pub labels: Vec<String>,
    pub reasons: Vec<UrlScoreReason>,
}

/// Seed-local queue identity used by the frontier.
///
/// This intentionally mirrors the budget identity in `state.rs`, but it is
/// defined here to keep the frontier module self-contained and avoid circular
/// ownership between state and queue.
///
/// `seed_url` alone is usually enough, but `seed_group_id` prevents accidental
/// merging when the same seed URL appears in distinct caller-supplied groups.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrontierSeedKey {
    pub seed_group_id: Option<SeedGroupId>,
    pub seed_url: Url,
}

impl FrontierSeedKey {
    pub fn from_request<P>(request: &CrawlRequest<P>) -> Self {
        Self {
            seed_group_id: request.seed_group_id,
            seed_url: request.seed_url.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrontierItem<P = serde_json::Value> {
    pub id: FrontierItemId,
    pub request: CrawlRequest<P>,

    /// Scheduling priority inside this request's seed bucket.
    ///
    /// This score is a frontier/scheduler hint. It should not affect cache keys,
    /// request identity, policy eligibility, or caller tag association.
    pub score: FrontierScore,

    /// Optional diagnostics explaining how `score` was produced.
    ///
    /// Keep this optional because high-throughput runs may care only about
    /// ordering and not per-URL explanation trails.
    pub score_evidence: Option<FrontierScoreEvidence>,
}

impl<P> FrontierItem<P> {
    pub fn new(request: CrawlRequest<P>) -> Self {
        Self {
            id: FrontierItemId::new(),
            request,
            score: FrontierScore::default(),
            score_evidence: None,
        }
    }

    pub fn with_score(mut self, score: FrontierScore) -> Self {
        self.score = score;
        self
    }

    pub fn with_score_evidence(mut self, evidence: FrontierScoreEvidence) -> Self {
        self.score = evidence.score;
        self.score_evidence = Some(evidence);
        self
    }

    pub fn seed_key(&self) -> FrontierSeedKey {
        FrontierSeedKey::from_request(&self.request)
    }
}

/// Heap wrapper for one frontier item.
///
/// This exists so `BinaryHeap` can order by scheduling metadata without
/// requiring `P: Ord` or comparing full `CrawlRequest<P>` values.
#[derive(Debug, Clone)]
struct PrioritizedFrontierItem<P> {
    score: FrontierScore,
    hop_depth: u32,
    discovery_seq: FrontierDiscoverySeq,
    url: String,
    item: FrontierItem<P>,
}

impl<P> PrioritizedFrontierItem<P> {
    fn new(item: FrontierItem<P>, discovery_seq: FrontierDiscoverySeq) -> Self {
        Self {
            score: item.score,
            hop_depth: item.request.hop_depth,
            discovery_seq,
            url: item.request.requested_url.as_str().to_string(),
            item,
        }
    }
}

impl<P> Ord for PrioritizedFrontierItem<P> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            // Lower hop depth should win in a max-heap.
            .then_with(|| other.hop_depth.cmp(&self.hop_depth))
            // Earlier discovery should win in a max-heap.
            .then_with(|| other.discovery_seq.cmp(&self.discovery_seq))
            // Lexicographically smaller URL should win as final tiebreaker.
            .then_with(|| other.url.cmp(&self.url))
    }
}

impl<P> PartialOrd for PrioritizedFrontierItem<P> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<P> PartialEq for PrioritizedFrontierItem<P> {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
            && self.hop_depth == other.hop_depth
            && self.discovery_seq == other.discovery_seq
            && self.url == other.url
    }
}

impl<P> Eq for PrioritizedFrontierItem<P> {}

/// Priority queue for one seed bucket.
#[derive(Debug, Clone)]
struct SeedPriorityQueue<P> {
    heap: BinaryHeap<PrioritizedFrontierItem<P>>,
}

impl<P> SeedPriorityQueue<P> {
    fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
        }
    }

    fn push(&mut self, item: FrontierItem<P>, discovery_seq: FrontierDiscoverySeq) {
        self.heap
            .push(PrioritizedFrontierItem::new(item, discovery_seq));
    }

    fn pop(&mut self) -> Option<FrontierItem<P>> {
        self.heap.pop().map(|prioritized| prioritized.item)
    }

    fn len(&self) -> usize {
        self.heap.len()
    }

    fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct FrontierQueue<P = serde_json::Value> {
    /// Per-seed priority item buckets.
    by_seed: IndexMap<FrontierSeedKey, SeedPriorityQueue<P>>,

    /// Active seed rotation.
    ///
    /// A seed key appears here when its bucket has at least one item waiting.
    /// The queue remains round-robin across seeds even though each seed bucket
    /// is priority ordered internally.
    active_seeds: VecDeque<FrontierSeedKey>,

    /// Maximum retained frontier items.
    max_items: usize,

    /// Total number of items across all seed buckets.
    len: usize,

    /// Run-local sequence assigned when an item enters the frontier.
    ///
    /// This is used only as a stable tiebreaker inside seed priority queues.
    next_discovery_seq: u64,
}

impl<P> FrontierQueue<P> {
    pub fn new(max_items: usize) -> Self {
        Self {
            by_seed: IndexMap::new(),
            active_seeds: VecDeque::new(),
            max_items,
            len: 0,
            next_discovery_seq: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn seed_count(&self) -> usize {
        self.by_seed.len()
    }

    pub fn active_seed_count(&self) -> usize {
        self.active_seeds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len >= self.max_items
    }

    /// Push an item into the seed-local priority bucket.
    ///
    /// Returns false if the frontier is full.
    ///
    /// If the seed bucket was previously empty or absent, the seed is appended
    /// to the active round-robin rotation.
    pub fn push(&mut self, item: FrontierItem<P>) -> bool {
        if self.is_full() {
            return false;
        }

        let seed_key = item.seed_key();

        let was_active = self
            .by_seed
            .get(&seed_key)
            .is_some_and(|queue| !queue.is_empty());

        let discovery_seq = FrontierDiscoverySeq::next(&mut self.next_discovery_seq);

        let queue = self
            .by_seed
            .entry(seed_key.clone())
            .or_insert_with(SeedPriorityQueue::new);

        queue.push(item, discovery_seq);
        self.len += 1;

        if !was_active {
            self.active_seeds.push_back(seed_key);
        }

        true
    }

    /// Pop the next item using seed-aware round-robin plus seed-local priority.
    ///
    /// Empty/stale seed keys are discarded defensively.
    pub fn pop(&mut self) -> Option<FrontierItem<P>> {
        while let Some(seed_key) = self.active_seeds.pop_front() {
            let Some(queue) = self.by_seed.get_mut(&seed_key) else {
                continue;
            };

            let Some(item) = queue.pop() else {
                self.by_seed.shift_remove(&seed_key);
                continue;
            };

            self.len = self.len.saturating_sub(1);

            if queue.is_empty() {
                self.by_seed.shift_remove(&seed_key);
            } else {
                self.active_seeds.push_back(seed_key);
            }

            return Some(item);
        }

        None
    }

    /// Remove all queued work for a seed.
    ///
    /// Useful when a seed budget is exhausted and the coordinator wants to
    /// aggressively free memory. Current state logic mostly prevents new items
    /// from being enqueued once a seed has no theoretical capacity.
    pub fn clear_seed(&mut self, seed_key: &FrontierSeedKey) -> usize {
        let removed = self
            .by_seed
            .shift_remove(seed_key)
            .map(|queue| queue.len())
            .unwrap_or(0);

        if removed > 0 {
            self.len = self.len.saturating_sub(removed);

            self.active_seeds.retain(|key| key != seed_key);
        }

        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str, seed: &str, hop_depth: u32) -> CrawlRequest {
        let mut request = CrawlRequest::seed(Url::parse(seed).unwrap());
        request.requested_url = Url::parse(url).unwrap();
        request.hop_depth = hop_depth;
        request
    }

    #[test]
    fn pops_higher_score_first_within_one_seed() {
        let seed = "https://example.com/";
        let mut queue = FrontierQueue::new(10);

        queue.push(
            FrontierItem::new(request("https://example.com/privacy", seed, 1))
                .with_score(FrontierScore(-10)),
        );

        queue.push(
            FrontierItem::new(request("https://example.com/careers", seed, 1))
                .with_score(FrontierScore(100)),
        );

        let first = queue.pop().unwrap();
        let second = queue.pop().unwrap();

        assert_eq!(first.request.requested_url.as_str(), "https://example.com/careers");
        assert_eq!(second.request.requested_url.as_str(), "https://example.com/privacy");
    }

    #[test]
    fn preserves_seed_round_robin_across_buckets() {
        let seed_a = "https://a.example/";
        let seed_b = "https://b.example/";

        let mut queue = FrontierQueue::new(10);

        queue.push(
            FrontierItem::new(request("https://a.example/privacy", seed_a, 1))
                .with_score(FrontierScore(-10)),
        );

        queue.push(
            FrontierItem::new(request("https://a.example/careers", seed_a, 1))
                .with_score(FrontierScore(100)),
        );

        queue.push(
            FrontierItem::new(request("https://b.example/about", seed_b, 1))
                .with_score(FrontierScore(0)),
        );

        let first = queue.pop().unwrap();
        let second = queue.pop().unwrap();
        let third = queue.pop().unwrap();

        assert_eq!(first.request.requested_url.as_str(), "https://a.example/careers");
        assert_eq!(second.request.requested_url.as_str(), "https://b.example/about");
        assert_eq!(third.request.requested_url.as_str(), "https://a.example/privacy");
    }

    #[test]
    fn shallower_hop_wins_when_score_ties() {
        let seed = "https://example.com/";
        let mut queue = FrontierQueue::new(10);

        queue.push(
            FrontierItem::new(request("https://example.com/careers/deep", seed, 2))
                .with_score(FrontierScore(100)),
        );

        queue.push(
            FrontierItem::new(request("https://example.com/careers", seed, 1))
                .with_score(FrontierScore(100)),
        );

        let first = queue.pop().unwrap();

        assert_eq!(first.request.requested_url.as_str(), "https://example.com/careers");
    }

    #[test]
    fn earlier_discovery_wins_when_score_and_depth_tie() {
        let seed = "https://example.com/";
        let mut queue = FrontierQueue::new(10);

        queue.push(
            FrontierItem::new(request("https://example.com/jobs-a", seed, 1))
                .with_score(FrontierScore(100)),
        );

        queue.push(
            FrontierItem::new(request("https://example.com/jobs-b", seed, 1))
                .with_score(FrontierScore(100)),
        );

        let first = queue.pop().unwrap();

        assert_eq!(first.request.requested_url.as_str(), "https://example.com/jobs-a");
    }

    #[test]
    fn clear_seed_removes_all_priority_items_for_seed() {
        let seed = Url::parse("https://example.com/").unwrap();
        let seed_key = FrontierSeedKey {
            seed_group_id: None,
            seed_url: seed.clone(),
        };

        let mut queue = FrontierQueue::new(10);

        queue.push(
            FrontierItem::new(request("https://example.com/a", seed.as_str(), 1))
                .with_score(FrontierScore(10)),
        );

        queue.push(
            FrontierItem::new(request("https://example.com/b", seed.as_str(), 1))
                .with_score(FrontierScore(20)),
        );

        assert_eq!(queue.clear_seed(&seed_key), 2);
        assert!(queue.is_empty());
    }
}
