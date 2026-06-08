//! Error types for crawler orchestration.
//!
//! This module represents failures at the crawler layer. Browser/CDP/page
//! errors should originate from `web-browser-driver` and be wrapped here only
//! when they affect crawl orchestration.
//!
//! Important distinction:
//!
//! - A page visit may fail without the entire crawl failing.
//! - A cache artifact may fail to decode without the crawl failing.
//! - A browser session may become poisoned, requiring session replacement.
//! - A caller may still receive partial crawl results.
//!
//! This crate should avoid panic-style failure for ordinary web weirdness.
//! Broken pages, bad redirects, noisy network behavior, and stale cache entries
//! are expected crawl inputs.

use thiserror::Error;

pub type CrawlEngineResult<T> = Result<T, CrawlEngineError>;

#[derive(Debug, Error)]
pub enum CrawlEngineError {
    #[error("browser driver error: {0}")]
    Browser(#[from] web_browser_driver::BrowserDriverError),

    #[error("cache error: {0}")]
    Cache(#[from] crate::cache::CrawlCacheError),

    #[error("URL parse error: {0}")]
    Url(#[from] url::ParseError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("crawl policy rejected request: {0}")]
    PolicyRejected(String),

    #[error("frontier error: {0}")]
    Frontier(String),

    #[error("scheduler error: {0}")]
    Scheduler(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("internal crawler error: {0}")]
    Internal(String),
}
