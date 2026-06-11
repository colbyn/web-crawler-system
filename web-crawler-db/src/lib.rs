//! `web-crawler-db` — PostgreSQL-backed artifact cache for the web crawler.
//!
//! This crate owns durable storage for reusable crawl artifacts:
//!
//! - cache identity,
//! - metadata/provenance,
//! - extracted replay JSON,
//! - payload descriptors,
//! - payload bytes,
//! - secondary-index tags,
//! - small auxiliary JSON sidecars.
//!
//! It is intentionally **not** the durable crawl scheduler. Task-centric state
//! such as frontier queues, retry waves, seed attempt ordering, leases, and run
//! resumption should live in a higher-level crawl/task store.
//!
//! ## Runtime versus administration
//!
//! Runtime code should use [`PostgresCache::connect`] to connect to an existing
//! database schema.
//!
//! Schema setup is explicit. Use [`migrate_pool`] or [`migrate_database_url`]
//! from setup scripts, tests, or a CLI helper. `connect()` does not migrate.
//!
//! ## Hot-path design
//!
//! The cache API separates metadata replay from payload inspection:
//!
//! - metadata-only reads support warm-cache replay without loading large bodies,
//! - payload-specific reads load bytes only when requested,
//! - full-entry reads remain available for inspection/export/debugging,
//! - the forgiving `get()` API can degrade corrupt per-entry data into a miss.
//!
//! ## Tags
//!
//! Tags are secondary-index associations represented as `(kind, key)`. They are
//! not cache identity and are not arbitrary metadata. Use metadata JSON,
//! extracted JSON, telemetry JSON, payload descriptors, or auxiliary storage for
//! richer structured data.

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
    sha256_hex, CacheEntry, CacheEntryMetadata, CacheEntryRef, CachePayload,
    CachePayloadCompression, CachePayloadDescriptor, CachePayloadRole, CacheProducerInfo,
    CacheRequestInfo, CacheResponseInfo, CACHE_ENTRY_KIND_PAGE, CACHE_METADATA_VERSION,
};
pub use postgres::PostgresCache;
pub use tags::CacheTag;
pub use time::now_unix_ms;

