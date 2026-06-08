//! Cache errors.
//!
//! Cache errors should usually be recoverable at the crawler layer. A stale,
//! corrupt, missing, or undecodable cache entry is normally a signal to recrawl,
//! not to fail the entire crawl.
//!
//! Filesystem permission errors and severe I/O failures are still surfaced so
//! callers can decide whether cache failure should be fatal in their context.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CrawlCacheError {
    #[error("cache I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("cache encode error: {0}")]
    Encode(String),

    #[error("cache decode error: {0}")]
    Decode(String),

    #[error("cache key serialization error: {0}")]
    KeySerialization(String),

    #[error("cache artifact rejected: {0}")]
    Rejected(String),

    #[error("cache internal error: {0}")]
    Internal(String),
}

