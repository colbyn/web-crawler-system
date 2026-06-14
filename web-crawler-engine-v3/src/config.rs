//! Engine-level crawl configuration.
//!
//! This module defines crawler limits, concurrency knobs, frontier scheduling
//! options, and general engine behavior.
//!
//! It should not contain browser launch configuration, app-specific business
//! metadata, or persistence details.
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
//!
//! # Frontier scoring model
//!
//! Frontier scoring is an optional scheduling hint.
//!
//! It should not:
//!
//! - decide whether a URL is eligible to crawl,
//! - replace scope or visit policy,
//! - participate in cache identity,
//! - touch durable storage,
//! - fetch pages,
//! - block worker dispatch.
//!
//! The intended use is online prioritization of newly uncovered internal URLs.
//! When an opened page yields anchors, those in-scope discovered URLs can be
//! scored before they enter the frontier. The frontier then remains seed-fair
//! while choosing higher-scored URLs first inside each seed bucket.

use std::time::Duration;

use serde::{
    Deserialize,
    Serialize,
};

use crate::url_score::BuiltinUrlScoringProfile;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlEngineConfig {
    pub limits: CrawlLimits,
    pub concurrency: CrawlConcurrency,

    /// Frontier scheduling configuration.
    ///
    /// This currently controls optional URL scoring for discovered internal
    /// links. Defaults preserve the old behavior: no scoring, neutral frontier
    /// scores, and seed-aware round-robin scheduling.
    pub frontier: FrontierConfig,

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
            frontier: FrontierConfig::default(),
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

/// Frontier scheduling configuration.
///
/// The frontier remains seed-aware and round-robin across seed buckets. These
/// settings only affect how candidate URLs are prioritized inside each seed's
/// bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrontierConfig {
    /// Optional URL scoring for discovered links.
    pub scoring: FrontierScoringConfig,
}

impl Default for FrontierConfig {
    fn default() -> Self {
        Self {
            scoring: FrontierScoringConfig::default(),
        }
    }
}

/// URL scoring configuration for discovered frontier items.
///
/// This uses a plain struct rather than a tagged enum so TOML config stays
/// boring and friendly:
///
/// ```toml
/// [frontier.scoring]
/// enabled = true
/// builtin_profile = "careers"
/// retain_evidence = false
/// ```
///
/// When disabled, discovered URLs enter the frontier with a neutral score.
/// When enabled, the selected built-in profile scores each in-scope discovered
/// URL before it is enqueued.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrontierScoringConfig {
    /// Enable URL scoring for newly discovered frontier items.
    ///
    /// Default is false to preserve existing crawl behavior.
    pub enabled: bool,

    /// Built-in profile used when scoring is enabled.
    ///
    /// The default is `careers` because it is the first concrete scheduling
    /// intent, but it has no effect unless `enabled` is true.
    pub builtin_profile: BuiltinUrlScoringProfile,

    /// Retain verbose score evidence on queued frontier items.
    ///
    /// When false, the frontier stores only the numeric score. This is the best
    /// default for high-throughput crawls because score reasons and labels are
    /// useful for diagnostics but not needed for scheduling.
    pub retain_evidence: bool,
}

impl Default for FrontierScoringConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            builtin_profile: BuiltinUrlScoringProfile::Careers,
            retain_evidence: false,
        }
    }
}
