//! Cache inspection and management commands (Postgres artifact cache).
//!
//! This command module provides operational tools for inspecting and managing
//! the shared Postgres-backed artifact cache used by the crawler.
//!
//! The cache has two operator-facing concepts:
//!
//! - entries: cached artifacts addressed by requested URL (CacheKey)
//! - tags: structured secondary associations (kind + key) over cached artifacts
//!
//! Tags are written in compound CLI form `kind:key` (e.g. `entity:business-123`,
//! `run:manual-debug`, `category:electricians`).
//!
//! The CLI intentionally exposes a small command language instead of mirroring
//! every helper method as its own subcommand.
//!
//! Destructive artifact deletion and tag-association removal are deliberately
//! separate:
//!
//! - `delete` removes cached entries
//! - `untag` / `remove-tag` remove tag links (entries stay)

use clap::Subcommand;
use sqlx::Row;
use std::path::PathBuf;

use web_crawler_db::{
    cache_key_digest,
    CacheEntry,
    CacheEntryRef,
    CacheKey,
    CachePayload,
    CachePayloadCompression,
    CacheTag,
    PostgresCache,
};

type CacheHandle = PostgresCache;

#[derive(Subcommand, Debug)]
pub enum CacheCommands {
    /// Show metadata for one cached URL.
    #[command(visible_alias = "get")]
    Lookup {
        url: String,
        /// Optional logical cache namespace (currently ignored by key model).
        #[arg(long)]
        namespace: Option<String>,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
        /// Include uncompressed payload body in JSON output.
        #[arg(long)]
        full: bool,
    },

    /// Print or save the cached HTML snapshot for one URL.
    #[command(visible_alias = "html")]
    Snapshot {
        url: String,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// List cached entries (optionally filtered by tag).
    List {
        #[arg(long)]
        tag: Option<String>,
        #[arg(long = "tag-kind")]
        tag_kind: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// List known tags or tags attached to one cached URL.
    Tags {
        url: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Add tags to one cached URL (idempotent merge).
    Tag {
        url: String,
        #[arg(long)]
        namespace: Option<String>,
        tags: Vec<String>,
    },

    /// Remove tags from one cached URL.
    Untag {
        url: String,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        all: bool,
        tags: Vec<String>,
    },

    /// Remove cached entries (URL, --tag, or --tag-kind).
    #[command(visible_alias = "rm", visible_alias = "remove")]
    Delete {
        url: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long = "tag-kind")]
        tag_kind: Option<String>,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(short, long)]
        force: bool,
    },

    /// Remove one exact tag association from every entry (entries stay).
    RemoveTag {
        tag: String,
        #[arg(short, long)]
        force: bool,
    },

    /// Remove all associations of one tag kind (entries stay).
    RemoveTagKind {
        kind: String,
        #[arg(short, long)]
        force: bool,
    },

    /// Clear the entire cache database.
    Clear {
        #[arg(short, long)]
        force: bool,
    },

    /// Show cache statistics.
    Stats {
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(action: CacheCommands, database_url: &str) -> anyhow::Result<()> {
    let cache = CacheHandle::connect(database_url).await?;

    match action {
        CacheCommands::Lookup { url, namespace, json, full } => {
            lookup_metadata(&cache, &url, namespace, json, full).await?;
        }
        CacheCommands::Snapshot { url, namespace, output } => {
            get_snapshot(&cache, &url, namespace, output).await?;
        }
        CacheCommands::List { tag, tag_kind, json } => {
            list_entries(&cache, tag, tag_kind, json).await?;
        }
        CacheCommands::Tags { url, kind, namespace, json } => {
            list_tags(&cache, url, kind, namespace, json).await?;
        }
        CacheCommands::Tag { url, namespace, tags } => {
            tag_url(&cache, &url, namespace, tags).await?;
        }
        CacheCommands::Untag { url, namespace, all, tags } => {
            untag_url(&cache, &url, namespace, all, tags).await?;
        }
        CacheCommands::Delete { url, tag, tag_kind, namespace, force } => {
            delete_entries(&cache, url, tag, tag_kind, namespace, force).await?;
        }
        CacheCommands::RemoveTag { tag, force } => {
            remove_tag_from_all(&cache, &tag, force).await?;
        }
        CacheCommands::RemoveTagKind { kind, force } => {
            remove_tag_kind_from_all(&cache, &kind, force).await?;
        }
        CacheCommands::Clear { force } => {
            clear_cache(&cache, database_url, force).await?;
        }
        CacheCommands::Stats { json } => {
            show_stats(&cache, database_url, json).await?;
        }
    }

    Ok(())
}

fn key_for_url(url: &str, namespace: Option<String>) -> anyhow::Result<CacheKey> {
    if namespace.is_some() {
        eprintln!(
            "warning: cache namespace is ignored by the current Postgres cache key model"
        );
    }
    Ok(CacheKey::for_url(url::Url::parse(url)?))
}

fn parse_tag(value: &str) -> anyhow::Result<CacheTag> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("tag cannot be empty");
    }
    if !trimmed.contains(':') {
        anyhow::bail!("tag `{}` must use kind:key format, e.g. entity:business-123", trimmed);
    }
    Ok(CacheTag::from_compound(trimmed))
}

fn parse_tags(values: Vec<String>) -> anyhow::Result<Vec<CacheTag>> {
    values.iter().map(|v| parse_tag(v)).collect()
}

/// Returns the single primary payload for a cache entry.
fn primary_payload(entry: &CacheEntry) -> &CachePayload {
    &entry.payload
}

fn format_status(status: Option<u16>) -> String {
    status.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string())
}

fn format_bytes(byte_len: i64) -> String {
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

fn prompt_confirm(message: &str, force: bool) -> anyhow::Result<bool> {
    if force {
        return Ok(true);
    }
    eprint!("{message} [y/N]: ");
    use std::io::{self, Write};
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().eq_ignore_ascii_case("y"))
}

async fn lookup_metadata(
    cache: &CacheHandle,
    url: &str,
    namespace: Option<String>,
    json: bool,
    full: bool,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;

    match cache.get(&cache_key).await {
        Some(entry) => {
            let primary = primary_payload(&entry);

            // JSON output shape (singular payload, typed metadata)
            let payload_json = serde_json::json!({
                "descriptor": primary.descriptor,
                "body": if full && primary.descriptor.compression == CachePayloadCompression::None {
                    Some(String::from_utf8_lossy(&primary.body).to_string())
                } else {
                    None::<String>
                }
            });

            let value = serde_json::json!({
                "metadata": entry.metadata,
                "payload": payload_json,
                "tags": entry.tags.iter().map(|tag| {
                    serde_json::json!({
                        "kind": tag.kind(),
                        "key": tag.key(),
                        "tag": tag.as_compound(),
                    })
                }).collect::<Vec<_>>(),
            });

            if json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                eprintln!("Cache hit");
                eprintln!("  Requested URL:  {}", entry.metadata.request.requested_url);
                if let Some(final_url) = &entry.metadata.response.final_url {
                    eprintln!("  Final URL:      {}", final_url);
                }
                eprintln!("  Stored at ms:   {}", entry.metadata.stored_at_unix_ms);
                eprintln!("  Status:         {}", format_status(entry.metadata.response.status_code));
                eprintln!(
                    "  Content type:   {}",
                    entry.metadata.response.content_type.as_deref().unwrap_or("-")
                );

                eprintln!("  Snapshot size:  {}", format_bytes(primary.descriptor.byte_len as i64));
                eprintln!("  SHA-256:        {}", primary.descriptor.sha256_hex);

                // Typed metadata access (no more extracted_json)
                let anchors_len = entry.metadata.anchors.len();
                eprintln!("  Anchors:        {}", anchors_len);

                // PageInfo title (adjust field name if compiler complains)
                if let Some(title) = entry.metadata.page_info.as_ref()
                    .and_then(|pi| {
                        // Common shape: pi.title: Option<String>
                        // If this doesn't compile, replace with the actual field
                        // e.g. pi.title.as_deref() or pi.metadata.get("title")...
                        #[allow(unused_variables)]
                        let _ = pi;
                        None::<&str> // TODO: replace with real PageInfo title access
                    })
                {
                    eprintln!("  Title:          {}", title);
                }

                if !entry.tags.is_empty() {
                    eprintln!(
                        "  Tags:           {}",
                        entry.tags.iter().map(|t| t.as_compound()).collect::<Vec<_>>().join(", ")
                    );
                }
            }
        }
        None => {
            if json {
                println!("null");
            } else {
                eprintln!("Not found in cache: {}", url);
            }
        }
    }
    Ok(())
}

async fn get_snapshot(
    cache: &CacheHandle,
    url: &str,
    namespace: Option<String>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;
    let entry = cache.get(&cache_key).await
        .ok_or_else(|| anyhow::anyhow!("URL not found in cache: {}", url))?;

    let payload = primary_payload(&entry);

    if payload.descriptor.compression != CachePayloadCompression::None {
        anyhow::bail!("unsupported compression: {:?}", payload.descriptor.compression);
    }

    let html = String::from_utf8_lossy(&payload.body).to_string();

    if let Some(path) = output {
        std::fs::write(&path, &html)?;
        eprintln!("Snapshot written to {}", path.display());
    } else {
        println!("{}", html);
    }
    Ok(())
}

async fn list_entries(
    cache: &CacheHandle,
    tag: Option<String>,
    tag_kind: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    match (tag, tag_kind) {
        (None, None) => list_all_entries(cache, json).await,
        (Some(t), None) => list_by_tag(cache, &t, json).await,
        (None, Some(k)) => list_by_tag_kind(cache, &k, json).await,
        (Some(_), Some(_)) => anyhow::bail!("use either --tag or --tag-kind, not both"),
    }
}

async fn list_all_entries(cache: &CacheHandle, json: bool) -> anyhow::Result<()> {
    // Raw query kept for aggregate payload size + tag count (high-level API doesn't expose this yet)
    let rows = sqlx::query(
        r#"
        SELECT
            e.key_digest,
            e.key_json,
            e.requested_url,
            e.final_url,
            e.stored_at_unix_ms,
            e.entry_kind,
            e.metadata_version,
            e.status_code,
            e.content_type,
            COALESCE(SUM(p.byte_len), 0) AS payload_bytes,
            COUNT(DISTINCT et.tag_kind || ':' || et.tag_key) AS tag_count
        FROM cache_entries e
        LEFT JOIN cache_payloads p ON p.key_digest = e.key_digest
        LEFT JOIN cache_entry_tags et ON et.key_digest = e.key_digest
        GROUP BY e.key_digest, e.key_json, e.requested_url, e.final_url,
                 e.stored_at_unix_ms, e.entry_kind, e.metadata_version,
                 e.status_code, e.content_type
        ORDER BY e.stored_at_unix_ms DESC
        "#,
    )
    .fetch_all(cache.pool())
    .await?;

    let values = rows.iter().map(|row| {
        anyhow::Ok(serde_json::json!({
            "key_digest": row.try_get::<String, _>("key_digest")?,
            "key": serde_json::from_str::<serde_json::Value>(&row.try_get::<String, _>("key_json")?)?,
            "requested_url": row.try_get::<String, _>("requested_url")?,
            "final_url": row.try_get::<Option<String>, _>("final_url")?,
            "stored_at_unix_ms": row.try_get::<i64, _>("stored_at_unix_ms")?,
            "entry_kind": row.try_get::<String, _>("entry_kind")?,
            "metadata_version": row.try_get::<i64, _>("metadata_version")?,
            "status_code": row.try_get::<Option<i64>, _>("status_code")?,
            "content_type": row.try_get::<Option<String>, _>("content_type")?,
            "payload_bytes": row.try_get::<i64, _>("payload_bytes")?,
            "tag_count": row.try_get::<i64, _>("tag_count")?,
        }))
    }).collect::<anyhow::Result<Vec<_>>>()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else if values.is_empty() {
        eprintln!("No cache entries found.");
    } else {
        println!("stored_at_ms\tstatus\tbytes\ttags\turl");
        for v in values {
            let stored = v.get("stored_at_unix_ms").and_then(|x| x.as_i64()).unwrap_or_default();
            let status = v.get("status_code").and_then(|x| x.as_i64()).map(|x| x.to_string()).unwrap_or_else(|| "-".into());
            let bytes = v.get("payload_bytes").and_then(|x| x.as_i64()).unwrap_or_default();
            let tags = v.get("tag_count").and_then(|x| x.as_i64()).unwrap_or_default();
            let url = v.get("requested_url").and_then(|x| x.as_str()).unwrap_or("-");
            println!("{}\t{}\t{}\t{}\t{}", stored, status, format_bytes(bytes), tags, url);
        }
    }
    Ok(())
}

async fn list_by_tag(cache: &CacheHandle, tag: &str, json: bool) -> anyhow::Result<()> {
    let tag = parse_tag(tag)?;
    let refs = cache.list_by_tag(&tag).await?;
    print_entry_refs(refs, json, &format!("No cache entries found for tag: {}", tag.as_compound()))
}

async fn list_by_tag_kind(cache: &CacheHandle, kind: &str, json: bool) -> anyhow::Result<()> {
    let refs = cache.list_by_tag_kind(kind).await?;
    print_entry_refs(refs, json, &format!("No cache entries found for tag kind: {}", kind))
}

fn print_entry_refs(refs: Vec<CacheEntryRef>, json: bool, empty_message: &str) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&refs)?);
    } else if refs.is_empty() {
        eprintln!("{empty_message}");
    } else {
        println!("stored_at_ms\tstatus\tcontent_type\turl\tkey_digest");
        for e in refs {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                e.stored_at_unix_ms,
                format_status(e.status_code),
                e.content_type.unwrap_or_else(|| "-".into()),
                e.requested_url,
                e.key_digest
            );
        }
    }
    Ok(())
}

async fn list_tags(
    cache: &CacheHandle,
    url: Option<String>,
    kind: Option<String>,
    namespace: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    match (url, kind) {
        (None, None) => list_all_tags(cache, json).await,
        (None, Some(k)) => list_tags_by_kind(cache, &k, json).await,
        (Some(u), None) => list_tags_for_url(cache, &u, namespace, json).await,
        (Some(_), Some(_)) => anyhow::bail!("use either a URL or --kind, not both"),
    }
}

async fn list_all_tags(cache: &CacheHandle, json: bool) -> anyhow::Result<()> {
    // Fixed: removed reference to non-existent `t.tag` column.
    // We now select kind+key and build the compound string in Rust.
    let rows = sqlx::query(
        r#"
        SELECT
            t.tag_kind,
            t.tag_key,
            COUNT(et.key_digest) AS entry_count
        FROM cache_tags t
        LEFT JOIN cache_entry_tags et
            ON et.tag_kind = t.tag_kind AND et.tag_key = t.tag_key
        GROUP BY t.tag_kind, t.tag_key
        ORDER BY t.tag_kind ASC, t.tag_key ASC
        "#,
    )
    .fetch_all(cache.pool())
    .await?;

    print_tag_rows(rows, json, "No tags found.")
}

async fn list_tags_by_kind(cache: &CacheHandle, kind: &str, json: bool) -> anyhow::Result<()> {
    let rows = sqlx::query(
        r#"
        SELECT
            t.tag_kind,
            t.tag_key,
            COUNT(et.key_digest) AS entry_count
        FROM cache_tags t
        LEFT JOIN cache_entry_tags et
            ON et.tag_kind = t.tag_kind AND et.tag_key = t.tag_key
        WHERE t.tag_kind = $1
        GROUP BY t.tag_kind, t.tag_key
        ORDER BY t.tag_key ASC
        "#,
    )
    .bind(kind)
    .fetch_all(cache.pool())
    .await?;

    print_tag_rows(rows, json, &format!("No tags found for kind: {kind}"))
}

fn print_tag_rows(rows: Vec<sqlx::postgres::PgRow>, json: bool, empty_message: &str) -> anyhow::Result<()> {
    if json {
        let values = rows.iter().map(|row| {
            let kind: String = row.try_get("tag_kind")?;
            let key: String = row.try_get("tag_key")?;
            let entry_count: i64 = row.try_get("entry_count")?;
            anyhow::Ok(serde_json::json!({
                "kind": kind,
                "key": key,
                "tag": format!("{}:{}", kind, key),
                "entry_count": entry_count,
            }))
        }).collect::<anyhow::Result<Vec<_>>>()?;
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else if rows.is_empty() {
        eprintln!("{empty_message}");
    } else {
        println!("tag\tentries");
        for row in rows {
            let kind: String = row.try_get("tag_kind")?;
            let key: String = row.try_get("tag_key")?;
            let entry_count: i64 = row.try_get("entry_count")?;
            println!("{}:{}\t{}", kind, key, entry_count);
        }
    }
    Ok(())
}

async fn list_tags_for_url(
    cache: &CacheHandle,
    url: &str,
    namespace: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;
    if cache.get_metadata(&cache_key).await?.is_none() {
        anyhow::bail!("URL not found in cache: {}", url);
    }
    let tags = cache.list_tags_for_entry(&cache_key).await?;

    if json {
        let value = tags.iter().map(|t| serde_json::json!({
            "kind": t.kind(),
            "key": t.key(),
            "tag": t.as_compound(),
        })).collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else if tags.is_empty() {
        eprintln!("No tags found for URL: {}", url);
    } else {
        for t in tags {
            println!("{}", t.as_compound());
        }
    }
    Ok(())
}

async fn tag_url(
    cache: &CacheHandle,
    url: &str,
    namespace: Option<String>,
    tags: Vec<String>,
) -> anyhow::Result<()> {
    if tags.is_empty() {
        anyhow::bail!("at least one tag is required");
    }
    let cache_key = key_for_url(url, namespace)?;
    if cache.get(&cache_key).await.is_none() {
        anyhow::bail!("URL not found in cache: {}", url);
    }
    let tags = parse_tags(tags)?;
    cache.add_tags(&cache_key, &tags).await?;
    eprintln!(
        "Tagged {} with {}",
        url,
        tags.iter().map(|t| t.as_compound()).collect::<Vec<_>>().join(", ")
    );
    Ok(())
}

async fn untag_url(
    cache: &CacheHandle,
    url: &str,
    namespace: Option<String>,
    all: bool,
    tags: Vec<String>,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;
    if cache.get(&cache_key).await.is_none() {
        anyhow::bail!("URL not found in cache: {}", url);
    }

    match (all, tags.is_empty()) {
        (true, true) => {
            let removed = cache.clear_tags(&cache_key).await?;
            eprintln!("Removed {} tag link(s) from {}", removed, url);
        }
        (true, false) => anyhow::bail!("use either --all or explicit tags, not both"),
        (false, true) => anyhow::bail!("provide at least one tag, or use --all"),
        (false, false) => {
            let tags = parse_tags(tags)?;
            cache.remove_tags(&cache_key, &tags).await?;
            eprintln!(
                "Removed tags from {}: {}",
                url,
                tags.iter().map(|t| t.as_compound()).collect::<Vec<_>>().join(", ")
            );
        }
    }
    Ok(())
}

async fn delete_entries(
    cache: &CacheHandle,
    url: Option<String>,
    tag: Option<String>,
    tag_kind: Option<String>,
    namespace: Option<String>,
    force: bool,
) -> anyhow::Result<()> {
    let target_count = url.is_some() as u8 + tag.is_some() as u8 + tag_kind.is_some() as u8;
    if target_count != 1 {
        anyhow::bail!("provide exactly one delete target: URL, --tag, or --tag-kind");
    }
    if let Some(u) = url {
        delete_url(cache, &u, namespace, force).await?;
    } else if let Some(t) = tag {
        delete_entries_by_tag(cache, &t, force).await?;
    } else if let Some(k) = tag_kind {
        delete_entries_by_tag_kind(cache, &k, force).await?;
    }
    Ok(())
}

async fn delete_url(cache: &CacheHandle, url: &str, namespace: Option<String>, force: bool) -> anyhow::Result<()> {
    if !prompt_confirm(&format!("Delete cached entry for {url}?"), force)? {
        eprintln!("Aborted.");
        return Ok(());
    }
    let cache_key = key_for_url(url, namespace)?;
    let key_digest = cache_key_digest(&cache_key)?;
    let result = sqlx::query("DELETE FROM cache_entries WHERE key_digest = $1")
        .bind(&key_digest)
        .execute(cache.pool())
        .await?;
    if result.rows_affected() > 0 {
        eprintln!("Deleted cached entry: {}", url);
    } else {
        eprintln!("Not found in cache: {}", url);
    }
    Ok(())
}

async fn delete_entries_by_tag(cache: &CacheHandle, tag: &str, force: bool) -> anyhow::Result<()> {
    let tag = parse_tag(tag)?;
    if !prompt_confirm(&format!("Delete ALL cached entries tagged {}?", tag.as_compound()), force)? {
        eprintln!("Aborted.");
        return Ok(());
    }
    let removed = cache.delete_entries_by_tag(&tag).await?;
    eprintln!("Deleted {} cached entr{} tagged {}", removed, if removed == 1 { "y" } else { "ies" }, tag.as_compound());
    Ok(())
}

async fn delete_entries_by_tag_kind(cache: &CacheHandle, kind: &str, force: bool) -> anyhow::Result<()> {
    if !prompt_confirm(&format!("Delete ALL cached entries carrying any `{}` tag?", kind), force)? {
        eprintln!("Aborted.");
        return Ok(());
    }
    let removed = cache.delete_entries_by_tag_kind(kind).await?;
    eprintln!("Deleted {} cached entr{} carrying tag kind `{}`", removed, if removed == 1 { "y" } else { "ies" }, kind);
    Ok(())
}

async fn remove_tag_from_all(cache: &CacheHandle, tag: &str, force: bool) -> anyhow::Result<()> {
    let tag = parse_tag(tag)?;
    if !prompt_confirm(&format!("Remove tag {} from ALL cached entries without deleting entries?", tag.as_compound()), force)? {
        eprintln!("Aborted.");
        return Ok(());
    }
    let removed = cache.remove_tag_from_all(&tag).await?;
    eprintln!("Removed {} tag link(s) for {}", removed, tag.as_compound());
    Ok(())
}

async fn remove_tag_kind_from_all(cache: &CacheHandle, kind: &str, force: bool) -> anyhow::Result<()> {
    if !prompt_confirm(&format!("Remove ALL `{}` tag links from cached entries without deleting entries?", kind), force)? {
        eprintln!("Aborted.");
        return Ok(());
    }
    let removed = cache.remove_tag_kind_from_all(kind).await?;
    eprintln!("Removed {} tag link(s) of kind `{}`", removed, kind);
    Ok(())
}

async fn clear_cache(cache: &CacheHandle, database_url: &str, force: bool) -> anyhow::Result<()> {
    if !prompt_confirm(&format!("Delete ALL cache rows in {}?", database_url), force)? {
        eprintln!("Aborted.");
        return Ok(());
    }
    let mut tx = cache.pool().begin().await?;
    sqlx::query("DELETE FROM cache_auxiliary").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM cache_entry_tags").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM cache_tags").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM cache_payloads").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM cache_entries").execute(&mut *tx).await?;
    tx.commit().await?;
    eprintln!("Cache cleared.");
    Ok(())
}

async fn show_stats(cache: &CacheHandle, database_url: &str, json: bool) -> anyhow::Result<()> {
    let row = sqlx::query(
        r#"
        SELECT
            (SELECT COUNT(*) FROM cache_entries) AS total_entries,
            (SELECT COUNT(*) FROM cache_payloads) AS total_payloads,
            (SELECT COALESCE(SUM(byte_len), 0) FROM cache_payloads) AS total_payload_bytes,
            (SELECT COUNT(*) FROM cache_tags) AS total_tags,
            (SELECT COUNT(*) FROM cache_entry_tags) AS total_tag_links,
            (SELECT COUNT(*) FROM cache_auxiliary) AS total_auxiliary_values
        "#,
    )
    .fetch_one(cache.pool())
    .await?;

    let total_entries: i64 = row.try_get("total_entries")?;
    let total_payloads: i64 = row.try_get("total_payloads")?;
    let total_payload_bytes: i64 = row.try_get("total_payload_bytes")?;
    let total_tags: i64 = row.try_get("total_tags")?;
    let total_tag_links: i64 = row.try_get("total_tag_links")?;
    let total_auxiliary_values: i64 = row.try_get("total_auxiliary_values")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "database_url": database_url,
            "total_entries": total_entries,
            "total_payloads": total_payloads,
            "total_payload_bytes": total_payload_bytes,
            "total_tags": total_tags,
            "total_tag_links": total_tag_links,
            "total_auxiliary_values": total_auxiliary_values,
        }))?);
    } else {
        eprintln!("Cache DB:          {}", database_url);
        eprintln!("Entries:           {}", total_entries);
        eprintln!("Payloads:          {}", total_payloads);
        eprintln!("Payload bytes:     {} ({:.2} MiB)", total_payload_bytes, total_payload_bytes as f64 / 1_048_576.0);
        eprintln!("Tags:              {}", total_tags);
        eprintln!("Tag links:         {}", total_tag_links);
        eprintln!("Auxiliary values:  {}", total_auxiliary_values);
    }
    Ok(())
}

