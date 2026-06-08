//! Crawl frontier.
//!
//! The frontier is the engine's queue of candidate URLs to visit.
//!
//! This implementation is intentionally deterministic FIFO for now.
//! Priority scheduling can return later once scoring is meaningful.
//!
//! Deterministic FIFO matters because cache-warm crawls should tend to replay
//! the same crawl order as cold crawls when inputs and extracted anchors are the
//! same.

use std::collections::VecDeque;

use serde::{
    Deserialize,
    Serialize,
};
use uuid::Uuid;

use crate::input::CrawlRequest;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FrontierItemId(pub Uuid);

impl FrontierItemId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrontierItem<P = serde_json::Value> {
    pub id: FrontierItemId,
    pub request: CrawlRequest<P>,

    /// Reserved for future priority scheduling.
    ///
    /// Current frontier behavior is FIFO, so this field is carried but not used.
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
}

#[derive(Debug, Clone)]
pub struct FrontierQueue<P = serde_json::Value> {
    queue: VecDeque<FrontierItem<P>>,
    max_items: usize,
}

impl<P> FrontierQueue<P> {
    pub fn new(max_items: usize) -> Self {
        Self {
            queue: VecDeque::new(),
            max_items,
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn push(&mut self, item: FrontierItem<P>) -> bool {
        if self.queue.len() >= self.max_items {
            return false;
        }

        self.queue.push_back(item);
        true
    }

    pub fn pop(&mut self) -> Option<FrontierItem<P>> {
        self.queue.pop_front()
    }
}
