//! Engine-level crawl configuration.
//!
//! This module defines general crawler limits and knobs. It should not contain
//! browser launch configuration, app-specific business metadata, or persistence
//! details.
//!
//! Browser launch settings belong to `web-browser-driver`.
//! App-specific settings belong above this crate.
//! Durable storage configuration belongs either in cache/store implementations
//! or downstream applications.
//!
//! The values here are deliberately boring. They define how much work the engine
//! may perform and how aggressively it may expand the crawl frontier.

use std::time::Duration;

use serde::{
    Deserialize,
    Serialize,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlEngineConfig {
    pub limits: CrawlLimits,

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
            page_open_timeout: Duration::from_secs(45),
            cache_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlLimits {
    /// Maximum number of pages to visit for one crawl invocation.
    ///
    /// This is an engine-level guardrail, not an app-level quota system.
    pub max_pages: usize,

    /// Maximum hop depth from the original seed.
    ///
    /// Seeds are depth 0. Links discovered from seeds are depth 1.
    pub max_hop_depth: u32,

    /// Maximum number of URLs retained in memory in the frontier.
    pub max_frontier_items: usize,
}

impl Default for CrawlLimits {
    fn default() -> Self {
        Self {
            max_pages: 10,
            max_hop_depth: 1,
            max_frontier_items: 10_000,
        }
    }
}

