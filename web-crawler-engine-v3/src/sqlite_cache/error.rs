//! Error types for the SQLite cache.
//!
//! The core design principle is that the **hot path must be extremely forgiving**.
//! `get()` returns `Option<CacheEntry>` and converts almost every per-entry
//! problem (missing entry, decode failure, checksum mismatch, version mismatch,
//! missing primary payload, etc.) into `None`.
//!
//! Only genuine storage-layer failures (I/O errors, SQLite errors, invariant
//! violations) surface as `Err(CacheError)`.
//!
//! Diagnostic and bulk operations may expose richer information in the future
//! (e.g. via `inspect()` and a `CacheMissReason` enum).

use thiserror::Error;

pub type CacheResult<T> = Result<T, CacheError>;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] sqlx::Error),

    #[error("JSON error: {0}")]
    Json(String),

    #[error("Key serialization error: {0}")]
    KeySerialization(String),

    #[error("Cache invariant violation: {0}")]
    Invariant(String),

    #[error("Internal cache error: {0}")]
    Internal(String),
}