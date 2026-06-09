//! `sqlite_cache` — SQLite-backed dual-role cache for the web crawler.
//!
//! This module is the primary storage layer for the crawler engine. It serves
//! two distinct purposes:
//!
//! 1. **Performance cache** — Short-circuit repeated expensive browser work
//!    (page opens, extractions, etc.) by returning previously computed results.
//! 2. **Persistent artifact store** — Hold completed crawl results and derived
//!    data so that downstream components and higher-level applications can
//!    reliably retrieve and build upon them.
//!
//! The design emphasizes:
//! - A very forgiving hot path (`get` returns `None` on any problem with an entry).
//! - Strong support for tags as a flexible linking mechanism between crawler
//!   results and application-level concepts (entities, jobs, batches, experiments).
//! - Per-entry auxiliary key/value storage for post-processing pipelines.
//! - Human and tool inspectability (pretty JSON metadata + useful indexes).
//! - Safe schema evolution through versioning and flexible JSON fields.
//!
//! There is **no trait abstraction** — `SqliteCache` is the concrete implementation.
//! This allows easy addition of SQLite-specific capabilities over time.
//!
//! # Module Organization
//! - `cache`   — Main `SqliteCache` type and public API
//! - `model`   — Core data shapes (`CacheEntry`, `CacheEntryMetadata`, payloads, etc.)
//! - `key`     — `CacheKey` definition and digest computation
//! - `tags`    — Tag normalization and tag operations
//! - `error`   — Error types with forgiving hot-path semantics
//! - `time`    — Small timestamp helper


pub mod cache;
pub mod error;
pub mod key;
pub mod model;
pub mod tags;
pub mod time;

pub use cache::SqliteCache;
pub use error::{CacheError, CacheResult};
pub use key::{cache_key_digest, CacheKey, CACHE_KEY_SCHEMA_VERSION, CACHE_KEY_VERSION};
pub use model::{
    CacheEntry, CacheEntryMetadata, CacheEntryRef, CachePayload, CachePayloadCompression,
    CachePayloadDescriptor, CachePayloadRole, CacheProducerInfo, CacheRequestInfo,
    CacheResponseInfo, CACHE_ENTRY_KIND_PAGE, CACHE_METADATA_VERSION,
};
pub use tags::CacheTag;
pub use time::now_unix_ms;

