//! Cache key definition and stable digest computation.
//!
//! A [`CacheKey`] is the stable logical identity of a cached crawl artifact.
//! It answers one narrow question:
//!
//! > “Should this crawl request reuse the same cached artifact?”
//!
//! Therefore the key intentionally contains only request-level identity:
//!
//! - cache key schema/version fields,
//! - the originally requested URL,
//! - an optional logical namespace.
//!
//! It intentionally does **not** contain runtime observations or implementation
//! details. In particular, it does not contain:
//!
//! - the final URL after redirects,
//! - status code,
//! - content type,
//! - browser telemetry,
//! - Chrome/browser profile IDs,
//! - worker/session IDs,
//! - tags,
//! - caller/entity provenance.
//!
//! Those belong in [`crate::sqlite_cache::CacheEntryMetadata`] and related
//! metadata structures.
//!
//! ## Why browser profile is not part of the key
//!
//! Browser profiles are an execution detail. Including a driver/browser profile
//! ID in the key fragments the performance cache by worker/profile assignment:
//! the same URL crawled by two different Chrome profiles would become two
//! unrelated cache entries.
//!
//! If the observable page genuinely varies by crawl context, represent that
//! later with an explicit semantic variation dimension such as a namespace or a
//! future `vary_key`:
//!
//! - `anonymous-us-ut`,
//! - `logged-out`,
//! - `mobile-en-us`,
//! - `customer-account-123`,
//! - `proxy-region-eu`.
//!
//! Do not use raw browser profile IDs for that purpose.
//!
//! ## Digest stability
//!
//! The SQLite primary key is a SHA-256 digest of the serialized [`CacheKey`].
//! Any intentional key-shape change should bump [`CACHE_KEY_VERSION`] so old
//! artifacts become safe cache misses instead of ambiguous partial matches.

use serde::{
    Deserialize,
    Serialize,
};
use sha2::{
    Digest,
    Sha256,
};
use url::Url;

use crate::sqlite_cache::{
    CacheError,
    CacheResult,
};

pub const CACHE_KEY_SCHEMA_VERSION: u32 = 1;

/// Bumped because browser/driver profile identity was removed from `CacheKey`.
///
/// Old digests were profile-scoped. New digests are shared across profiles for
/// the same requested URL + namespace.
pub const CACHE_KEY_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheKey {
    pub schema_version: u32,
    pub key_version: u32,

    /// The URL the crawler was asked to fetch.
    ///
    /// This is intentionally the requested URL, not the final URL after
    /// redirects. Redirects are runtime observations and belong in response
    /// metadata.
    pub requested_url: Url,

    /// Optional logical namespace for deliberate separation.
    ///
    /// Examples:
    ///
    /// - `prod`
    /// - `dev`
    /// - `experiment-v3`
    /// - `anonymous-us-ut`
    ///
    /// Use this when the caller intentionally wants separate cache universes.
    /// Do not use it for incidental worker/session/browser profile identity.
    pub namespace: Option<String>,
}

impl CacheKey {
    pub fn for_request(
        requested_url: Url,
        namespace: Option<String>,
    ) -> Self {
        Self {
            schema_version: CACHE_KEY_SCHEMA_VERSION,
            key_version: CACHE_KEY_VERSION,
            requested_url,
            namespace,
        }
    }

    pub fn requested_host(&self) -> Option<String> {
        self.requested_url.host_str().map(ToOwned::to_owned)
    }
}

/// Compute a stable SHA-256 digest of the key for use as the SQLite primary key.
///
/// The digest is based only on [`CacheKey`]. Runtime metadata, tags, payloads,
/// browser profile IDs, and extracted facts are deliberately excluded.
pub fn cache_key_digest(key: &CacheKey) -> CacheResult<String> {
    let bytes = serde_json::to_vec(key)
        .map_err(|e| CacheError::KeySerialization(e.to_string()))?;

    let digest = Sha256::digest(&bytes);

    Ok(hex::encode(digest))
}
