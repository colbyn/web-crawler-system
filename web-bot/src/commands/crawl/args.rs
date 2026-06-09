//! CLI argument model for the crawl command.
//!
//! This file is intentionally Clap-only.
//!
//! The crawl command has two user-facing configuration surfaces:
//!
//! - command-line flags for quick overrides,
//! - TOML settings files for durable crawl profiles.
//!
//! Both surfaces share one vocabulary. TOML uses nested sections such as
//! `[budget] pages = 10`; the CLI uses flattened flags such as `--pages 10`.
//!
//! This module owns only the command-line shape. TOML deserialization and
//! runtime defaults live in `settings.rs`. Shared enums and parsing helpers
//! live in `common.rs`.

use std::path::PathBuf;

use clap::{
    ArgAction,
    Args,
};

use super::common::{
    CrawlInputFormat,
    CrawlOutputFormat,
    CrawlProfileStrategy,
};

#[derive(Args, Debug, Clone)]
pub struct CrawlArgs {
    /// Load crawl settings from a TOML file.
    #[arg(long = "config")]
    pub config: Option<PathBuf>,

    /// URLs to crawl. Repeat this flag or pass whitespace-separated URLs.
    ///
    /// Use `-i -` to explicitly read from stdin.
    #[arg(short = 'i', long = "input", action = ArgAction::Append)]
    pub inputs: Vec<String>,

    /// Input format.
    #[arg(long = "format", value_enum)]
    pub format: Option<CrawlInputFormat>,

    /// JSON Pointer to extract the URL from JSON input.
    #[arg(long = "url-pointer")]
    pub url_pointer: Option<String>,

    /// Attach a global tag to every crawl seed.
    ///
    /// Format: kind:key
    ///
    /// Examples:
    ///
    /// - run:manual-debug
    /// - category:electricians
    #[arg(long = "tag", action = ArgAction::Append)]
    pub tags: Vec<String>,

    /// Attach a tag from a JSON value for each structured input row.
    ///
    /// Format: kind=/json/pointer
    ///
    /// Example:
    ///
    /// --tag-pointer entity=/id
    #[arg(long = "tag-pointer", action = ArgAction::Append)]
    pub tag_pointers: Vec<String>,

    /// Output format.
    #[arg(long = "output", value_enum)]
    pub output: Option<CrawlOutputFormat>,

    /// Deprecated alias for `--output ndjson`.
    #[arg(long = "json", hide = true)]
    pub json: bool,

    /// Maximum opened pages per original seed.
    ///
    /// This is the main crawl budget.
    #[arg(
        long = "pages",
        alias = "max-pages-per-seed",
        alias = "max-pages"
    )]
    pub pages: Option<usize>,

    /// Optional global emergency brake for the whole crawl invocation.
    #[arg(long = "total-pages", alias = "max-total-pages")]
    pub total_pages: Option<usize>,

    /// Maximum hop depth from each original seed.
    ///
    /// Seeds are depth 0. Links discovered from seeds are depth 1.
    #[arg(long = "depth", alias = "max-depth")]
    pub depth: Option<u32>,

    /// Maximum URLs retained in the frontier.
    #[arg(long = "frontier-items", alias = "max-frontier-items")]
    pub frontier_items: Option<usize>,

    /// Global in-flight page jobs across the whole crawl.
    #[arg(long = "jobs", alias = "max-concurrent-pages")]
    pub jobs: Option<usize>,

    /// Maximum live Chromium browser sessions.
    #[arg(long = "sessions", alias = "max-sessions")]
    pub sessions: Option<usize>,

    /// Maximum concurrent tabs/pages per browser session.
    #[arg(long = "tabs", alias = "max-concurrent-pages-per-session")]
    pub tabs: Option<usize>,

    /// Maximum concurrent cache operations.
    #[arg(long = "cache-jobs", alias = "max-concurrent-cache-ops")]
    pub cache_jobs: Option<usize>,

    /// Rotate each browser session after this many page opens.
    #[arg(long = "rotate", alias = "max-pages-per-session")]
    pub rotate: Option<usize>,

    /// Page open timeout in seconds.
    #[arg(long = "timeout-secs", alias = "timeout")]
    pub timeout_secs: Option<u64>,

    /// Browser profile assignment strategy.
    #[arg(long = "profile-strategy", value_enum)]
    pub profile_strategy: Option<CrawlProfileStrategy>,

    /// Fallback/single browser profile key.
    #[arg(long = "profile")]
    pub profile_key: Option<String>,

    /// Optional cache namespace.
    #[arg(long = "namespace")]
    pub namespace: Option<String>,

    /// Disable the crawler's SQLite artifact cache.
    ///
    /// This does not necessarily disable Chromium's own browser profile cache.
    #[arg(long = "no-cache")]
    pub no_cache: bool,

    /// Deprecated/no-op while provenance is represented by tags.
    #[arg(long = "attach-provenance", hide = true)]
    pub attach_provenance: bool,
}

