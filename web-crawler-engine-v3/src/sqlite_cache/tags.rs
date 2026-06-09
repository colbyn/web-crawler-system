//! Cache tags — flexible secondary indexing and application integration point.
//!
//! Tags are **not** part of cache identity (`CacheKey`). They exist as a
//! many-to-many secondary index to support:
//!
//! - Grouping related crawl sessions or experiments
//! - Linking crawler results to higher-level application concepts
//!   (entities, jobs, batches, categories, provenance, etc.)
//! - Inheriting seed context across all discovered pages
//! - Bulk operations by exact tag or by tag kind
//! - Discovery via tag lookup
//!
//! A tag is intentionally structured as `(kind, key)` rather than a single
//! string. This lets callers ask both:
//!
//! ```text
//! all entries tagged entity:business-123
//! all entries tagged with any entity
//! all entries tagged with any category
//! ```
//!
//! The compound string form (`kind:key`) is still available for display,
//! logging, and transitional compatibility, but storage should prefer separate
//! `tag_kind` and `tag_key` columns.

use std::{
    fmt,
    str::FromStr,
};

use serde::{
    Deserialize,
    Serialize,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheTag {
    kind: String,
    key: String,
}

impl CacheTag {
    /// Create a normalized structured cache tag.
    ///
    /// Examples:
    ///
    /// ```ignore
    /// CacheTag::new("entity", "business-123")
    /// CacheTag::new("category", "electricians")
    /// CacheTag::new("run", "manual-debug")
    /// ```
    pub fn new(
        kind: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        Self {
            kind: normalize_tag_part(kind.into()),
            key: normalize_tag_part(key.into()),
        }
    }

    /// Create a tag from already-normalized or database-loaded parts.
    ///
    /// This intentionally avoids normalization so round-tripping from SQLite does
    /// not silently rewrite historic values.
    pub fn raw(
        kind: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            key: key.into(),
        }
    }

    /// Transitional helper for old flat tag strings.
    ///
    /// If the value contains `:`, the first segment becomes the kind and the rest
    /// becomes the key. Otherwise the tag is placed under the `tag` kind.
    ///
    /// Examples:
    ///
    /// ```text
    /// entity:business-123 -> kind=entity, key=business-123
    /// electricians        -> kind=tag,    key=electricians
    /// ```
    pub fn from_compound(value: impl Into<String>) -> Self {
        let value = value.into();
        let value = value.trim();

        if let Some((kind, key)) = value.split_once(':') {
            Self::new(kind, key)
        } else {
            Self::new("tag", value)
        }
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// Compound `kind:key` representation.
    ///
    /// Prefer `kind()` and `key()` for database storage/querying. Use this for
    /// display, logging, and compatibility with old call sites.
    pub fn as_compound(&self) -> String {
        format!("{}:{}", self.kind, self.key)
    }

    /// Compatibility alias for older code that expected a flat string tag.
    ///
    /// This allocates because the canonical representation is now structured.
    pub fn to_string_tag(&self) -> String {
        self.as_compound()
    }

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
/// This is intentionally boring:
///
/// - trim whitespace,
/// - lowercase ASCII,
/// - preserve useful separators,
/// - convert other characters to `-`,
/// - collapse repeated `-`.
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
                || ch == ':'
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
