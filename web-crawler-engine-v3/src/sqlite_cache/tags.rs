//! Cache tags — flexible secondary indexing and application integration point.
//!
//! Tags are **not** part of cache identity (`CacheKey`). They exist as a
//! many-to-many secondary index to support:
//!
//! - Grouping related crawl sessions or experiments
//! - Linking crawler results to higher-level application concepts
//!   (entities, jobs, batches, provenance, etc.)
//! - Bulk operations (`delete_entries_by_tag`, `remove_tag_from_all`)
//! - Discovery via `list_by_tag`
//!
//! `normalize_tag()` produces clean, lowercase identifiers while preserving
//! a small set of useful punctuation characters.

use serde::{Deserialize, Serialize};

use crate::sqlite_cache::CacheKey;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CacheTag {
    value: String,
}

impl CacheTag {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: normalize_tag(value.into()),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn into_string(self) -> String {
        self.value
    }
}

/// Normalize a raw tag into a clean, lowercase form suitable for storage and lookup.
pub fn normalize_tag(raw: String) -> String {
    let normalized = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' || ch == ':' {
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

