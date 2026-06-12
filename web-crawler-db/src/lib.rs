//! `web-crawler-db` is the PostgreSQL-backed artifact cache for the web crawler.
//!
//! This crate owns durable storage for reusable crawl artifacts:
//!
//! - cache identity,
//! - metadata and provenance,
//! - extracted replay data,
//! - one primary payload body,
//! - secondary-index tags,
//! - small auxiliary JSON sidecars.
//!
//! It is intentionally not the durable crawl scheduler.
//!
//! Task-centric state such as frontier queues, retry waves, seed attempt
//! ordering, leases, run resumption, and worker coordination should live in a
//! higher-level crawl/task store.
//!
//! ## Runtime versus administration
//!
//! Runtime code should use [`PostgresCache::connect`] to connect to an existing
//! database schema.
//!
//! Schema setup is explicit. Use [`migrate_pool`] or [`migrate_database_url`]
//! from setup scripts, tests, or a CLI helper.
//!
//! [`PostgresCache::connect`] does not create or migrate schema.
//!
//! ## Cache shape
//!
//! The current cache model is intentionally simple:
//!
//! ```text
//! CacheKey -> CacheEntry
//!
//! CacheEntry = metadata + one primary payload body + tags
//! ```
//!
//! Multi-payload support is deliberately not part of this revision. If the
//! crawler later needs screenshots, raw response bodies, rendered snapshots,
//! network logs, or other independent artifacts attached to the same cache
//! entry, that should be added as an explicit schema/API migration.
//!
//! ## Hot-path design
//!
//! The cache API separates metadata replay from payload inspection:
//!
//! - metadata-only reads support warm-cache replay without loading large bodies,
//! - payload reads load the single primary body only when requested,
//! - full-entry reads remain available for inspection, export, and debugging,
//! - the forgiving [`PostgresCache::get`] API can degrade corrupt per-entry data
//!   into a cache miss.
//!
//! ## Tags
//!
//! Tags are secondary-index associations represented as `(kind, key)`.
//!
//! Tags are not cache identity and are not arbitrary metadata. Use metadata
//! fields or auxiliary JSON for richer structured data.
//!
//! Tag writes are merge-oriented by default so incremental crawler phases can
//! attach additional associations without erasing earlier ones.

pub mod error;
pub mod key;
pub mod migrate;
pub mod model;
pub mod postgres;
pub mod queries;
pub mod tags;
pub mod time;

pub use error::{DbError, DbResult};
pub use key::{cache_key_digest, CacheKey};
pub use migrate::{migrate_database_url, migrate_pool};
pub use model::{
    sha256_hex, CacheCapturePolicy, CacheEntry, CacheEntryMetadata, CacheEntryRef, CachePayload,
    CachePayloadCompression, CachePayloadDescriptor, CacheProducerInfo, CacheRequestInfo,
    CacheResponseInfo, CACHE_ENTRY_KIND_PAGE, CACHE_METADATA_VERSION,
};
pub use postgres::PostgresCache;
pub use tags::CacheTag;
pub use time::now_unix_ms;

