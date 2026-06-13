//! Cache tag listing.
//!
//! This module owns tag registry inspection and per-entry tag inspection.
//!
//! Tags are stored as two canonical columns:
//!
//! - `tag_kind`
//! - `tag_key`
//!
//! The compound operator-facing form `kind:key` is assembled at the CLI
//! boundary only. Do not query a synthetic `tag` column; the schema does not
//! have one.
//!
//! List-style tag commands support filtering, deterministic sorting, and
//! pagination because broad crawl runs can accumulate very large tag registries.

use clap::{
    Args,
    ValueEnum,
};
use serde_json::Value;
use sqlx::{
    postgres::PgRow,
    Row,
};

use web_crawler_db::cache_key_digest;

use super::common::{
    key_for_url,
    print_result_window,
    CacheHandle,
    PageArgs,
    SortDirection,
    TagFilterArgs,
};

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagSort {
    Kind,
    Key,
    Entries,
}

impl TagSort {
    fn as_str(self) -> &'static str {
        match self {
            Self::Kind => "kind",
            Self::Key => "key",
            Self::Entries => "entries",
        }
    }
}

#[derive(Args, Debug, Clone)]
pub(crate) struct TagSortArgs {
    /// Sort field.
    #[arg(long, value_enum, default_value_t = TagSort::Kind)]
    pub sort: TagSort,

    /// Sort direction.
    #[arg(long, value_enum, default_value_t = SortDirection::Asc)]
    pub order: SortDirection,
}

pub(crate) async fn list_tags(
    cache: &CacheHandle,
    url: Option<String>,
    namespace: Option<String>,
    filters: TagFilterArgs,
    page: PageArgs,
    sort: TagSortArgs,
    json: bool,
) -> anyhow::Result<()> {
    match url {
        Some(url) => list_tags_for_url(cache, &url, namespace, filters, page, sort, json).await,
        None => list_registry_tags(cache, filters, page, sort, json).await,
    }
}

async fn list_registry_tags(
    cache: &CacheHandle,
    filters: TagFilterArgs,
    page: PageArgs,
    sort: TagSortArgs,
    json: bool,
) -> anyhow::Result<()> {
    let limit = page.checked_limit_i64()?;
    let offset = page.checked_offset_i64()?;
    let order_clause = tag_order_clause(sort.sort, sort.order);

    let sql = format!(
        r#"
        WITH filtered AS (
            SELECT
                t.tag_kind,
                t.tag_key,
                COUNT(et.key_digest)::BIGINT AS entry_count
            FROM cache_tags t
            LEFT JOIN cache_entry_tags et
                ON et.tag_kind = t.tag_kind
               AND et.tag_key = t.tag_key
            WHERE
                ($1::TEXT IS NULL OR t.tag_kind = $1)
                AND ($2::TEXT IS NULL OR t.tag_key ILIKE '%' || $2 || '%')
            GROUP BY
                t.tag_kind,
                t.tag_key
        )
        SELECT
            tag_kind,
            tag_key,
            entry_count,
            COUNT(*) OVER()::BIGINT AS total_count
        FROM filtered
        ORDER BY {order_clause}, tag_kind ASC, tag_key ASC
        LIMIT $3 OFFSET $4
        "#,
    );

    let rows = sqlx::query(&sql)
        .bind(filters.kind.as_deref())
        .bind(filters.key_contains.as_deref())
        .bind(limit)
        .bind(offset)
        .fetch_all(cache.pool())
        .await?;

    print_tag_query_result(rows, page, sort, json, "No tags found.")
}

async fn list_tags_for_url(
    cache: &CacheHandle,
    url: &str,
    namespace: Option<String>,
    filters: TagFilterArgs,
    page: PageArgs,
    sort: TagSortArgs,
    json: bool,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;

    if cache.get_metadata(&cache_key).await?.is_none() {
        anyhow::bail!("URL not found in cache: {}", url);
    }

    let key_digest = cache_key_digest(&cache_key)?;
    let limit = page.checked_limit_i64()?;
    let offset = page.checked_offset_i64()?;
    let order_clause = tag_order_clause(sort.sort, sort.order);

    let sql = format!(
        r#"
        WITH url_tags AS (
            SELECT
                et.tag_kind,
                et.tag_key
            FROM cache_entry_tags et
            WHERE et.key_digest = $1
        ),
        filtered AS (
            SELECT
                ut.tag_kind,
                ut.tag_key,
                COUNT(all_et.key_digest)::BIGINT AS entry_count
            FROM url_tags ut
            LEFT JOIN cache_entry_tags all_et
                ON all_et.tag_kind = ut.tag_kind
               AND all_et.tag_key = ut.tag_key
            WHERE
                ($2::TEXT IS NULL OR ut.tag_kind = $2)
                AND ($3::TEXT IS NULL OR ut.tag_key ILIKE '%' || $3 || '%')
            GROUP BY
                ut.tag_kind,
                ut.tag_key
        )
        SELECT
            tag_kind,
            tag_key,
            entry_count,
            COUNT(*) OVER()::BIGINT AS total_count
        FROM filtered
        ORDER BY {order_clause}, tag_kind ASC, tag_key ASC
        LIMIT $4 OFFSET $5
        "#,
    );

    let rows = sqlx::query(&sql)
        .bind(&key_digest)
        .bind(filters.kind.as_deref())
        .bind(filters.key_contains.as_deref())
        .bind(limit)
        .bind(offset)
        .fetch_all(cache.pool())
        .await?;

    print_tag_query_result(
        rows,
        page,
        sort,
        json,
        &format!("No tags found for URL: {url}"),
    )
}

fn tag_order_clause(sort: TagSort, direction: SortDirection) -> String {
    let direction = direction.sql();

    match sort {
        TagSort::Kind => format!("tag_kind {direction}"),
        TagSort::Key => format!("tag_key {direction}"),
        TagSort::Entries => format!("entry_count {direction}"),
    }
}

fn print_tag_query_result(
    rows: Vec<PgRow>,
    page: PageArgs,
    sort: TagSortArgs,
    json: bool,
    empty_message: &str,
) -> anyhow::Result<()> {
    let total = total_count(&rows)?;

    if json {
        let tags = rows
            .iter()
            .map(tag_row_json)
            .collect::<anyhow::Result<Vec<_>>>()?;

        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "limit": page.limit,
                "offset": page.offset,
                "returned": tags.len(),
                "total": total,
                "sort": sort.sort.as_str(),
                "order": sort.order.as_str(),
                "tags": tags,
            }))?
        );
    } else if rows.is_empty() {
        eprintln!("{empty_message}");
    } else {
        print_tag_rows(&rows)?;
        print_result_window(rows.len(), total, &page);
    }

    Ok(())
}

fn total_count(rows: &[PgRow]) -> anyhow::Result<i64> {
    match rows.first() {
        Some(row) => Ok(row.try_get("total_count")?),
        None => Ok(0),
    }
}

fn tag_row_json(row: &PgRow) -> anyhow::Result<Value> {
    let kind: String = row.try_get("tag_kind")?;
    let key: String = row.try_get("tag_key")?;
    let entry_count: i64 = row.try_get("entry_count")?;

    Ok(serde_json::json!({
        "kind": kind,
        "key": key,
        "tag": format!("{kind}:{key}"),
        "entry_count": entry_count,
    }))
}

fn print_tag_rows(rows: &[PgRow]) -> anyhow::Result<()> {
    println!("{:<64} {:>10}", "tag", "entries");

    for row in rows {
        let kind: String = row.try_get("tag_kind")?;
        let key: String = row.try_get("tag_key")?;
        let entry_count: i64 = row.try_get("entry_count")?;

        println!("{:<64} {:>10}", format!("{kind}:{key}"), entry_count);
    }

    Ok(())
}

