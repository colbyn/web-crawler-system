//! URL identity and normalization helpers.
//!
//! URL handling is a separate module because cache lookup, frontier dedupe,
//! scope policy, and downstream grouping all need slightly different URL
//! concepts.
//!
//! Important distinction:
//!
//! - Requested URL: what the crawler/browser was asked to open.
//! - Final URL: where the browser landed after redirects/navigation.
//! - Canonical URL: what the page claims as canonical.
//! - Normalized URL: an engine-derived identity used for dedupe/scheduling.
//!
//! This module should avoid pretending one identity can serve every purpose.
//! In particular, cache lookup should not blindly use final URL or canonical
//! URL, because doing so can erase provenance or allow bad content to poison
//! future lookups.

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NormalizedUrl(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UrlIdentity {
    pub normalized: NormalizedUrl,
}

#[derive(Debug, Clone, Default)]
pub struct UrlNormalizer;

impl UrlNormalizer {
    pub fn normalize_for_frontier(url: &Url) -> UrlIdentity {
        let mut normalized = url.clone();

        normalized.set_fragment(None);

        UrlIdentity {
            normalized: NormalizedUrl(normalized.to_string()),
        }
    }

    pub fn normalize_for_cache_request(url: &Url) -> UrlIdentity {
        let mut normalized = url.clone();

        normalized.set_fragment(None);

        UrlIdentity {
            normalized: NormalizedUrl(normalized.to_string()),
        }
    }
}

