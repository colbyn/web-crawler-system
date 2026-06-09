//! Crawl frontier.
//!
//! The frontier is the engine's queue of candidate URLs to visit.
//!
//! This implementation is seed-aware round-robin rather than plain FIFO.
//!
//! Why:
//!
//! - A crawl invocation may contain tens or hundreds of thousands of seeds.
//! - Page budgets are per seed.
//! - One noisy seed may discover thousands of links.
//! - Plain FIFO lets that seed monopolize the crawl order.
//!
//! The desired behavior is closer to:
//!
//! ```text
//! seed A page 0
//! seed B page 0
//! seed C page 0
//! seed A page 1
//! seed B page 1
//! seed C page 1
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
//! The queue remains deterministic. Within each seed bucket, items are FIFO.
//! Across seed buckets, buckets are rotated round-robin.
//!
//! `FrontierScore` is still carried as a future hook. Scoring can later become
//! per-seed priority selection without changing the crawl request/result model.

use std::collections::VecDeque;

use indexmap::IndexMap;
use serde::{
    Deserialize,
    Serialize,
};
use url::Url;
use uuid::Uuid;

use crate::input::{
    CrawlRequest,
    SeedGroupId,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FrontierScore(pub i64);

impl Default for FrontierScore {
    fn default() -> Self {
        Self(0)
    }
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

    /// Reserved for future priority scheduling.
    ///
    /// Current behavior is seed-aware round-robin. This score is retained so a
    /// later frontier can order items within each seed bucket, or pick between
    /// seed buckets by aggregate priority.
    pub score: FrontierScore,
}

impl<P> FrontierItem<P> {
    pub fn new(request: CrawlRequest<P>) -> Self {
        Self {
            id: FrontierItemId::new(),
            request,
            score: FrontierScore::default(),
        }
    }

    pub fn with_score(mut self, score: FrontierScore) -> Self {
        self.score = score;
        self
    }

    pub fn seed_key(&self) -> FrontierSeedKey {
        FrontierSeedKey::from_request(&self.request)
    }
}

#[derive(Debug, Clone)]
pub struct FrontierQueue<P = serde_json::Value> {
    /// Per-seed FIFO item buckets.
    by_seed: IndexMap<FrontierSeedKey, VecDeque<FrontierItem<P>>>,

    /// Active seed rotation.
    ///
    /// A seed key appears here when its bucket has at least one item waiting.
    active_seeds: VecDeque<FrontierSeedKey>,

    /// Maximum retained frontier items.
    max_items: usize,

    /// Total number of items across all seed buckets.
    len: usize,
}

impl<P> FrontierQueue<P> {
    pub fn new(max_items: usize) -> Self {
        Self {
            by_seed: IndexMap::new(),
            active_seeds: VecDeque::new(),
            max_items,
            len: 0,
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

    /// Push an item into the seed-local FIFO bucket.
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

        let queue = self
            .by_seed
            .entry(seed_key.clone())
            .or_insert_with(VecDeque::new);

        queue.push_back(item);
        self.len += 1;

        if !was_active {
            self.active_seeds.push_back(seed_key);
        }

        true
    }

    /// Pop the next item using seed-aware round-robin.
    ///
    /// Empty/stale seed keys are discarded defensively.
    pub fn pop(&mut self) -> Option<FrontierItem<P>> {
        while let Some(seed_key) = self.active_seeds.pop_front() {
            let Some(queue) = self.by_seed.get_mut(&seed_key) else {
                continue;
            };

            let Some(item) = queue.pop_front() else {
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
    /// Useful later when a seed budget is exhausted and the coordinator wants to
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
