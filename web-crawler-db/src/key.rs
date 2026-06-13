//! Cache key definition and stable digest computation.
//!
//! A [`CacheKey`] is the logical lookup identity for a cached page artifact.
//!
//! For this crate revision, the key is intentionally simple:
//!
//! ```text
//! requested URL -> stable cache digest
//! ```
//!
//! More elaborate partitioning concepts such as namespaces, tenants, crawl
//! tasks, run IDs, retry waves, or producer profiles do not belong in this type
//! yet. Those concerns can be added later through an explicit migration if the
//! artifact store truly needs them.
//!
//! Versioning is still present, but it is private to digest computation. The
//! public key model should describe the resource being looked up, not expose
//! storage-version machinery as ordinary identity fields.
//!
//! The digest is used as the compact primary key in the database. The original
//! key is still stored as JSON for inspection, debugging, and future migration.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use schemars::JsonSchema;

use crate::error::{DbError, DbResult};

/// Private version for the canonical digest input format.
///
/// This is intentionally not a public field on [`CacheKey`]. If digest semantics
/// change later, this constant can be bumped while migration code decides how to
/// handle old rows.
const CACHE_KEY_DIGEST_VERSION: u32 = 1;

/// Logical cache lookup key for a requested page URL.
///
/// This type deliberately avoids broader workflow concepts. In particular:
///
/// - no namespace,
/// - no crawl run ID,
/// - no retry/task state,
/// - no browser profile key,
/// - no public digest/schema version fields.
///
/// Those values may exist elsewhere as provenance, tags, or higher-level task
/// state, but they are not part of this first cache identity model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CacheKey {
    pub requested_url: Url,
}

impl CacheKey {
    /// Construct a cache key for a requested URL.
    pub fn for_url(requested_url: Url) -> Self {
        Self { requested_url }
    }

    /// Compatibility constructor for call sites that still speak in terms of
    /// crawl requests.
    ///
    /// New code should prefer [`CacheKey::for_url`].
    pub fn for_request(requested_url: Url) -> Self {
        Self::for_url(requested_url)
    }

    /// Return the requested URL host, if the URL has one.
    pub fn requested_host(&self) -> Option<String> {
        self.requested_url.host_str().map(ToOwned::to_owned)
    }

    /// Borrow the requested URL.
    pub fn requested_url(&self) -> &Url {
        &self.requested_url
    }
}

/// Compute a stable SHA-256 digest of the key for database storage.
///
/// The digest input is a small canonical JSON object that includes a private
/// digest-format version. Keeping that version outside [`CacheKey`] prevents
/// storage mechanics from leaking into the public logical identity model.
pub fn cache_key_digest(key: &CacheKey) -> DbResult<String> {
    let canonical = CacheKeyDigestInput {
        digest_version: CACHE_KEY_DIGEST_VERSION,
        requested_url: key.requested_url.as_str(),
    };

    let bytes = serde_json::to_vec(&canonical)
        .map_err(|e| DbError::KeySerialization(e.to_string()))?;

    let digest = Sha256::digest(&bytes);
    Ok(hex::encode(digest))
}

/// Canonical serialized shape used only for digest computation.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct CacheKeyDigestInput<'a> {
    digest_version: u32,
    requested_url: &'a str,
}

