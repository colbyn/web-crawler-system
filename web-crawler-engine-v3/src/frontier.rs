//! Crawl frontier.
//!
//! The frontier is the engine's queue of candidate URLs to visit.
//!
//! It must preserve provenance. A discovered URL is not just a string waiting
//! in line. It carries seed lineage, hop depth, source URL, profile assignment,
//! and caller-owned metadata.
//!
//! This module should remain focused on queue mechanics and item identity. It
//! should not open browsers, inspect pages, evaluate cache health, or decide
//! application-level meaning.
//!
//! Priority scoring is intentionally primitive at first. The engine can later
//! add URL heuristics such as favoring about/contact/careers pages or penalizing
//! assets, tracking params, and infinite calendar traps.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

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

impl<P> PartialEq for FrontierItem<P> {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.id == other.id
    }
}

impl<P> Eq for FrontierItem<P> {}

impl<P> PartialOrd for FrontierItem<P> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<P> Ord for FrontierItem<P> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| self.id.0.cmp(&other.id.0))
    }
}

#[derive(Debug, Clone)]
pub struct FrontierQueue<P = serde_json::Value> {
    heap: BinaryHeap<FrontierItem<P>>,
    max_items: usize,
}

impl<P> FrontierQueue<P> {
    pub fn new(max_items: usize) -> Self {
        Self {
            heap: BinaryHeap::new(),
            max_items,
        }
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn push(&mut self, item: FrontierItem<P>) -> bool {
        if self.heap.len() >= self.max_items {
            return false;
        }

        self.heap.push(item);
        true
    }

    pub fn pop(&mut self) -> Option<FrontierItem<P>> {
        self.heap.pop()
    }
}
