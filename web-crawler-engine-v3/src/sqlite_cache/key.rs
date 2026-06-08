//! Cache key definition and stable digest computation.
//!
//! `CacheKey` represents the *logical identity* of a crawl request from the
//! engine's perspective. It includes the originally requested URL, browser
//! profile/partition, and an optional namespace.
//!
//! Importantly, it does **not** contain observed facts (final URL after
//! redirects, status code, content type, etc.). Those belong in
//! `CacheEntryMetadata` / `CacheResponseInfo`.
//!
//! The SHA-256 digest of the key is used as the primary key in the database,
//! providing stable, content-addressable identity.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::sqlite_cache::{CacheError, CacheResult};
use web_browser_driver::BrowserProfileKey;

pub const CACHE_KEY_SCHEMA_VERSION: u32 = 1;
pub const CACHE_KEY_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheKey {
    pub schema_version: u32,
    pub key_version: u32,
    /// The URL the crawler was asked to fetch (not the final URL after redirects).
    pub requested_url: Url,
    pub profile_key: BrowserProfileKey,
    /// Optional namespace for logical separation (e.g. "prod", "dev", "experiment-v3").
    pub namespace: Option<String>,
}

impl CacheKey {
    pub fn for_request(
        requested_url: Url,
        profile_key: BrowserProfileKey,
        namespace: Option<String>,
    ) -> Self {
        Self {
            schema_version: CACHE_KEY_SCHEMA_VERSION,
            key_version: CACHE_KEY_VERSION,
            requested_url,
            profile_key,
            namespace,
        }
    }

    pub fn requested_host(&self) -> Option<String> {
        self.requested_url.host_str().map(ToOwned::to_owned)
    }
}

/// Compute a stable SHA-256 digest of the key for use as primary key in SQLite.
pub fn cache_key_digest(key: &CacheKey) -> CacheResult<String> {
    let bytes = serde_json::to_vec(key)
        .map_err(|e| CacheError::KeySerialization(e.to_string()))?;
    let digest = Sha256::digest(&bytes);
    Ok(hex::encode(digest))
}
