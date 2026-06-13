//! Cache entry listing.
//!
//! This module owns large entry-list queries. It intentionally uses raw SQL
//! because the operator view needs aggregate facts such as payload byte length,
//! tag count, filtering, sorting, and pagination without loading payload bodies.
//!
//! Important detail:
//!
//! The cache currently stores one primary payload per entry. Because tag joins
//! duplicate the payload row, aggregate payload size must use `MAX(p.byte_len)`,
//! not `SUM(p.byte_len)`.

use clap::{
    Args,
    ValueEnum,
};
use serde_json::Value;
use sqlx::{
    postgres::PgRow,
    Row,
};

use super::common::{
    format_bytes,
    format_status,
    parse_tag,
    print_result_window,
    CacheHandle,
    EntryFilterArgs,
    PageArgs,
    SortDirection,
};

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntrySort {
    StoredAt,
    Url,
    FinalUrl,
    Host,
    Status,
    Bytes,
    Tags,
    Kind,
}

impl EntrySort {
    fn as_str(self) -> &'static str {
        match self {
            Self::StoredAt => "stored-at",
            Self::Url => "url",
            Self::FinalUrl => "final-url",
            Self::Host => "host",
            Self::Status => "status",
            Self::Bytes => "bytes",
            Self::Tags => "tags",
            Self::Kind => "kind",
        }
    }
}

#[derive(Args, Debug, Clone)]
pub(crate) struct EntrySortArgs {
    /// Sort field.
    #[arg(long, value_enum, default_value_t = EntrySort::StoredAt)]
    pub sort: EntrySort,

    /// Sort direction.
    #[arg(long, value_enum, default_value_t = SortDirection::Desc)]
    pub order: SortDirection,
}

pub(crate) async fn list_entries(
    cache: &CacheHandle,
    filters: EntryFilterArgs,
    page: PageArgs,
    sort: EntrySortArgs,
    json: bool,
) -> anyhow::Result<()> {
    filters.validate_time_bounds()?;

    let status = filters.checked_status_i32()?;
    let exact_tag = match filters.tag.as_deref() {
        Some(value) => Some(parse_tag(value)?),
        None => None,
    };

    let exact_tag_kind = exact_tag.as_ref().map(|tag| tag.kind().to_string());
    let exact_tag_key = exact_tag.as_ref().map(|tag| tag.key().to_string());

    let limit = page.checked_limit_i64()?;
    let offset = page.checked_offset_i64()?;
    let order_clause = entry_order_clause(sort.sort, sort.order);

    let sql = format!(
        r#"
        WITH filtered AS (
            SELECT
                e.key_digest,
                e.key_json,
                e.requested_url,
                e.requested_host,
                e.final_url,
                e.final_host,
                e.stored_at_unix_ms,
                e.entry_kind,
                e.metadata_version,
                e.status_code,
                e.content_type,
                COALESCE(MAX(p.byte_len), 0)::BIGINT AS payload_bytes,
                COUNT(DISTINCT CASE
                    WHEN et.tag_kind IS NULL THEN NULL
                    ELSE et.tag_kind || E'\x1F' || et.tag_key
                END)::BIGINT AS tag_count
            FROM cache_entries e
            LEFT JOIN cache_payloads p
                ON p.key_digest = e.key_digest
            LEFT JOIN cache_entry_tags et
                ON et.key_digest = e.key_digest
            WHERE
                ($1::TEXT IS NULL OR e.entry_kind = $1)
                AND ($2::TEXT IS NULL OR e.requested_host = $2 OR e.final_host = $2)
                AND ($3::TEXT IS NULL OR e.requested_host = $3)
                AND ($4::TEXT IS NULL OR e.final_host = $4)
                AND ($5::INTEGER IS NULL OR e.status_code = $5)
                AND ($6::TEXT IS NULL OR e.content_type ILIKE '%' || $6 || '%')
                AND (
                    $7::TEXT IS NULL
                    OR e.requested_url ILIKE '%' || $7 || '%'
                    OR COALESCE(e.final_url, '') ILIKE '%' || $7 || '%'
                )
                AND ($8::BIGINT IS NULL OR e.stored_at_unix_ms >= $8)
                AND ($9::BIGINT IS NULL OR e.stored_at_unix_ms < $9)
                AND (
                    $10::TEXT IS NULL
                    OR EXISTS (
                        SELECT 1
                        FROM cache_entry_tags exact_tag
                        WHERE exact_tag.key_digest = e.key_digest
                          AND exact_tag.tag_kind = $10
                          AND exact_tag.tag_key = $11
                    )
                )
                AND (
                    $12::TEXT IS NULL
                    OR EXISTS (
                        SELECT 1
                        FROM cache_entry_tags kind_tag
                        WHERE kind_tag.key_digest = e.key_digest
                          AND kind_tag.tag_kind = $12
                    )
                )
            GROUP BY
                e.key_digest,
                e.key_json,
                e.requested_url,
                e.requested_host,
                e.final_url,
                e.final_host,
                e.stored_at_unix_ms,
                e.entry_kind,
                e.metadata_version,
                e.status_code,
                e.content_type
        )
        SELECT
            *,
            COUNT(*) OVER()::BIGINT AS total_count
        FROM filtered
        ORDER BY {order_clause}, key_digest ASC
        LIMIT $13 OFFSET $14
        "#,
    );

    let rows = sqlx::query(&sql)
        .bind(filters.entry_kind.as_deref())
        .bind(filters.host.as_deref())
        .bind(filters.requested_host.as_deref())
        .bind(filters.final_host.as_deref())
        .bind(status)
        .bind(filters.content_type.as_deref())
        .bind(filters.url_contains.as_deref())
        .bind(filters.since_stored_at_unix_ms)
        .bind(filters.before_stored_at_unix_ms)
        .bind(exact_tag_kind.as_deref())
        .bind(exact_tag_key.as_deref())
        .bind(filters.tag_kind.as_deref())
        .bind(limit)
        .bind(offset)
        .fetch_all(cache.pool())
        .await?;

    let total = total_count(&rows)?;

    if json {
        let entries = rows
            .iter()
            .map(entry_row_json)
            .collect::<anyhow::Result<Vec<_>>>()?;

        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "limit": page.limit,
                "offset": page.offset,
                "returned": entries.len(),
                "total": total,
                "sort": sort.sort.as_str(),
                "order": sort.order.as_str(),
                "entries": entries,
            }))?
        );
    } else if rows.is_empty() {
        eprintln!("No cache entries found.");
    } else {
        print_entry_rows(&rows)?;
        print_result_window(rows.len(), total, &page);
    }

    Ok(())
}

fn entry_order_clause(sort: EntrySort, direction: SortDirection) -> String {
    let direction = direction.sql();

    match sort {
        EntrySort::StoredAt => format!("stored_at_unix_ms {direction}"),
        EntrySort::Url => format!("requested_url {direction}"),
        EntrySort::FinalUrl => format!("final_url {direction} NULLS LAST"),
        EntrySort::Host => format!("requested_host {direction} NULLS LAST"),
        EntrySort::Status => format!("status_code {direction} NULLS LAST"),
        EntrySort::Bytes => format!("payload_bytes {direction}"),
        EntrySort::Tags => format!("tag_count {direction}"),
        EntrySort::Kind => format!("entry_kind {direction}"),
    }
}

fn total_count(rows: &[PgRow]) -> anyhow::Result<i64> {
    match rows.first() {
        Some(row) => Ok(row.try_get("total_count")?),
        None => Ok(0),
    }
}

fn entry_row_json(row: &PgRow) -> anyhow::Result<Value> {
    let key: Value = row.try_get("key_json")?;
    let metadata_version: i32 = row.try_get("metadata_version")?;
    let status_code: Option<i32> = row.try_get("status_code")?;

    Ok(serde_json::json!({
        "key_digest": row.try_get::<String, _>("key_digest")?,
        "key": key,
        "requested_url": row.try_get::<String, _>("requested_url")?,
        "requested_host": row.try_get::<Option<String>, _>("requested_host")?,
        "final_url": row.try_get::<Option<String>, _>("final_url")?,
        "final_host": row.try_get::<Option<String>, _>("final_host")?,
        "stored_at_unix_ms": row.try_get::<i64, _>("stored_at_unix_ms")?,
        "entry_kind": row.try_get::<String, _>("entry_kind")?,
        "metadata_version": metadata_version,
        "status_code": status_code,
        "content_type": row.try_get::<Option<String>, _>("content_type")?,
        "payload_bytes": row.try_get::<i64, _>("payload_bytes")?,
        "tag_count": row.try_get::<i64, _>("tag_count")?,
    }))
}

fn print_entry_rows(rows: &[PgRow]) -> anyhow::Result<()> {
    println!(
        "{:<14} {:>11} {:>6} {:>5} {:<10} {}",
        "stored_at_ms",
        "bytes",
        "status",
        "tags",
        "kind",
        "url"
    );

    for row in rows {
        let stored_at_unix_ms: i64 = row.try_get("stored_at_unix_ms")?;
        let payload_bytes: i64 = row.try_get("payload_bytes")?;
        let status_code: Option<i32> = row.try_get("status_code")?;
        let tag_count: i64 = row.try_get("tag_count")?;
        let entry_kind: String = row.try_get("entry_kind")?;
        let requested_url: String = row.try_get("requested_url")?;

        println!(
            "{:<14} {:>11} {:>6} {:>5} {:<10} {}",
            stored_at_unix_ms,
            format_bytes(payload_bytes),
            format_status(status_code.map(|value| value as u16)),
            tag_count,
            entry_kind,
            requested_url
        );
    }

    Ok(())
}

