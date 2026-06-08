//! Cache key construction.
//!
//! Cache keys define lookup identity for cached crawl artifacts.
//!
//! The cache key is intentionally based on the request side of the crawl:
//!
//! - requested URL,
//! - browser profile key,
//! - cache namespace,
//! - key/schema versions.
//!
//! It should not be based primarily on final URL or canonical URL. Those are
//! browser/page observations stored inside the artifact. Using them as primary
//! lookup keys can erase provenance or cause bad artifacts to poison unrelated
//! requests.
//!
//! The CLI helper and crawler engine must both construct keys through this
//! module. Do not duplicate key-building logic in binaries.

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;
use web_browser_driver::BrowserProfileKey;

pub const CACHE_KEY_SCHEMA_VERSION: u32 = 1;
pub const CACHE_KEY_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheKey {
    pub schema_version: u32,
    pub key_version: u32,
    pub requested_url: Url,
    pub profile_key: BrowserProfileKey,
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
}

