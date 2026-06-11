//! Core data models for the `web-crawler-db` artifact cache.
//!
//! This module defines the storage-agnostic Rust shapes used by the Postgres
//! implementation.
//!
//! The cache model has four layers:
//!
//! - [`CacheKey`] identifies the requested URL being cached.
//! - [`CacheEntryMetadata`] stores durable, queryable, JSON-friendly provenance
//!   and extracted replay data.
//! - [`CachePayload`] stores artifact bytes such as rendered HTML, raw response
//!   bodies, screenshots, or future binary/text artifacts.
//! - [`CacheTag`] stores secondary-index associations to caller-owned concepts
//!   such as entities, categories, batches, datasets, or campaigns.
//!
//! A crucial invariant is that payload descriptors in metadata must match the
//! actual payload list exactly. Metadata describes payloads; payload rows store
//! bytes. If those drift apart, cache replay becomes haunted furniture.
//!
//! New write paths should prefer [`CacheEntry::new_page`], which derives
//! metadata payload descriptors from the supplied payloads. Lower-level callers
//! may still construct structs directly, but [`CacheEntry::validate`] should be
//! called before persistence.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::{DbError, DbResult};
use crate::key::CacheKey;
pub use crate::tags::CacheTag;

/// Entry kind used for ordinary crawled page artifacts.
pub const CACHE_ENTRY_KIND_PAGE: &str = "page";

/// Current metadata shape version.
///
/// This version describes the JSON metadata contract, not the cache-key digest
/// contract and not the SQL schema version.
pub const CACHE_METADATA_VERSION: u32 = 1;

/// Versioned metadata for a cached artifact.
///
/// Metadata is intentionally JSON-friendly because it is stored as JSONB in
/// Postgres. Large artifact bytes should not be embedded here. Put bytes in
/// [`CachePayload`] rows and keep only descriptors in `payloads`.
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

    #[serde(default)]
    pub telemetry_json: serde_json::Value,

    #[serde(default)]
    pub extracted_json: serde_json::Value,

    #[serde(default)]
    pub non_critical_errors_json: Vec<serde_json::Value>,

    #[serde(default)]
    pub payloads: Vec<CachePayloadDescriptor>,
}

impl CacheEntryMetadata {
    /// Construct metadata for a page artifact from explicit payload descriptors.
    ///
    /// Most write paths should prefer [`CacheEntry::new_page`], which derives
    /// descriptors from actual payloads and validates the finished entry.
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

    /// Construct metadata for a page artifact by deriving descriptors from
    /// actual payloads.
    pub fn new_page_from_payloads(
        cache_key: CacheKey,
        stored_at_unix_ms: i64,
        producer: CacheProducerInfo,
        request: CacheRequestInfo,
        response: CacheResponseInfo,
        payloads: &[CachePayload],
    ) -> Self {
        Self::new_page(
            cache_key,
            stored_at_unix_ms,
            producer,
            request,
            response,
            payloads
                .iter()
                .map(|payload| payload.descriptor.clone())
                .collect(),
        )
    }

    /// Return the preferred primary payload descriptor for replay.
    ///
    /// `PrimarySnapshot` is preferred. `ResponseBody` is a fallback so older or
    /// alternate capture strategies can still provide a main body artifact.
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
/// `capture_policy_json` records producer/browser/profile/policy details that
/// may explain how the artifact was captured. It is intentionally provenance,
/// not key material.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheRequestInfo {
    pub requested_url: String,
    pub requested_host: Option<String>,

    #[serde(default)]
    pub capture_policy_json: serde_json::Value,
}

impl CacheRequestInfo {
    /// Build request provenance from a cache key and capture-policy JSON.
    pub fn from_key(cache_key: &CacheKey, capture_policy_json: serde_json::Value) -> Self {
        Self {
            requested_url: cache_key.requested_url.to_string(),
            requested_host: cache_key.requested_host(),
            capture_policy_json,
        }
    }
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

/// Metadata descriptor for one stored payload.
///
/// Descriptors live inside [`CacheEntryMetadata`]. Payload bytes live in
/// [`CachePayload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CachePayloadDescriptor {
    pub payload_id: String,
    pub role: CachePayloadRole,
    pub media_type: Option<String>,
    pub compression: CachePayloadCompression,
    pub sha256_hex: String,
    pub byte_len: usize,
}

/// Semantic role of a payload within a cache entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachePayloadRole {
    PrimarySnapshot,
    ResponseBody,
    Screenshot,
    Other,
}

impl CachePayloadRole {
    /// Return the stable database representation for this role.
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::PrimarySnapshot => "primary_snapshot",
            Self::ResponseBody => "response_body",
            Self::Screenshot => "screenshot",
            Self::Other => "other",
        }
    }
}

/// Compression state of the bytes stored in a payload row.
///
/// `sha256_hex` and `byte_len` in [`CachePayloadDescriptor`] describe the bytes
/// as stored, not necessarily the decoded/original body.
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
}

/// One cache payload and its descriptor.
///
/// The descriptor is computed when the payload is constructed. If callers mutate
/// fields manually, [`CacheEntry::validate`] will catch descriptor/body drift
/// before persistence.
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
        payload_id: impl Into<String>,
        role: CachePayloadRole,
        media_type: Option<String>,
        compression: CachePayloadCompression,
        body: Vec<u8>,
    ) -> Self {
        let sha256_hex = sha256_hex(&body);
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

    /// Borrow the payload ID.
    pub fn payload_id(&self) -> &str {
        &self.descriptor.payload_id
    }
}

/// A complete cache entry.
///
/// This includes metadata, payload bytes, and secondary-index tags. Hot replay
/// paths should usually read metadata only and fetch payload bytes only when
/// needed.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub metadata: CacheEntryMetadata,
    pub payloads: Vec<CachePayload>,
    pub tags: Vec<CacheTag>,
}

impl CacheEntry {
    /// Construct a validated page cache entry.
    ///
    /// Payload descriptors are derived from `payloads`, which prevents metadata
    /// and payload bytes from drifting apart at construction time.
    pub fn new_page(
        cache_key: CacheKey,
        stored_at_unix_ms: i64,
        producer: CacheProducerInfo,
        request: CacheRequestInfo,
        response: CacheResponseInfo,
        payloads: Vec<CachePayload>,
        tags: Vec<CacheTag>,
    ) -> DbResult<Self> {
        let metadata = CacheEntryMetadata::new_page_from_payloads(
            cache_key,
            stored_at_unix_ms,
            producer,
            request,
            response,
            &payloads,
        );

        let entry = Self {
            metadata,
            payloads,
            tags,
        };

        entry.validate()?;
        Ok(entry)
    }

    /// Borrow the cache key.
    pub fn key(&self) -> &CacheKey {
        &self.metadata.cache_key
    }

    /// Validate entry-level invariants before persistence.
    ///
    /// This protects the database from caller-created impossible states.
    pub fn validate(&self) -> DbResult<()> {
        self.validate_metadata_identity()?;
        self.validate_payloads()?;
        Ok(())
    }

    fn validate_metadata_identity(&self) -> DbResult<()> {
        if self.metadata.metadata_version != CACHE_METADATA_VERSION {
            return Err(DbError::invalid_entry(format!(
                "unsupported metadata version {}; expected {}",
                self.metadata.metadata_version, CACHE_METADATA_VERSION
            )));
        }

        if self.metadata.entry_kind.trim().is_empty() {
            return Err(DbError::invalid_entry("entry_kind must not be empty"));
        }

        let key_url = self.metadata.cache_key.requested_url.as_str();
        let request_url = self.metadata.request.requested_url.as_str();

        if key_url != request_url {
            return Err(DbError::invalid_entry(format!(
                "request URL does not match cache key URL: request={} key={}",
                request_url, key_url
            )));
        }

        let expected_host = self.metadata.cache_key.requested_host();
        if self.metadata.request.requested_host != expected_host {
            return Err(DbError::invalid_entry(format!(
                "requested_host does not match cache key host: request={:?} key={:?}",
                self.metadata.request.requested_host, expected_host
            )));
        }

        Ok(())
    }

    fn validate_payloads(&self) -> DbResult<()> {
        let mut payload_ids = HashSet::new();

        for payload in &self.payloads {
            validate_payload_descriptor(&payload.descriptor)?;

            if !payload_ids.insert(payload.descriptor.payload_id.as_str()) {
                return Err(DbError::invalid_entry(format!(
                    "duplicate payload id in payload list: {}",
                    payload.descriptor.payload_id
                )));
            }

            if payload.descriptor.byte_len != payload.body.len() {
                return Err(DbError::invalid_entry(format!(
                    "payload byte length mismatch for {}: descriptor={} actual={}",
                    payload.descriptor.payload_id,
                    payload.descriptor.byte_len,
                    payload.body.len()
                )));
            }

            let observed_sha256_hex = sha256_hex(&payload.body);
            if payload.descriptor.sha256_hex != observed_sha256_hex {
                return Err(DbError::invalid_entry(format!(
                    "payload checksum mismatch for {}",
                    payload.descriptor.payload_id
                )));
            }
        }

        let mut descriptor_ids = HashSet::new();

        for descriptor in &self.metadata.payloads {
            validate_payload_descriptor(descriptor)?;

            if !descriptor_ids.insert(descriptor.payload_id.as_str()) {
                return Err(DbError::invalid_entry(format!(
                    "duplicate payload id in metadata descriptors: {}",
                    descriptor.payload_id
                )));
            }
        }

        if self.metadata.payloads.len() != self.payloads.len() {
            return Err(DbError::invalid_entry(format!(
                "metadata payload descriptor count {} does not match payload count {}",
                self.metadata.payloads.len(),
                self.payloads.len()
            )));
        }

        for descriptor in &self.metadata.payloads {
            let Some(payload) = self
                .payloads
                .iter()
                .find(|payload| payload.descriptor.payload_id == descriptor.payload_id)
            else {
                return Err(DbError::invalid_entry(format!(
                    "metadata descriptor has no matching payload body: {}",
                    descriptor.payload_id
                )));
            };

            if descriptor != &payload.descriptor {
                return Err(DbError::invalid_entry(format!(
                    "metadata descriptor does not match payload descriptor: {}",
                    descriptor.payload_id
                )));
            }
        }

        if self.metadata.entry_kind == CACHE_ENTRY_KIND_PAGE
            && self.metadata.primary_payload().is_none()
        {
            return Err(DbError::invalid_entry(
                "page cache entry must include a primary_snapshot or response_body payload",
            ));
        }

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

fn validate_payload_descriptor(descriptor: &CachePayloadDescriptor) -> DbResult<()> {
    if descriptor.payload_id.trim().is_empty() {
        return Err(DbError::invalid_entry("payload_id must not be empty"));
    }

    if descriptor.sha256_hex.len() != 64
        || !descriptor
            .sha256_hex
            .chars()
            .all(|ch| ch.is_ascii_hexdigit())
    {
        return Err(DbError::invalid_entry(format!(
            "payload {} has invalid sha256 hex digest",
            descriptor.payload_id
        )));
    }

    Ok(())
}

