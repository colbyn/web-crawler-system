//! Cache tags for secondary indexing and caller/application association.
//!
//! Tags are intentionally small:
//!
//! ```text
//! (kind, key)
//! ```
//!
//! A tag is a queryable association between a cache entry and some stable
//! bucket, external identifier, classification, or workflow label.
//!
//! Examples:
//!
//! ```text
//! entity.place_id : ChIJ...
//! category.id     : software_company
//! crawl.batch     : 2026-06-11-utah
//! source.dataset  : google_places
//! host            : example.com
//! http.status     : 200
//! ```
//!
//! ## What tags are
//!
//! Tags are:
//!
//! - secondary indexes,
//! - many-to-many associations,
//! - a lightweight way to connect cached artifacts to entities, categories,
//!   batches, sources, campaigns, or other caller-owned concepts.
//!
//! ## What tags are not
//!
//! Tags are not:
//!
//! - cache identity,
//! - arbitrary metadata,
//! - payload-specific facts,
//! - a structured fact system,
//! - a replacement for `extracted_json`, `telemetry_json`, or auxiliary storage.
//!
//! The canonical identity of a tag is always `(kind, key)`. The compound
//! `kind:key` string is only for display, logging, and legacy input parsing.
//! Database schemas should not rely on the compound string as a unique key.
//!
//! New tags created through [`CacheTag::new`] are normalized. Tags loaded from
//! storage may use [`CacheTag::raw`] so historical values can round-trip without
//! being rewritten.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// A normalized secondary-index association for a cache entry.
///
/// `kind` names the tag namespace. `key` names the value within that namespace.
/// Together they form the canonical tag identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheTag {
    kind: String,
    key: String,
}

impl CacheTag {
    /// Create a normalized structured cache tag.
    ///
    /// This is the preferred constructor for application code.
    pub fn new(kind: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            kind: normalize_tag_part(kind.into()),
            key: normalize_tag_part(key.into()),
        }
    }

    /// Create a tag from already-normalized or database-loaded parts.
    ///
    /// This intentionally avoids normalization so stored historic values can
    /// round-trip exactly. Prefer [`CacheTag::new`] for new application tags.
    pub fn raw(kind: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            key: key.into(),
        }
    }

    /// Transitional helper for old flat tag strings.
    ///
    /// If `value` contains a colon, the first colon separates `kind` and `key`.
    /// If no colon is present, the tag is stored under the generic `tag` kind.
    ///
    /// This method is intended for legacy input compatibility. The compound
    /// representation is not the canonical tag identity.
    pub fn from_compound(value: impl Into<String>) -> Self {
        let value = value.into().trim().to_string();

        if let Some((kind, key)) = value.split_once(':') {
            Self::new(kind, key)
        } else {
            Self::new("tag", value)
        }
    }

    /// Return the tag kind.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Return the tag key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Return the canonical `(kind, key)` pair as borrowed parts.
    pub fn as_parts(&self) -> (&str, &str) {
        (&self.kind, &self.key)
    }

    /// Return a display-oriented `kind:key` representation.
    ///
    /// This is useful for logging, CLI output, and legacy interop. It should not
    /// be used as the canonical identity in storage.
    pub fn as_compound(&self) -> String {
        format!("{}:{}", self.kind, self.key)
    }

    /// Legacy display helper.
    ///
    /// New code should prefer [`CacheTag::as_compound`] when a display string is
    /// explicitly needed.
    pub fn to_string_tag(&self) -> String {
        self.as_compound()
    }

    /// Consume the tag into owned `(kind, key)` parts.
    pub fn into_parts(self) -> (String, String) {
        (self.kind, self.key)
    }
}

impl fmt::Display for CacheTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_compound())
    }
}

impl FromStr for CacheTag {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_compound(value))
    }
}

impl From<(String, String)> for CacheTag {
    fn from((kind, key): (String, String)) -> Self {
        Self::new(kind, key)
    }
}

impl From<(&str, &str)> for CacheTag {
    fn from((kind, key): (&str, &str)) -> Self {
        Self::new(kind, key)
    }
}

/// Normalize one tag component into a clean lowercase storage key.
///
/// The normalization is deliberately conservative:
///
/// - ASCII alphanumeric characters are preserved,
/// - `-`, `_`, `.`, and `/` are preserved,
/// - all other characters become `-`,
/// - repeated/empty dash segments are collapsed,
/// - empty normalized values become `untagged`.
///
/// Colons are not preserved in newly-created tags. This keeps the display-only
/// `kind:key` representation easier to read and avoids ambiguity for legacy
/// compound parsing.
pub(crate) fn normalize_tag_part(raw: String) -> String {
    let normalized = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric()
                || ch == '-'
                || ch == '_'
                || ch == '.'
                || ch == '/'
            {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if normalized.is_empty() {
        "untagged".to_string()
    } else {
        normalized
    }
}

