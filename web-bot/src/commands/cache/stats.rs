//! Cache statistics.
//!
//! This module owns aggregate, read-only database facts for the Postgres cache.
//!
//! Keep this command cheap and boring:
//!
//! - no payload bodies,
//! - no wide metadata JSON reads,
//! - no per-entry scans beyond simple grouped counts,
//! - no destructive behavior.
//!
//! The stats command is meant for operational sanity checks during crawler runs:
//!
//! - how many entries exist,
//! - how much payload data is stored,
//! - how many tags and tag links exist,
//! - which entry kinds and status codes dominate the cache.

use sqlx::Row;

use super::common::{
    format_bytes,
    redact_database_url,
    CacheHandle,
};

pub(crate) async fn show_stats(
    cache: &CacheHandle,
    database_url: &str,
    json: bool,
) -> anyhow::Result<()> {
    let totals = read_totals(cache).await?;
    let by_entry_kind = read_entry_kind_counts(cache).await?;
    let by_status = read_status_counts(cache).await?;
    let top_tag_kinds = read_top_tag_kind_counts(cache).await?;

    let display_url = redact_database_url(database_url);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "database_url": display_url,
                "total_entries": totals.total_entries,
                "total_payloads": totals.total_payloads,
                "total_payload_bytes": totals.total_payload_bytes,
                "total_tags": totals.total_tags,
                "total_tag_links": totals.total_tag_links,
                "total_auxiliary_values": totals.total_auxiliary_values,
                "entry_kinds": by_entry_kind,
                "status_codes": by_status,
                "top_tag_kinds": top_tag_kinds,
            }))?
        );
    } else {
        eprintln!("Cache DB:          {}", display_url);
        eprintln!("Entries:           {}", totals.total_entries);
        eprintln!("Payloads:          {}", totals.total_payloads);
        eprintln!(
            "Payload bytes:     {} ({:.2} MiB)",
            totals.total_payload_bytes,
            totals.total_payload_bytes as f64 / 1_048_576.0
        );
        eprintln!("Tags:              {}", totals.total_tags);
        eprintln!("Tag links:         {}", totals.total_tag_links);
        eprintln!("Auxiliary values:  {}", totals.total_auxiliary_values);

        if !by_entry_kind.is_empty() {
            eprintln!();
            eprintln!("Entry kinds:");
            for row in &by_entry_kind {
                eprintln!("  {:<20} {}", row.entry_kind, row.entry_count);
            }
        }

        if !by_status.is_empty() {
            eprintln!();
            eprintln!("Status codes:");
            for row in &by_status {
                eprintln!(
                    "  {:<6} {:>8} entries  {:>11}",
                    row.status
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    row.entry_count,
                    format_bytes(row.payload_bytes)
                );
            }
        }

        if !top_tag_kinds.is_empty() {
            eprintln!();
            eprintln!("Top tag kinds:");
            for row in &top_tag_kinds {
                eprintln!(
                    "  {:<32} {:>8} tag(s)  {:>8} link(s)",
                    row.tag_kind,
                    row.tag_count,
                    row.link_count
                );
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct CacheTotals {
    total_entries: i64,
    total_payloads: i64,
    total_payload_bytes: i64,
    total_tags: i64,
    total_tag_links: i64,
    total_auxiliary_values: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct EntryKindCount {
    entry_kind: String,
    entry_count: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct StatusCount {
    status: Option<i32>,
    entry_count: i64,
    payload_bytes: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct TagKindCount {
    tag_kind: String,
    tag_count: i64,
    link_count: i64,
}

async fn read_totals(cache: &CacheHandle) -> anyhow::Result<CacheTotals> {
    let row = sqlx::query(
        r#"
        SELECT
            (SELECT COUNT(*) FROM cache_entries)::BIGINT AS total_entries,
            (SELECT COUNT(*) FROM cache_payloads)::BIGINT AS total_payloads,
            (SELECT COALESCE(SUM(byte_len), 0) FROM cache_payloads)::BIGINT
                AS total_payload_bytes,
            (SELECT COUNT(*) FROM cache_tags)::BIGINT AS total_tags,
            (SELECT COUNT(*) FROM cache_entry_tags)::BIGINT AS total_tag_links,
            (SELECT COUNT(*) FROM cache_auxiliary)::BIGINT
                AS total_auxiliary_values
        "#,
    )
    .fetch_one(cache.pool())
    .await?;

    Ok(CacheTotals {
        total_entries: row.try_get("total_entries")?,
        total_payloads: row.try_get("total_payloads")?,
        total_payload_bytes: row.try_get("total_payload_bytes")?,
        total_tags: row.try_get("total_tags")?,
        total_tag_links: row.try_get("total_tag_links")?,
        total_auxiliary_values: row.try_get("total_auxiliary_values")?,
    })
}

async fn read_entry_kind_counts(cache: &CacheHandle) -> anyhow::Result<Vec<EntryKindCount>> {
    let rows = sqlx::query(
        r#"
        SELECT
            entry_kind,
            COUNT(*)::BIGINT AS entry_count
        FROM cache_entries
        GROUP BY entry_kind
        ORDER BY entry_count DESC, entry_kind ASC
        "#,
    )
    .fetch_all(cache.pool())
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(EntryKindCount {
                entry_kind: row.try_get("entry_kind")?,
                entry_count: row.try_get("entry_count")?,
            })
        })
        .collect()
}

async fn read_status_counts(cache: &CacheHandle) -> anyhow::Result<Vec<StatusCount>> {
    let rows = sqlx::query(
        r#"
        SELECT
            e.status_code AS status,
            COUNT(*)::BIGINT AS entry_count,
            COALESCE(SUM(p.byte_len), 0)::BIGINT AS payload_bytes
        FROM cache_entries e
        LEFT JOIN cache_payloads p
            ON p.key_digest = e.key_digest
        GROUP BY e.status_code
        ORDER BY entry_count DESC, status ASC NULLS LAST
        "#,
    )
    .fetch_all(cache.pool())
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(StatusCount {
                status: row.try_get("status")?,
                entry_count: row.try_get("entry_count")?,
                payload_bytes: row.try_get("payload_bytes")?,
            })
        })
        .collect()
}

async fn read_top_tag_kind_counts(cache: &CacheHandle) -> anyhow::Result<Vec<TagKindCount>> {
    let rows = sqlx::query(
        r#"
        SELECT
            t.tag_kind,
            COUNT(*)::BIGINT AS tag_count,
            COUNT(et.key_digest)::BIGINT AS link_count
        FROM cache_tags t
        LEFT JOIN cache_entry_tags et
            ON et.tag_kind = t.tag_kind
           AND et.tag_key = t.tag_key
        GROUP BY t.tag_kind
        ORDER BY link_count DESC, tag_count DESC, t.tag_kind ASC
        LIMIT 25
        "#,
    )
    .fetch_all(cache.pool())
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(TagKindCount {
                tag_kind: row.try_get("tag_kind")?,
                tag_count: row.try_get("tag_count")?,
                link_count: row.try_get("link_count")?,
            })
        })
        .collect()
}
