//! Error types for the `web-crawler-db` artifact cache.
//!
//! This crate is used in two very different paths:
//!
//! - the crawler hot path, where cache problems should usually degrade into a
//!   cache miss,
//! - administrative, migration, validation, and inspection paths, where exact
//!   failure reasons matter.
//!
//! The error model reflects that split.
//!
//! `PostgresCache::get()` may remain intentionally forgiving and convert many
//! per-entry failures into `None`. Lower-level and stricter APIs should return
//! `DbResult<T>` so callers can distinguish database failures, malformed caller
//! input, JSON decoding problems, and stored-data corruption.
//!
//! In particular:
//!
//! - [`DbError::InvalidEntry`] means the caller attempted to write an invalid
//!   cache object.
//! - [`DbError::Invariant`] means data already stored in the database violated
//!   an internal cache invariant.
//!
//! That distinction is important. Bad input should be rejected before it reaches
//! storage. Bad stored data should be diagnosable, repairable, and safe for the
//! crawler hot path to treat as a cache miss.

use thiserror::Error;

/// Standard result type used by the database cache layer.
pub type DbResult<T> = Result<T, DbError>;

/// Errors produced by the database cache and artifact-store layer.
#[derive(Debug, Error)]
pub enum DbError {
    /// Filesystem or process I/O failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// PostgreSQL/sqlx operation failed.
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// JSON serialization or deserialization failed.
    #[error("JSON error: {0}")]
    Json(String),

    /// Stable cache-key serialization failed.
    #[error("Key serialization error: {0}")]
    KeySerialization(String),

    /// Caller attempted to write an invalid cache entry.
    ///
    /// This is used for validation failures caught before persistence, such as:
    ///
    /// - duplicate payload IDs,
    /// - payload descriptors that do not match payload bodies,
    /// - metadata request URL that disagrees with the cache key URL.
    #[error("Invalid cache entry: {0}")]
    InvalidEntry(String),

    /// Stored data violated an internal cache invariant.
    ///
    /// This generally means the database contains malformed, incomplete, or
    /// contradictory cache data. The forgiving hot path may convert this into a
    /// cache miss, while strict diagnostic APIs should surface it.
    #[error("Cache invariant violation: {0}")]
    Invariant(String),

    /// Internal library error that does not fit a narrower category.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl DbError {
    /// Convenience constructor for caller-provided invalid cache objects.
    pub fn invalid_entry(message: impl Into<String>) -> Self {
        Self::InvalidEntry(message.into())
    }

    /// Convenience constructor for stored-data invariant violations.
    pub fn invariant(message: impl Into<String>) -> Self {
        Self::Invariant(message.into())
    }

    /// Convenience constructor for JSON errors when the source error type is not
    /// being preserved directly.
    pub fn json(message: impl Into<String>) -> Self {
        Self::Json(message.into())
    }

    /// Convenience constructor for internal errors.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

