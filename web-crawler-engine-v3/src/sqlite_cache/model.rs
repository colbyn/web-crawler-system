//! Core data models for the SQLite cache.
//!
//! This module defines the main in-memory and serializable types:
//!
//! - `CacheEntry` + `CacheEntryMetadata` — the full cached artifact
//! - `CachePayload` + `CachePayloadDescriptor` — binary artifacts (HTML, screenshots, etc.)
//! - `CacheEntryRef` — lightweight reference used by `list_by_tag`
//!
//! ## Key Design Decisions
//! - Metadata is versioned and contains flexible `*_json` fields so that
//!   extractors and downstream processors can evolve without cache schema changes.
//! - Binary payloads are stored separately from metadata for efficiency and clarity.
//! - `primary_payload()` provides a conventional way to get the "main" content
//!   of a page (PrimarySnapshot → ResponseBody fallback).
//!
//! All structs use `snake_case` JSON naming for easy inspection with `jq` and
//! SQLite's JSON functions.

use serde::{Deserialize, Serialize};

use crate::sqlite_cache::{CacheKey, CacheTag};

pub const CACHE_ENTRY_KIND_PAGE: &str = "page";
pub const CACHE_METADATA_VERSION: u32 = 1;

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

    /// Flexible JSON for driver/browser telemetry.
    #[serde(default)]
    pub telemetry_json: serde_json::Value,

    /// Flexible JSON for extracted facts (evolves independently of core schema).
    #[serde(default)]
    pub extracted_json: serde_json::Value,

    /// Non-fatal errors captured during crawl.
    #[serde(default)]
    pub non_critical_errors_json: Vec<serde_json::Value>,

    /// Descriptors for binary payloads stored in the separate `cache_payloads` table.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheProducerInfo {
    pub engine_name: String,
    pub engine_version: String,
    pub driver_version: Option<String>,
    pub cache_policy_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheRequestInfo {
    pub requested_url: String,
    pub requested_host: Option<String>,
    pub profile_key_json: serde_json::Value,
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheResponseInfo {
    pub final_url: Option<String>,
    pub final_host: Option<String>,
    pub status_code: Option<u16>,
    pub content_type: Option<String>,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachePayloadRole {
    PrimarySnapshot,
    ResponseBody,
    Screenshot,
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

/// Lightweight reference returned by `list_by_tag`. Does not contain payloads.
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

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
