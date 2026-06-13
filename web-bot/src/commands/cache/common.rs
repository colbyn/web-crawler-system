//! Shared support code for cache subcommands.
//!
//! This module owns CLI argument fragments, lightweight parsing helpers,
//! operator-facing formatting, and safety helpers shared by the cache command
//! implementation.
//!
//! Keep SQL query construction out of this file. Query-heavy behavior belongs
//! in the specific subcommand module so each command remains easy to audit.

use anyhow::bail;
use clap::{
    Args,
    ValueEnum,
};
use std::io::{
    self,
    Write,
};
use url::Url;

use web_crawler_db::{
    CacheEntry,
    CacheKey,
    CachePayload,
    CacheTag,
    PostgresCache,
};

pub(crate) type CacheHandle = PostgresCache;

/// Shared pagination arguments for list-style commands.
#[derive(Args, Debug, Clone)]
pub(crate) struct PageArgs {
    /// Maximum rows to return.
    #[arg(long, default_value_t = 100)]
    pub limit: u32,

    /// Rows to skip before returning results.
    #[arg(long, default_value_t = 0)]
    pub offset: u32,
}

impl PageArgs {
    pub(crate) fn checked_limit_i64(&self) -> anyhow::Result<i64> {
        if self.limit == 0 {
            bail!("--limit must be greater than zero");
        }

        if self.limit > 10_000 {
            bail!("--limit is capped at 10000 to avoid accidental terminal avalanches");
        }

        Ok(i64::from(self.limit))
    }

    pub(crate) fn checked_offset_i64(&self) -> anyhow::Result<i64> {
        Ok(i64::from(self.offset))
    }
}

/// Shared sort direction for list-style commands.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub(crate) fn sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

/// Filters for entry-list queries.
#[derive(Args, Debug, Clone, Default)]
pub(crate) struct EntryFilterArgs {
    /// Exact tag in `kind:key` form.
    #[arg(long)]
    pub tag: Option<String>,

    /// Entries carrying any tag of this kind.
    #[arg(long = "tag-kind")]
    pub tag_kind: Option<String>,

    /// Match either requested host or final host exactly.
    #[arg(long)]
    pub host: Option<String>,

    /// Match requested host exactly.
    #[arg(long = "requested-host")]
    pub requested_host: Option<String>,

    /// Match final host exactly.
    #[arg(long = "final-host")]
    pub final_host: Option<String>,

    /// Match entry kind exactly.
    #[arg(long = "kind")]
    pub entry_kind: Option<String>,

    /// Match HTTP status code exactly.
    #[arg(long)]
    pub status: Option<u16>,

    /// Case-insensitive substring match against content type.
    #[arg(long = "content-type")]
    pub content_type: Option<String>,

    /// Case-insensitive substring match against requested or final URL.
    #[arg(long = "url-contains")]
    pub url_contains: Option<String>,

    /// Include entries stored at or after this Unix millisecond timestamp.
    #[arg(long = "since-ms")]
    pub since_stored_at_unix_ms: Option<i64>,

    /// Include entries stored before this Unix millisecond timestamp.
    #[arg(long = "before-ms")]
    pub before_stored_at_unix_ms: Option<i64>,
}

impl EntryFilterArgs {
    pub(crate) fn checked_status_i32(&self) -> anyhow::Result<Option<i32>> {
        let Some(status) = self.status else {
            return Ok(None);
        };

        if !(100..=599).contains(&status) {
            bail!("--status must be between 100 and 599");
        }

        Ok(Some(i32::from(status)))
    }

    pub(crate) fn validate_time_bounds(&self) -> anyhow::Result<()> {
        if let Some(value) = self.since_stored_at_unix_ms {
            if value < 0 {
                bail!("--since-ms must not be negative");
            }
        }

        if let Some(value) = self.before_stored_at_unix_ms {
            if value < 0 {
                bail!("--before-ms must not be negative");
            }
        }

        if let (Some(since), Some(before)) = (
            self.since_stored_at_unix_ms,
            self.before_stored_at_unix_ms,
        ) {
            if before <= since {
                bail!("--before-ms must be greater than --since-ms");
            }
        }

        Ok(())
    }
}

/// Filters for tag-list queries.
#[derive(Args, Debug, Clone, Default)]
pub(crate) struct TagFilterArgs {
    /// Match tag kind exactly.
    #[arg(long)]
    pub kind: Option<String>,

    /// Case-insensitive substring match against tag key.
    #[arg(long = "key-contains")]
    pub key_contains: Option<String>,
}

pub(crate) fn key_for_url(url: &str, namespace: Option<String>) -> anyhow::Result<CacheKey> {
    if namespace.is_some() {
        eprintln!("warning: cache namespace is ignored by the current Postgres cache key model");
    }

    Ok(CacheKey::for_url(Url::parse(url)?))
}

pub(crate) fn parse_tag(value: &str) -> anyhow::Result<CacheTag> {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        bail!("tag cannot be empty");
    }

    let Some((kind, key)) = trimmed.split_once(':') else {
        bail!("tag `{trimmed}` must use kind:key format, e.g. entity:business-123");
    };

    if kind.trim().is_empty() {
        bail!("tag `{trimmed}` has an empty kind");
    }

    if key.trim().is_empty() {
        bail!("tag `{trimmed}` has an empty key");
    }

    Ok(CacheTag::from_compound(trimmed))
}

pub(crate) fn parse_tags(values: Vec<String>) -> anyhow::Result<Vec<CacheTag>> {
    values.iter().map(|value| parse_tag(value)).collect()
}

/// Returns the single primary payload for a cache entry.
pub(crate) fn primary_payload(entry: &CacheEntry) -> &CachePayload {
    &entry.payload
}

pub(crate) fn format_status(status: Option<u16>) -> String {
    status
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

pub(crate) fn format_bytes(byte_len: i64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;

    if byte_len >= 1_048_576 {
        format!("{:.2} MiB", byte_len as f64 / MIB)
    } else if byte_len >= 1024 {
        format!("{:.2} KiB", byte_len as f64 / KIB)
    } else {
        format!("{byte_len} B")
    }
}

pub(crate) fn prompt_confirm(message: &str, force: bool) -> anyhow::Result<bool> {
    if force {
        return Ok(true);
    }

    eprint!("{message} [y/N]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input.trim().eq_ignore_ascii_case("y"))
}

pub(crate) fn print_result_window(returned: usize, total: i64, page: &PageArgs) {
    if total > returned as i64 || page.offset > 0 {
        eprintln!(
            "Showing {} of {} matching row(s), limit={}, offset={}.",
            returned,
            total,
            page.limit,
            page.offset
        );
    }
}

pub(crate) fn redact_database_url(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return "<database-url>".to_string();
    };

    if !url.username().is_empty() {
        let _ = url.set_username("redacted");
    }

    if url.password().is_some() {
        let _ = url.set_password(Some("redacted"));
    }

    url.to_string()
}
