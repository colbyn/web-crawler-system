//! Core data models for the SQLite cache.
//!
//! This module defines the serializable in-memory shapes stored by
//! [`crate::sqlite_cache::SqliteCache`].
//!
//! The cache model deliberately separates four concepts that are easy to blur:
//!
//! 1. **Identity**
//!    [`CacheKey`] decides whether a request is eligible to reuse a cached
//!    artifact. It should stay small and stable: requested URL, namespace, and
//!    key/schema versions.
//!
//! 2. **Request / response metadata**
//!    [`CacheRequestInfo`] and [`CacheResponseInfo`] describe what happened
//!    during a crawl: requested URL, final URL, status code, content type,
//!    browser/profile provenance, and similar runtime facts.
//!
//! 3. **Payloads**
//!    [`CachePayload`] stores binary artifacts such as HTML snapshots,
//!    response bodies, screenshots, or future media captures. Payload metadata
//!    is duplicated into [`CachePayloadDescriptor`] so integrity can be checked
//!    when loading from SQLite.
//!
//! 4. **Associations**
//!    [`crate::sqlite_cache::CacheTag`] links one cached artifact to many
//!    caller-level concepts: entities, batches, categories, runs, imports,
//!    campaigns, and downstream workflows.
//!
//! ## Important boundary
//!
//! Browser/driver profile IDs are **metadata**, not cache identity.
//!
//! A profile can tell us which Chrome partition produced an artifact, which is
//! useful for debugging and provenance. But putting that profile ID into
//! [`CacheKey`] fragments the shared cache by worker/session assignment and
//! causes unnecessary misses.
//!
//! If the observable page truly varies by crawl context, represent that
//! deliberately with namespace or a future semantic vary dimension such as
//! `anonymous-us-ut`, `logged-out`, `mobile-en-us`, or `proxy-region-eu`.
//! Do not use raw Chrome profile IDs as artifact identity.
//!
//! ## Evolution strategy
//!
//! Metadata is versioned and includes flexible JSON fields so extractors,
//! telemetry, and downstream processors can evolve without a SQLite migration
//! for every new field. Breaking identity changes belong in
//! [`crate::sqlite_cache::CACHE_KEY_VERSION`]; breaking metadata changes belong
//! in [`CACHE_METADATA_VERSION`].

use serde::{
    Deserialize,
    Serialize,
};

use crate::sqlite_cache::{
    CacheKey,
    CacheTag,
};

pub const CACHE_ENTRY_KIND_PAGE: &str = "page";
pub const CACHE_METADATA_VERSION: u32 = 1;

/// Versioned metadata for a cached artifact.
///
/// This is the durable envelope around payloads. It stores enough context to
/// inspect, debug, validate, and post-process a cached crawl result without
/// needing to rerun the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheEntryMetadata {
    pub metadata_version: u32,
    pub entry_kind: String,

    /// Logical cache identity for this artifact.
    ///
    /// This should match the key used to address the row in SQLite. It should
    /// not contain runtime facts such as browser profile, final URL, status
    /// code, or telemetry.
    pub cache_key: CacheKey,

    pub stored_at_unix_ms: i64,
    pub producer: CacheProducerInfo,
    pub request: CacheRequestInfo,
    pub response: CacheResponseInfo,

    /// Flexible JSON for driver/browser telemetry.
    ///
    /// Examples:
    ///
    /// - navigation timing,
    /// - network idle measurements,
    /// - browser/CDP warnings,
    /// - page readiness observations,
    /// - retry/capture diagnostics.
    #[serde(default)]
    pub telemetry_json: serde_json::Value,

    /// Flexible JSON for extracted page facts.
    ///
    /// Examples:
    ///
    /// - title/meta description,
    /// - headings,
    /// - links,
    /// - forms,
    /// - contact facts,
    /// - structured data.
    #[serde(default)]
    pub extracted_json: serde_json::Value,

    /// Non-fatal errors captured during crawl.
    ///
    /// These are warnings worth preserving, not necessarily reasons to reject
    /// the artifact. Fatal storage/load problems should surface through
    /// [`crate::sqlite_cache::CacheError`] or become hot-path misses.
    #[serde(default)]
    pub non_critical_errors_json: Vec<serde_json::Value>,

    /// Descriptors for binary payloads stored in `cache_payloads`.
    ///
    /// The actual bytes live outside metadata so large snapshots and future
    /// binary captures do not become JSON strings.
    #[serde(default)]
    pub payloads: Vec<CachePayloadDescriptor>,
}

impl CacheEntryMetadata {
    pub fn new_page(
        cache_key: CacheKey,
        stored_at_unix_ms: i64,
        producer: CacheProducerInfo,
        request: CacheRequestInfo,
        response: CacheResponseInfo,
        payloads: Vec<CachePayloadDescriptor>,
    ) -> Self {
        Self {
            metadata_version: CACHE_METADATA_VERSION,
            entry_kind: CACHE_ENTRY_KIND_PAGE.to_string(),
            cache_key,
            stored_at_unix_ms,
            producer,
            request,
            response,
            telemetry_json: serde_json::Value::Null,
            extracted_json: serde_json::Value::Null,
            non_critical_errors_json: Vec::new(),
            payloads,
        }
    }

    /// Return the conventional primary content payload descriptor.
    ///
    /// `PrimarySnapshot` is preferred because the crawler usually wants the DOM
    /// snapshot it intentionally captured. `ResponseBody` is a fallback for
    /// simpler fetch-style artifacts or future cache entries that do not have a
    /// rendered DOM snapshot.
    pub fn primary_payload(&self) -> Option<&CachePayloadDescriptor> {
        self.payloads
            .iter()
            .find(|p| p.role == CachePayloadRole::PrimarySnapshot)
            .or_else(|| {
                self.payloads
                    .iter()
                    .find(|p| p.role == CachePayloadRole::ResponseBody)
            })
    }
}

/// Identifies the software/policy version that produced an artifact.
///
/// This helps determine whether an old artifact should be trusted, recrawled,
/// reprocessed, or simply treated as a cache miss by higher-level policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheProducerInfo {
    pub engine_name: String,
    pub engine_version: String,
    pub driver_version: Option<String>,
    pub cache_policy_version: u32,
}

/// Request-side metadata captured with a cache entry.
///
/// This is provenance and diagnostics, not identity. The cache identity lives in
/// [`CacheKey`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheRequestInfo {
    /// The URL originally requested by the crawler.
    ///
    /// This should normally mirror `cache_key.requested_url` as a string for
    /// easy SQLite inspection and JSON querying.
    pub requested_url: String,

    /// Host extracted from the requested URL for indexing and inspection.
    pub requested_host: Option<String>,

    /// Browser/driver profile used to produce this artifact.
    ///
    /// This is intentionally metadata only. It must not participate in cache
    /// identity. Different workers or Chrome profiles should be able to reuse
    /// the same cached artifact when the requested URL and namespace match.
    pub profile_key_json: serde_json::Value,

    /// Logical cache namespace used for this request.
    ///
    /// This should normally mirror `cache_key.namespace`.
    pub namespace: Option<String>,
}

/// Response-side observations captured during crawl.
///
/// These are runtime facts. They are useful for inspection, policy decisions,
/// redirects, and downstream processing, but they are not the primary cache key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheResponseInfo {
    pub final_url: Option<String>,
    pub final_host: Option<String>,
    pub status_code: Option<u16>,
    pub content_type: Option<String>,
}

/// Metadata describing one binary payload stored in SQLite.
///
/// The descriptor is kept in metadata so loaders can validate payload integrity
/// after reading bytes from `cache_payloads`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CachePayloadDescriptor {
    pub payload_id: String,
    pub role: CachePayloadRole,
    pub media_type: Option<String>,
    pub compression: CachePayloadCompression,
    pub sha256_hex: String,
    pub byte_len: usize,
}

/// Semantic role for a payload.
///
/// Roles allow callers to find the important artifact without hard-coding
/// payload IDs. More roles can be added later without changing the payload table
/// shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachePayloadRole {
    /// Main rendered snapshot captured by the crawler.
    PrimarySnapshot,

    /// Raw response body or fetch-style content.
    ResponseBody,

    /// Screenshot or visual capture.
    Screenshot,

    /// Extension point for payloads not yet modeled explicitly.
    Other,
}

impl CachePayloadRole {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::PrimarySnapshot => "primary_snapshot",
            Self::ResponseBody => "response_body",
            Self::Screenshot => "screenshot",
            Self::Other => "other",
        }
    }
}

/// Compression used for a stored payload body.
///
/// The current implementation may store only uncompressed bytes, but the enum
/// keeps the database ready for larger artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachePayloadCompression {
    None,
    Gzip,
    Zstd,
}

impl CachePayloadCompression {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
        }
    }
}

/// Binary payload plus descriptor.
///
/// The body is stored as a SQLite BLOB. Integrity fields are computed at
/// construction time and checked when loading.
#[derive(Debug, Clone)]
pub struct CachePayload {
    pub descriptor: CachePayloadDescriptor,
    pub body: Vec<u8>,
}

impl CachePayload {
    pub fn new(
        payload_id: impl Into<String>,
        role: CachePayloadRole,
        media_type: Option<String>,
        compression: CachePayloadCompression,
        body: Vec<u8>,
    ) -> Self {
        let sha256_hex = crate::sqlite_cache::model::sha256_hex(&body);
        let byte_len = body.len();

        Self {
            descriptor: CachePayloadDescriptor {
                payload_id: payload_id.into(),
                role,
                media_type,
                compression,
                sha256_hex,
                byte_len,
            },
            body,
        }
    }
}

/// Full cache artifact loaded from or written to SQLite.
///
/// Tags are carried with the entry for convenience, but they are not part of
/// [`CacheKey`] and are stored through the secondary tag index.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub metadata: CacheEntryMetadata,
    pub payloads: Vec<CachePayload>,
    pub tags: Vec<CacheTag>,
}

impl CacheEntry {
    pub fn key(&self) -> &CacheKey {
        &self.metadata.cache_key
    }
}

/// Lightweight reference returned by tag/listing queries.
///
/// This intentionally omits payload bytes so browsing cache contents remains
/// cheap. Use [`crate::sqlite_cache::SqliteCache::get`] with the embedded key to
/// load the full entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheEntryRef {
    pub key_digest: String,
    pub key: CacheKey,
    pub requested_url: String,
    pub final_url: Option<String>,
    pub stored_at_unix_ms: i64,
    pub entry_kind: String,
    pub metadata_version: u32,
    pub status_code: Option<u16>,
    pub content_type: Option<String>,
}

/// Compute the SHA-256 digest of payload bytes as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{
        Digest,
        Sha256,
    };

    hex::encode(Sha256::digest(bytes))
}
