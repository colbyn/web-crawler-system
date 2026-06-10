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
//!
//! ## Input formats
//!
//! The crawl command supports three practical input lanes:
//!
//! - `text` is the Unix-friendly default: plain URL lists from files, flags, or
//!   stdin.
//! - `ndjson` is the adapter lane for foreign newline-delimited JSON. It uses a
//!   URL JSON pointer plus optional tag JSON pointers.
//! - `seed-bundle` is the WebBot-native structured JSON lane. Each input line is
//!   one seed bundle shaped like:
//!
//!   ```json
//!   {"urls":["https://example.com"],"tags":[{"kind":"run.id","key":"batch-1"}]}
//!   ```
//!
//! In the engine/cache API, tags are `kind + key` pairs. The SQLite cache may
//! also store a compound printable `tag` string internally, but that is not part
//! of the public input model. Structured JSON input therefore accepts exactly
//! `{ kind, key }` tag objects and no separate value field.

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
    ///
    /// This is the adapter format for foreign JSON feeds. Use `url-pointer` to
    /// select the crawl URL and `tag-pointer` entries to derive tags from JSON
    /// scalar fields.
    Ndjson,

    /// WebBot-native newline-delimited seed bundles.
    ///
    /// Each input line must be a JSON object with this shape:
    ///
    /// ```json
    /// {
    ///   "urls": ["https://example.com"],
    ///   "tags": [
    ///     { "kind": "run.id", "key": "batch-1" }
    ///   ]
    /// }
    /// ```
    ///
    /// Each URL expands into an independent crawl seed carrying the full tag
    /// set from the bundle plus any global tags configured through CLI/TOML.
    SeedBundle,

    /// JSON input.
    ///
    /// Reserved for full-document JSON ingestion. The current command should not
    /// silently treat this as either pointer-mapped NDJSON or seed-bundle NDJSON.
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

/// A tag extraction rule for generic pointer-mapped JSON input.
///
/// This is used by the `ndjson` adapter format, where the input JSON shape is
/// owned by some foreign producer and WebBot is told how to extract tags.
#[derive(Debug, Clone)]
pub struct TagPointer {
    pub kind: String,
    pub pointer: String,
}

/// One WebBot-native crawl seed bundle input object.
///
/// This is used by the `seed-bundle` input format. Each URL becomes one crawl
/// seed, and every expanded seed receives the full tag set.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedBundle {
    pub urls: Vec<String>,

    #[serde(default)]
    pub tags: Vec<InputTag>,
}

/// A structured tag from WebBot-native JSON seed-bundle input.
///
/// This mirrors the public cache tag model: tags are `kind + key` pairs. There
/// is intentionally no `value` field here.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputTag {
    pub kind: String,
    pub key: String,
}

/// Parse a single `kind:key` cache tag.
///
/// This is used by CLI/TOML global tags, for example:
///
/// - `run:manual-debug`
/// - `category:electricians`
/// - `entity.ids.place_id:ChIJ...`
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

/// Validate a structured tag kind used by seed-bundle JSON input.
///
/// WebBot accepts a general dotted namespace instead of a fixed producer enum.
/// Producer-specific tools may use stricter schemas, but the crawler should only
/// require a stable, machine-readable namespace shape.
pub fn validate_tag_kind(kind: &str) -> anyhow::Result<()> {
    let kind = kind.trim();

    if kind.is_empty() {
        anyhow::bail!("tag kind cannot be empty");
    }

    for part in kind.split('.') {
        if part.is_empty() {
            anyhow::bail!("tag kind `{}` contains an empty namespace segment", kind);
        }

        let mut chars = part.chars();

        let Some(first) = chars.next() else {
            anyhow::bail!("tag kind `{}` contains an empty namespace segment", kind);
        };

        if !first.is_ascii_lowercase() {
            anyhow::bail!(
                "tag kind `{}` must use lowercase dotted namespace segments",
                kind
            );
        }

        if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_') {
            anyhow::bail!(
                "tag kind `{}` must contain only lowercase letters, digits, underscores, and dots",
                kind
            );
        }
    }

    Ok(())
}

/// Convert one structured seed-bundle JSON tag into a cache tag.
pub fn parse_input_tag(tag: InputTag) -> anyhow::Result<CacheTag> {
    let kind = tag.kind.trim();
    let key = tag.key.trim();

    validate_tag_kind(kind)?;

    if key.is_empty() {
        anyhow::bail!("tag key cannot be empty for kind `{}`", kind);
    }

    Ok(CacheTag::new(kind, key.to_string()))
}

/// Convert many structured seed-bundle JSON tags into cache tags.
pub fn parse_input_tags(tags: Vec<InputTag>) -> anyhow::Result<Vec<CacheTag>> {
    tags.into_iter().map(parse_input_tag).collect()
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

    validate_tag_kind(kind)?;

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
