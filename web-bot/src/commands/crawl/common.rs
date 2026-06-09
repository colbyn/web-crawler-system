//! Shared crawl command vocabulary.
//!
//! This module owns concepts used by both the command-line UI and the TOML
//! settings format.
//!
//! The crawl command intentionally exposes one vocabulary in two shapes:
//!
//! - TOML uses nested sections, for example `[budget] pages = 10`.
//! - CLI uses flattened flags, for example `--pages 10`.
//!
//! Keep shared user-facing concepts here so the CLI and config format do not
//! drift into separate dialects.

use clap::ValueEnum;
use serde::Deserialize;
use serde_json::Value;
use web_crawler_engine_v3::sqlite_cache::CacheTag;

/// Input format accepted by the crawl command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum CrawlInputFormat {
    /// Plain text URLs, split by line and whitespace.
    Text,

    /// Newline-delimited JSON, one JSON object per input line.
    Ndjson,

    /// JSON input.
    ///
    /// The current command parser treats this like line-oriented JSON for
    /// compatibility with earlier behavior. Full JSON-array ingestion can be
    /// added later without changing the public enum.
    Json,
}

impl Default for CrawlInputFormat {
    fn default() -> Self {
        Self::Text
    }
}

/// Output format produced by the crawl command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum CrawlOutputFormat {
    /// Human-readable progress and summary output.
    Human,

    /// One JSON crawl page result per stdout line.
    Ndjson,
}

impl Default for CrawlOutputFormat {
    fn default() -> Self {
        Self::Human
    }
}

/// Browser profile assignment strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum CrawlProfileStrategy {
    /// Every request uses the configured fallback browser profile.
    Single,

    /// Use caller-provided profile keys when available, otherwise fallback.
    CallerProvidedOrSingle,

    /// Derive the browser profile from the requested URL host.
    ByHost,

    /// Derive the browser profile from the original seed URL host.
    BySeedHost,
}

impl Default for CrawlProfileStrategy {
    fn default() -> Self {
        Self::BySeedHost
    }
}

/// A tag extracted from a JSON input row.
#[derive(Debug, Clone)]
pub struct TagPointer {
    pub kind: String,
    pub pointer: String,
}

/// Parse a single `kind:key` cache tag.
pub fn parse_tag(value: &str) -> anyhow::Result<CacheTag> {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        anyhow::bail!("tag cannot be empty");
    }

    if !trimmed.contains(':') {
        anyhow::bail!(
            "tag `{}` must use kind:key format, e.g. entity:business-123",
            trimmed
        );
    }

    Ok(CacheTag::from_compound(trimmed))
}

/// Parse many `kind:key` cache tags.
pub fn parse_tags(values: &[String]) -> anyhow::Result<Vec<CacheTag>> {
    values.iter().map(|value| parse_tag(value)).collect()
}

/// Parse a `kind=/json/pointer` tag-pointer specification.
pub fn parse_tag_pointer(value: &str) -> anyhow::Result<TagPointer> {
    let Some((kind, pointer)) = value.split_once('=') else {
        anyhow::bail!(
            "tag pointer `{}` must use kind=/json/pointer format",
            value
        );
    };

    let kind = kind.trim();
    let pointer = pointer.trim();

    if kind.is_empty() {
        anyhow::bail!("tag pointer kind cannot be empty");
    }

    if !pointer.starts_with('/') {
        anyhow::bail!(
            "tag pointer `{}` must use a JSON pointer beginning with `/`",
            value
        );
    }

    Ok(TagPointer {
        kind: kind.to_string(),
        pointer: pointer.to_string(),
    })
}

/// Parse many `kind=/json/pointer` tag-pointer specifications.
pub fn parse_tag_pointers(values: &[String]) -> anyhow::Result<Vec<TagPointer>> {
    values
        .iter()
        .map(|value| parse_tag_pointer(value))
        .collect()
}

/// Convert a JSON scalar into a cache tag key.
pub fn scalar_tag_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let value = value.trim();

            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }

        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),

        _ => None,
    }
}

/// Extract cache tags from a JSON input row using configured tag pointers.
///
/// A pointer may resolve to a scalar or an array of scalars. Objects and nulls
/// are ignored because they do not produce stable textual tag keys.
pub fn tags_from_json_pointers(
    json: &Value,
    tag_pointers: &[TagPointer],
) -> anyhow::Result<Vec<CacheTag>> {
    let mut tags = Vec::new();

    for spec in tag_pointers {
        let Some(value) = json.pointer(&spec.pointer) else {
            continue;
        };

        match value {
            Value::Array(values) => {
                for item in values {
                    if let Some(key) = scalar_tag_value(item) {
                        tags.push(CacheTag::new(&spec.kind, key));
                    }
                }
            }

            other => {
                if let Some(key) = scalar_tag_value(other) {
                    tags.push(CacheTag::new(&spec.kind, key));
                }
            }
        }
    }

    Ok(tags)
}

