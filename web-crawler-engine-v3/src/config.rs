//! Engine-level crawl configuration.
//!
//! This module defines crawler limits, concurrency knobs, and general engine
//! behavior. It should not contain browser launch configuration,
//! app-specific business metadata, or persistence details.
//!
//! Browser launch settings belong to `web-browser-driver`.
//! App-specific settings belong above this crate.
//! Durable storage configuration belongs either in cache/store implementations
//! or downstream applications.
//!
//! # Budget model
//!
//! The primary page budget is per seed, not per crawl invocation.
//!
//! This matters for batch crawls. A caller may pipe 100,000 seeds into the
//! engine and expect each seed to receive a small crawl budget. A single global
//! `max_pages` would let the first few seeds consume the entire run.
//!
//! # Concurrency model
//!
//! Concurrency is controlled separately from crawl budgets.
//!
//! Page budgets answer:
//!
//! ```text
//! how much evidence may this seed collect?
//! ```
//!
//! Concurrency answers:
//!
//! ```text
//! how many page jobs may run at the same time?
//! ```

use std::time::Duration;

use serde::{
    Deserialize,
    Serialize,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlEngineConfig {
    pub limits: CrawlLimits,
    pub concurrency: CrawlConcurrency,

    /// Default timeout applied by the crawler around a single page open.
    ///
    /// The browser driver may also have lower-level launch/wait timeouts. This
    /// timeout is the crawler envelope around one page visit attempt.
    pub page_open_timeout: Duration,

    /// Whether cached artifacts may be used when they pass current policy.
    pub cache_enabled: bool,
}

impl Default for CrawlEngineConfig {
    fn default() -> Self {
        Self {
            limits: CrawlLimits::default(),
            concurrency: CrawlConcurrency::default(),
            page_open_timeout: Duration::from_secs(45),
            cache_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlLimits {
    /// Maximum number of opened pages per original seed.
    ///
    /// This is the primary crawl budget.
    pub max_pages_per_seed: usize,

    /// Maximum hop depth from the original seed.
    ///
    /// Seeds are depth 0. Links discovered from seeds are depth 1.
    pub max_hop_depth: u32,

    /// Maximum number of URLs retained in memory in the frontier.
    ///
    /// This is a memory/backpressure guard, not a crawl budget.
    pub max_frontier_items: usize,

    /// Optional global emergency brake for one crawl invocation.
    ///
    /// This should not be used as the ordinary crawl budget.
    pub max_total_pages: Option<usize>,
}

impl Default for CrawlLimits {
    fn default() -> Self {
        Self {
            max_pages_per_seed: 10,
            max_hop_depth: 1,
            max_frontier_items: 100_000,
            max_total_pages: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlConcurrency {
    /// Maximum number of page jobs in flight across the whole crawl.
    pub max_concurrent_pages: usize,

    /// Maximum number of live Chromium browser sessions.
    pub max_sessions: usize,

    /// Maximum number of concurrent pages/tabs per browser session.
    ///
    /// Start with 1 for safety, then raise to 2, 4, etc. after testing.
    pub max_concurrent_pages_per_session: usize,

    /// Maximum concurrent cache operations.
    pub max_concurrent_cache_ops: usize,

    /// Rotate a browser session after this many page opens.
    ///
    /// This is a browser health/memory control, not a crawl budget.
    pub max_pages_per_session: usize,
}

impl Default for CrawlConcurrency {
    fn default() -> Self {
        Self {
            max_concurrent_pages: 8,
            max_sessions: 4,
            max_concurrent_pages_per_session: 2,
            max_concurrent_cache_ops: 32,
            max_pages_per_session: 150,
        }
    }
}

