//! Persisted cache artifact model.
//!
//! A cache artifact is a self-contained snapshot bundle. It should contain the
//! page payload and the metadata needed to evaluate whether the payload is still
//! trustworthy under current crawler policy.
//!
//! This file deliberately avoids sidecar metadata. A filesystem cache entry
//! should be one binary artifact containing:
//!
//! - cache key,
//! - producer/version information,
//! - URL resolution facts,
//! - browser telemetry,
//! - non-critical page errors,
//! - optional generic extracted facts,
//! - HTML or other snapshot bytes.
//!
//! Keeping the artifact self-contained makes cache repair simple. If decode,
//! schema, policy, or health checks fail, the engine can reject the artifact and
//! recrawl later.

use serde::{
    Deserialize,
    Serialize,
};
use web_browser_driver::{
    ExtractedAnchor,
    NonCriticalBrowserError,
    PageInfo,
    PageTelemetry,
    UrlResolution,
};

use crate::cache::CacheKey;

pub const CACHED_PAGE_ARTIFACT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CachedPageArtifact {
    pub artifact_version: u32,
    pub cache_key: CacheKey,
    pub stored_at_unix_ms: i64,
    pub producer: CacheProducerInfo,
    pub resolution: UrlResolution,
    pub status_code: Option<u16>,
    pub telemetry: PageTelemetry,
    pub non_critical_errors: Vec<NonCriticalBrowserError>,
    pub snapshot: CacheSnapshot,
    pub extracted: CachedExtractedFacts,
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
pub struct CacheSnapshot {
    pub captured_at_unix_ms: i64,
    pub content_type: Option<String>,

    /// Snapshot bytes as stored in the artifact.
    ///
    /// If `compression` is not `None`, these are compressed bytes.
    pub body: Vec<u8>,

    pub compression: SnapshotCompression,

    /// SHA-256 of the uncompressed body.
    pub body_sha256_hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotCompression {
    None,
    Gzip,
    Zstd,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CachedExtractedFacts {
    pub page_info: Option<PageInfo>,
    pub anchors: Vec<ExtractedAnchor>,
}

