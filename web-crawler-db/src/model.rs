//! Core data models for the `web-crawler-db` artifact cache.
//!
//! This module defines the storage-agnostic Rust shapes used by the Postgres
//! implementation.
//!
//! The cache model is intentionally simple:
//!
//! - [`CacheKey`] identifies the requested URL being cached.
//! - [`CacheEntryMetadata`] stores durable, JSON-friendly page provenance and
//!   extracted replay data.
//! - [`CachePayload`] stores the single primary artifact body for the entry.
//! - [`CacheTag`] stores secondary-index associations to caller-owned concepts.
//!
//! This crate currently supports one payload per cache entry. That keeps the
//! API aligned with the real crawler path: one requested URL / page artifact
//! maps to one primary stored body.
//!
//! Multi-payload support, such as screenshots, raw response bodies, rendered
//! HTML, network logs, or other artifacts attached to the same cache entry,
//! should be added later as an explicit schema/API migration if a real caller
//! needs it.
//!
//! Metadata and bytes are deliberately separate:
//!
//! - metadata-only reads avoid loading large `BYTEA` payloads,
//! - payload reads can verify checksum and byte length,
//! - full-entry reads combine metadata, payload, and tags for inspection or
//!   replay.

use serde::{Deserialize, Serialize};

use crate::error::{DbError, DbResult};
use crate::key::CacheKey;
pub use crate::tags::CacheTag;

use web_browser_driver::{
    ExtractedAnchor, NonCriticalBrowserError, PageInfo, PageTelemetry, UrlResolution,
};

/// Entry kind used for ordinary crawled page artifacts.
pub const CACHE_ENTRY_KIND_PAGE: &str = "page";

/// Current metadata shape version.
///
/// Bump this when the persisted shape of [`CacheEntryMetadata`] changes in a
/// backward-incompatible way.
pub const CACHE_METADATA_VERSION: u32 = 3;

/// Versioned metadata for a cached artifact.
///
/// Metadata is stored as JSONB in Postgres. Large artifact bytes should not be
/// embedded here. The primary body lives in [`CachePayload`] and the
/// `cache_payloads` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheEntryMetadata {
    pub metadata_version: u32,
    pub entry_kind: String,
    pub cache_key: CacheKey,
    pub stored_at_unix_ms: i64,
    pub producer: CacheProducerInfo,
    pub request: CacheRequestInfo,
    pub response: CacheResponseInfo,

    /// Low-level browser telemetry captured while opening the page.
    #[serde(default)]
    pub telemetry: Option<PageTelemetry>,

    /// Page-level metadata extracted from the DOM.
    #[serde(default)]
    pub page_info: Option<PageInfo>,

    /// Anchors discovered on the page.
    #[serde(default)]
    pub anchors: Vec<ExtractedAnchor>,

    /// URL resolution facts, including requested URL, final URL, and redirects.
    #[serde(default)]
    pub resolution: Option<UrlResolution>,

    /// Non-critical browser/page errors observed during the visit.
    #[serde(default)]
    pub non_critical_errors: Vec<NonCriticalBrowserError>,
}

impl CacheEntryMetadata {
    /// Construct metadata for an ordinary page artifact.
    pub fn new_page(
        cache_key: CacheKey,
        stored_at_unix_ms: i64,
        producer: CacheProducerInfo,
        request: CacheRequestInfo,
        response: CacheResponseInfo,
    ) -> Self {
        Self {
            metadata_version: CACHE_METADATA_VERSION,
            entry_kind: CACHE_ENTRY_KIND_PAGE.to_string(),
            cache_key,
            stored_at_unix_ms,
            producer,
            request,
            response,
            telemetry: None,
            page_info: None,
            anchors: Vec::new(),
            resolution: None,
            non_critical_errors: Vec::new(),
        }
    }

    /// Borrow the cache key.
    pub fn key(&self) -> &CacheKey {
        &self.cache_key
    }

    /// Validate metadata-only invariants.
    ///
    /// This is intentionally narrower than [`CacheEntry::validate`]. It is used
    /// by metadata-only update/read paths that should not require payload bytes.
    pub fn validate(&self) -> DbResult<()> {
        if self.metadata_version != CACHE_METADATA_VERSION {
            return Err(DbError::invalid_entry(format!(
                "unsupported metadata version {}; expected {}",
                self.metadata_version, CACHE_METADATA_VERSION
            )));
        }

        if self.entry_kind.trim().is_empty() {
            return Err(DbError::invalid_entry("entry_kind must not be empty"));
        }

        if self.stored_at_unix_ms < 0 {
            return Err(DbError::invalid_entry(
                "stored_at_unix_ms must not be negative",
            ));
        }

        validate_non_empty("producer.engine_name", &self.producer.engine_name)?;
        validate_non_empty("producer.engine_version", &self.producer.engine_version)?;

        let key_url = self.cache_key.requested_url.as_str();
        let request_url = self.request.requested_url.as_str();

        if key_url != request_url {
            return Err(DbError::invalid_entry(format!(
                "request URL does not match cache key URL: request={} key={}",
                request_url, key_url
            )));
        }

        let expected_host = self.cache_key.requested_host();
        if self.request.requested_host != expected_host {
            return Err(DbError::invalid_entry(format!(
                "requested_host does not match cache key host: request={:?} key={:?}",
                self.request.requested_host, expected_host
            )));
        }

        Ok(())
    }
}

/// Information about the producer that created the cache artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheProducerInfo {
    pub engine_name: String,
    pub engine_version: String,
    pub driver_version: Option<String>,
    pub cache_policy_version: u32,
}

/// Request-side provenance for a cache artifact.
///
/// This is not cache identity. Identity lives in [`CacheKey`].
///
/// `capture_policy` records producer/browser/profile/policy details that may
/// explain how the artifact was captured.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheRequestInfo {
    pub requested_url: String,
    pub requested_host: Option<String>,

    #[serde(default)]
    pub capture_policy: Option<CacheCapturePolicy>,
}

impl CacheRequestInfo {
    /// Build request provenance from a cache key and capture policy.
    pub fn from_key(cache_key: &CacheKey, capture_policy: Option<CacheCapturePolicy>) -> Self {
        Self {
            requested_url: cache_key.requested_url.to_string(),
            requested_host: cache_key.requested_host(),
            capture_policy,
        }
    }
}

/// Lightweight policy snapshot stored with each cache entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheCapturePolicy {
    pub browser_profile_key: String,
    pub cache_policy_version: u32,
    pub capture_html: bool,
}

/// Response-side provenance for a cache artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheResponseInfo {
    pub final_url: Option<String>,
    pub final_host: Option<String>,
    pub status_code: Option<u16>,
    pub content_type: Option<String>,
}

/// Descriptor for the single stored payload.
///
/// The descriptor describes the bytes exactly as stored in Postgres. If
/// compression is not [`CachePayloadCompression::None`], `sha256_hex` and
/// `byte_len` describe the compressed byte sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CachePayloadDescriptor {
    pub media_type: Option<String>,
    pub compression: CachePayloadCompression,
    pub sha256_hex: String,
    pub byte_len: usize,
}

impl CachePayloadDescriptor {
    /// Validate descriptor-only invariants.
    pub fn validate(&self) -> DbResult<()> {
        if let Some(media_type) = &self.media_type {
            validate_non_empty("payload.media_type", media_type)?;
        }

        if self.sha256_hex.len() != 64
            || !self
                .sha256_hex
                .chars()
                .all(|ch| ch.is_ascii_hexdigit())
        {
            return Err(DbError::invalid_entry(
                "payload has invalid sha256 hex digest",
            ));
        }

        Ok(())
    }
}

/// Compression state of the bytes stored in the payload row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachePayloadCompression {
    None,
    Gzip,
    Zstd,
}

impl CachePayloadCompression {
    /// Return the stable database representation for this compression mode.
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
        }
    }

    /// Parse the stable database representation for this compression mode.
    pub fn from_db_str(value: &str) -> DbResult<Self> {
        match value {
            "none" => Ok(Self::None),
            "gzip" => Ok(Self::Gzip),
            "zstd" => Ok(Self::Zstd),
            other => Err(DbError::invariant(format!(
                "unknown payload compression mode: {other}"
            ))),
        }
    }
}

/// The single primary payload body for a cache entry.
#[derive(Debug, Clone)]
pub struct CachePayload {
    pub descriptor: CachePayloadDescriptor,
    pub body: Vec<u8>,
}

impl CachePayload {
    /// Construct a payload from stored bytes.
    ///
    /// The checksum and byte length are computed from `body` as supplied.
    pub fn new(
        media_type: Option<String>,
        compression: CachePayloadCompression,
        body: Vec<u8>,
    ) -> Self {
        let sha256_hex = sha256_hex(&body);
        let byte_len = body.len();

        Self {
            descriptor: CachePayloadDescriptor {
                media_type,
                compression,
                sha256_hex,
                byte_len,
            },
            body,
        }
    }

    /// Validate descriptor/body consistency.
    pub fn validate(&self) -> DbResult<()> {
        self.descriptor.validate()?;

        if self.descriptor.byte_len != self.body.len() {
            return Err(DbError::invalid_entry(format!(
                "payload byte length mismatch: descriptor={} actual={}",
                self.descriptor.byte_len,
                self.body.len()
            )));
        }

        let observed_sha256_hex = sha256_hex(&self.body);

        if self.descriptor.sha256_hex != observed_sha256_hex {
            return Err(DbError::invalid_entry("payload checksum mismatch"));
        }

        Ok(())
    }
}

/// A complete cache entry.
///
/// This combines metadata, the single primary payload body, and secondary-index
/// tags. Hot replay paths may read metadata only and fetch payload bytes only
/// when needed.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub metadata: CacheEntryMetadata,
    pub payload: CachePayload,
    pub tags: Vec<CacheTag>,
}

impl CacheEntry {
    /// Construct a validated page cache entry.
    pub fn new_page(
        cache_key: CacheKey,
        stored_at_unix_ms: i64,
        producer: CacheProducerInfo,
        request: CacheRequestInfo,
        response: CacheResponseInfo,
        payload: CachePayload,
        tags: Vec<CacheTag>,
    ) -> DbResult<Self> {
        let metadata = CacheEntryMetadata::new_page(
            cache_key,
            stored_at_unix_ms,
            producer,
            request,
            response,
        );

        let entry = Self {
            metadata,
            payload,
            tags,
        };

        entry.validate()?;

        Ok(entry)
    }

    /// Borrow the cache key.
    pub fn key(&self) -> &CacheKey {
        self.metadata.key()
    }

    /// Validate entry-level invariants before persistence.
    pub fn validate(&self) -> DbResult<()> {
        self.metadata.validate()?;
        self.payload.validate()?;
        Ok(())
    }
}

/// Lightweight reference to a cache entry for list/query APIs.
///
/// This avoids loading payload bodies when callers only need entry metadata.
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

/// Compute SHA-256 hex digest of bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    hex::encode(Sha256::digest(bytes))
}

fn validate_non_empty(field: &str, value: &str) -> DbResult<()> {
    if value.trim().is_empty() {
        return Err(DbError::invalid_entry(format!("{field} must not be empty")));
    }

    Ok(())
}

