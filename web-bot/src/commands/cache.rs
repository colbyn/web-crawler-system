//! Cache inspection and management commands.
//!
//! This command module provides operational tools for inspecting and managing
//! the shared SQLite cache used by the crawler.
//!
//! The cache has two operator-facing concepts:
//!
//! - entries: cached artifacts addressed by requested URL + optional namespace,
//! - tags: structured secondary associations over cached artifacts.
//!
//! Tags are written in compound CLI form:
//!
//! ```text
//! kind:key
//! ```
//!
//! Examples:
//!
//! ```text
//! entity:business-123
//! category:electricians
//! run:manual-debug
//! ```
//!
//! The CLI intentionally exposes a small command language instead of mirroring
//! every SQLite helper method as its own subcommand:
//!
//! ```text
//! cache list
//! cache list --tag entity:business-123
//! cache list --tag-kind entity
//! cache tags
//! cache tags --kind entity
//! cache tags https://example.com
//! cache tag https://example.com entity:business-123
//! cache untag https://example.com entity:business-123
//! cache delete --tag run:debug
//! cache remove-tag run:debug
//! ```
//!
//! Destructive artifact deletion and tag-association removal are deliberately
//! separate:
//!
//! - `delete` removes cached entries,
//! - `untag` removes tags from one entry,
//! - `remove-tag` / `remove-tag-kind` remove associations globally without
//!   deleting entries.

use clap::Subcommand;
use sqlx::Row;
use std::path::PathBuf;
use web_crawler_engine_v3::sqlite_cache::{
    cache_key_digest,
    CacheEntry,
    CacheEntryRef,
    CacheKey,
    CachePayload,
    CachePayloadCompression,
    CachePayloadRole,
    CacheTag,
};
use web_crawler_engine_v3::SqliteCache;

#[derive(Subcommand, Debug)]
pub enum CacheCommands {
    /// Show metadata for one cached URL.
    #[command(visible_alias = "get")]
    Lookup {
        url: String,

        /// Optional logical cache namespace.
        #[arg(long)]
        namespace: Option<String>,

        /// Output as JSON.
        #[arg(long)]
        json: bool,

        /// Include uncompressed payload bodies in JSON output.
        #[arg(long)]
        full: bool,
    },

    /// Print or save the cached HTML snapshot for one URL.
    #[command(visible_alias = "html")]
    Snapshot {
        url: String,

        /// Optional logical cache namespace.
        #[arg(long)]
        namespace: Option<String>,

        /// Write to file instead of stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// List cached entries.
    ///
    /// Without filters, this lists all entries.
    ///
    /// Use `--tag kind:key` for an exact tag lookup.
    /// Use `--tag-kind kind` for all entries carrying any tag of that kind.
    List {
        /// Filter entries by exact tag, in kind:key form.
        #[arg(long)]
        tag: Option<String>,

        /// Filter entries by tag kind.
        #[arg(long = "tag-kind")]
        tag_kind: Option<String>,

        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// List known tags, or tags attached to one cached URL.
    ///
    /// Examples:
    ///
    /// - `cache tags`
    /// - `cache tags --kind entity`
    /// - `cache tags https://example.com`
    Tags {
        /// Optional URL whose attached tags should be listed.
        url: Option<String>,

        /// Filter global tag listing by tag kind.
        #[arg(long)]
        kind: Option<String>,

        /// Optional logical cache namespace, used only when URL is provided.
        #[arg(long)]
        namespace: Option<String>,

        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Add tags to one cached URL.
    ///
    /// Tag format: kind:key
    ///
    /// Existing tags are preserved.
    Tag {
        url: String,

        /// Optional logical cache namespace.
        #[arg(long)]
        namespace: Option<String>,

        /// Tags to add, in kind:key form.
        tags: Vec<String>,
    },

    /// Remove tags from one cached URL.
    ///
    /// Use explicit tag arguments to remove specific tags.
    /// Use `--all` to remove every tag from the entry.
    Untag {
        url: String,

        /// Optional logical cache namespace.
        #[arg(long)]
        namespace: Option<String>,

        /// Remove all tags from this entry.
        #[arg(long)]
        all: bool,

        /// Tags to remove, in kind:key form.
        tags: Vec<String>,
    },

    /// Remove cached entries.
    ///
    /// Exactly one target must be provided:
    ///
    /// - URL positional argument,
    /// - `--tag kind:key`,
    /// - `--tag-kind kind`.
    #[command(visible_alias = "rm", visible_alias = "remove")]
    Delete {
        /// URL to delete from cache.
        url: Option<String>,

        /// Delete all entries carrying this exact tag.
        #[arg(long)]
        tag: Option<String>,

        /// Delete all entries carrying any tag of this kind.
        #[arg(long = "tag-kind")]
        tag_kind: Option<String>,

        /// Optional logical cache namespace, used only when URL is provided.
        #[arg(long)]
        namespace: Option<String>,

        /// Do not prompt before deleting.
        #[arg(short, long)]
        force: bool,
    },

    /// Remove one exact tag association from every entry without deleting entries.
    RemoveTag {
        /// Tag to remove globally, in kind:key form.
        tag: String,

        /// Do not prompt before removing tag associations.
        #[arg(short, long)]
        force: bool,
    },

    /// Remove all tag associations of one kind without deleting entries.
    RemoveTagKind {
        /// Tag kind to remove globally.
        kind: String,

        /// Do not prompt before removing tag associations.
        #[arg(short, long)]
        force: bool,
    },

    /// Clear the entire cache database.
    Clear {
        /// Do not prompt before clearing.
        #[arg(short, long)]
        force: bool,
    },

    /// Show cache statistics.
    Stats {
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(action: CacheCommands, cache_db: &PathBuf) -> anyhow::Result<()> {
    let cache = SqliteCache::open(cache_db).await?;

    match action {
        CacheCommands::Lookup {
            url,
            namespace,
            json,
            full,
        } => {
            lookup_metadata(&cache, &url, namespace, json, full).await?;
        }

        CacheCommands::Snapshot {
            url,
            namespace,
            output,
        } => {
            get_snapshot(&cache, &url, namespace, output).await?;
        }

        CacheCommands::List {
            tag,
            tag_kind,
            json,
        } => {
            list_entries(&cache, tag, tag_kind, json).await?;
        }

        CacheCommands::Tags {
            url,
            kind,
            namespace,
            json,
        } => {
            list_tags(&cache, url, kind, namespace, json).await?;
        }

        CacheCommands::Tag {
            url,
            namespace,
            tags,
        } => {
            tag_url(&cache, &url, namespace, tags).await?;
        }

        CacheCommands::Untag {
            url,
            namespace,
            all,
            tags,
        } => {
            untag_url(&cache, &url, namespace, all, tags).await?;
        }

        CacheCommands::Delete {
            url,
            tag,
            tag_kind,
            namespace,
            force,
        } => {
            delete_entries(&cache, url, tag, tag_kind, namespace, force).await?;
        }

        CacheCommands::RemoveTag { tag, force } => {
            remove_tag_from_all(&cache, &tag, force).await?;
        }

        CacheCommands::RemoveTagKind { kind, force } => {
            remove_tag_kind_from_all(&cache, &kind, force).await?;
        }

        CacheCommands::Clear { force } => {
            clear_cache(&cache, cache_db, force).await?;
        }

        CacheCommands::Stats { json } => {
            show_stats(&cache, cache_db, json).await?;
        }
    }

    Ok(())
}

fn key_for_url(
    url: &str,
    namespace: Option<String>,
) -> anyhow::Result<CacheKey> {
    Ok(CacheKey::for_request(
        url::Url::parse(url)?,
        namespace,
    ))
}

fn parse_tag(value: &str) -> anyhow::Result<CacheTag> {
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

fn parse_tags(values: Vec<String>) -> anyhow::Result<Vec<CacheTag>> {
    values.iter().map(|value| parse_tag(value)).collect()
}

fn primary_payload(entry: &CacheEntry) -> Option<&CachePayload> {
    entry
        .payloads
        .iter()
        .find(|payload| payload.descriptor.role == CachePayloadRole::PrimarySnapshot)
        .or_else(|| {
            entry
                .payloads
                .iter()
                .find(|payload| payload.descriptor.role == CachePayloadRole::ResponseBody)
        })
}

fn format_status(status: Option<u16>) -> String {
    status
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
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

    use std::io::{
        self,
        Write,
    };

    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input.trim().eq_ignore_ascii_case("y"))
}

async fn lookup_metadata(
    cache: &SqliteCache,
    url: &str,
    namespace: Option<String>,
    json: bool,
    full: bool,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;

    match cache.get(&cache_key).await {
        Some(entry) => {
            let payload_summary = entry
                .payloads
                .iter()
                .map(|payload| {
                    serde_json::json!({
                        "descriptor": payload.descriptor,
                        "body": if full && payload.descriptor.compression == CachePayloadCompression::None {
                            Some(String::from_utf8_lossy(&payload.body).to_string())
                        } else {
                            None::<String>
                        },
                    })
                })
                .collect::<Vec<_>>();

            let value = serde_json::json!({
                "metadata": entry.metadata,
                "payloads": payload_summary,
                "tags": entry
                    .tags
                    .iter()
                    .map(|tag| {
                        serde_json::json!({
                            "kind": tag.kind(),
                            "key": tag.key(),
                            "tag": tag.as_compound(),
                        })
                    })
                    .collect::<Vec<_>>(),
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
                eprintln!(
                    "  Status:         {}",
                    format_status(entry.metadata.response.status_code)
                );
                eprintln!(
                    "  Content type:   {}",
                    entry
                        .metadata
                        .response
                        .content_type
                        .as_deref()
                        .unwrap_or("-")
                );

                if let Some(primary) = entry.metadata.primary_payload() {
                    eprintln!("  Snapshot size:  {}", format_bytes(primary.byte_len as i64));
                    eprintln!("  SHA-256:        {}", primary.sha256_hex);
                }

                let anchors_len = entry
                    .metadata
                    .extracted_json
                    .get("anchors")
                    .and_then(|value| value.as_array())
                    .map(|value| value.len())
                    .unwrap_or(0);

                eprintln!("  Anchors:        {}", anchors_len);

                if let Some(title) = entry
                    .metadata
                    .extracted_json
                    .pointer("/page_info/title")
                    .and_then(|value| value.as_str())
                {
                    eprintln!("  Title:          {}", title);
                }

                if !entry.tags.is_empty() {
                    eprintln!(
                        "  Tags:           {}",
                        entry
                            .tags
                            .iter()
                            .map(|tag| tag.as_compound())
                            .collect::<Vec<_>>()
                            .join(", ")
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
    cache: &SqliteCache,
    url: &str,
    namespace: Option<String>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;

    let entry = cache
        .get(&cache_key)
        .await
        .ok_or_else(|| anyhow::anyhow!("URL not found in cache: {}", url))?;

    let payload = primary_payload(&entry)
        .ok_or_else(|| anyhow::anyhow!("cache entry has no primary payload: {}", url))?;

    match payload.descriptor.compression {
        CachePayloadCompression::None => {}
        other => anyhow::bail!("unsupported compression: {:?}", other),
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
    cache: &SqliteCache,
    tag: Option<String>,
    tag_kind: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    match (tag, tag_kind) {
        (None, None) => list_all_entries(cache, json).await,
        (Some(tag), None) => list_by_tag(cache, &tag, json).await,
        (None, Some(kind)) => list_by_tag_kind(cache, &kind, json).await,
        (Some(_), Some(_)) => {
            anyhow::bail!("use either --tag or --tag-kind, not both")
        }
    }
}

async fn list_all_entries(
    cache: &SqliteCache,
    json: bool,
) -> anyhow::Result<()> {
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
            COUNT(DISTINCT et.tag) AS tag_count
        FROM cache_entries e
        LEFT JOIN cache_payloads p
            ON p.key_digest = e.key_digest
        LEFT JOIN cache_entry_tags et
            ON et.key_digest = e.key_digest
        GROUP BY
            e.key_digest,
            e.key_json,
            e.requested_url,
            e.final_url,
            e.stored_at_unix_ms,
            e.entry_kind,
            e.metadata_version,
            e.status_code,
            e.content_type
        ORDER BY e.stored_at_unix_ms DESC
        "#,
    )
    .fetch_all(cache.pool())
    .await?;

    let values = rows
        .iter()
        .map(|row| {
            anyhow::Ok(serde_json::json!({
                "key_digest": row.try_get::<String, _>("key_digest")?,
                "key": serde_json::from_str::<serde_json::Value>(
                    &row.try_get::<String, _>("key_json")?
                )?,
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
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else if values.is_empty() {
        eprintln!("No cache entries found.");
    } else {
        println!("stored_at_ms\tstatus\tbytes\ttags\turl");

        for value in values {
            let stored_at = value
                .get("stored_at_unix_ms")
                .and_then(|value| value.as_i64())
                .unwrap_or_default();

            let status = value
                .get("status_code")
                .and_then(|value| value.as_i64())
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());

            let payload_bytes = value
                .get("payload_bytes")
                .and_then(|value| value.as_i64())
                .unwrap_or_default();

            let tag_count = value
                .get("tag_count")
                .and_then(|value| value.as_i64())
                .unwrap_or_default();

            let requested_url = value
                .get("requested_url")
                .and_then(|value| value.as_str())
                .unwrap_or("-");

            println!(
                "{}\t{}\t{}\t{}\t{}",
                stored_at,
                status,
                format_bytes(payload_bytes),
                tag_count,
                requested_url
            );
        }
    }

    Ok(())
}

async fn list_by_tag(
    cache: &SqliteCache,
    tag: &str,
    json: bool,
) -> anyhow::Result<()> {
    let tag = parse_tag(tag)?;
    let refs = cache.list_by_tag(&tag).await?;

    print_entry_refs(
        refs,
        json,
        &format!("No cache entries found for tag: {}", tag.as_compound()),
    )
}

async fn list_by_tag_kind(
    cache: &SqliteCache,
    kind: &str,
    json: bool,
) -> anyhow::Result<()> {
    let refs = cache.list_by_tag_kind(kind).await?;

    print_entry_refs(
        refs,
        json,
        &format!("No cache entries found for tag kind: {}", kind),
    )
}

fn print_entry_refs(
    refs: Vec<CacheEntryRef>,
    json: bool,
    empty_message: &str,
) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&refs)?);
    } else if refs.is_empty() {
        eprintln!("{empty_message}");
    } else {
        println!("stored_at_ms\tstatus\tcontent_type\turl\tkey_digest");

        for entry in refs {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                entry.stored_at_unix_ms,
                format_status(entry.status_code),
                entry.content_type.unwrap_or_else(|| "-".to_string()),
                entry.requested_url,
                entry.key_digest,
            );
        }
    }

    Ok(())
}

async fn list_tags(
    cache: &SqliteCache,
    url: Option<String>,
    kind: Option<String>,
    namespace: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    match (url, kind) {
        (None, None) => list_all_tags(cache, json).await,
        (None, Some(kind)) => list_tags_by_kind(cache, &kind, json).await,
        (Some(url), None) => list_tags_for_url(cache, &url, namespace, json).await,
        (Some(_), Some(_)) => {
            anyhow::bail!("use either a URL or --kind, not both")
        }
    }
}

async fn list_all_tags(
    cache: &SqliteCache,
    json: bool,
) -> anyhow::Result<()> {
    let rows = sqlx::query(
        r#"
        SELECT
            t.tag_kind,
            t.tag_key,
            t.tag,
            COUNT(et.key_digest) AS entry_count
        FROM cache_tags t
        LEFT JOIN cache_entry_tags et
            ON et.tag_kind = t.tag_kind
           AND et.tag_key = t.tag_key
        GROUP BY t.tag_kind, t.tag_key, t.tag
        ORDER BY t.tag_kind ASC, t.tag_key ASC
        "#,
    )
    .fetch_all(cache.pool())
    .await?;

    print_tag_rows(rows, json, "No tags found.")
}

async fn list_tags_by_kind(
    cache: &SqliteCache,
    kind: &str,
    json: bool,
) -> anyhow::Result<()> {
    let rows = sqlx::query(
        r#"
        SELECT
            t.tag_kind,
            t.tag_key,
            t.tag,
            COUNT(et.key_digest) AS entry_count
        FROM cache_tags t
        LEFT JOIN cache_entry_tags et
            ON et.tag_kind = t.tag_kind
           AND et.tag_key = t.tag_key
        WHERE t.tag_kind = ?
        GROUP BY t.tag_kind, t.tag_key, t.tag
        ORDER BY t.tag_key ASC
        "#,
    )
    .bind(kind)
    .fetch_all(cache.pool())
    .await?;

    print_tag_rows(rows, json, &format!("No tags found for kind: {kind}"))
}

fn print_tag_rows(
    rows: Vec<sqlx::sqlite::SqliteRow>,
    json: bool,
    empty_message: &str,
) -> anyhow::Result<()> {
    if json {
        let values = rows
            .iter()
            .map(|row| {
                anyhow::Ok(serde_json::json!({
                    "kind": row.try_get::<String, _>("tag_kind")?,
                    "key": row.try_get::<String, _>("tag_key")?,
                    "tag": row.try_get::<String, _>("tag")?,
                    "entry_count": row.try_get::<i64, _>("entry_count")?,
                }))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        println!("{}", serde_json::to_string_pretty(&values)?);
    } else if rows.is_empty() {
        eprintln!("{empty_message}");
    } else {
        println!("tag\tentries");

        for row in rows {
            let tag: String = row.try_get("tag")?;
            let entry_count: i64 = row.try_get("entry_count")?;
            println!("{tag}\t{entry_count}");
        }
    }

    Ok(())
}

async fn list_tags_for_url(
    cache: &SqliteCache,
    url: &str,
    namespace: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;

    if cache.get(&cache_key).await.is_none() {
        anyhow::bail!("URL not found in cache: {}", url);
    }

    let tags = cache.list_tags_for_entry(&cache_key).await?;

    if json {
        let value = tags
            .iter()
            .map(|tag| {
                serde_json::json!({
                    "kind": tag.kind(),
                    "key": tag.key(),
                    "tag": tag.as_compound(),
                })
            })
            .collect::<Vec<_>>();

        println!("{}", serde_json::to_string_pretty(&value)?);
    } else if tags.is_empty() {
        eprintln!("No tags found for URL: {}", url);
    } else {
        for tag in tags {
            println!("{}", tag.as_compound());
        }
    }

    Ok(())
}

async fn tag_url(
    cache: &SqliteCache,
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
    cache.tag(&cache_key, &tags).await?;

    eprintln!(
        "Tagged {} with {}",
        url,
        tags.iter()
            .map(|tag| tag.as_compound())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(())
}

async fn untag_url(
    cache: &SqliteCache,
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
        (true, false) => {
            anyhow::bail!("use either --all or explicit tags, not both");
        }
        (false, true) => {
            anyhow::bail!("provide at least one tag, or use --all");
        }
        (false, false) => {
            let tags = parse_tags(tags)?;
            cache.untag(&cache_key, &tags).await?;

            eprintln!(
                "Removed tags from {}: {}",
                url,
                tags.iter()
                    .map(|tag| tag.as_compound())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    Ok(())
}

async fn delete_entries(
    cache: &SqliteCache,
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

    if let Some(url) = url {
        delete_url(cache, &url, namespace, force).await?;
    } else if let Some(tag) = tag {
        delete_entries_by_tag(cache, &tag, force).await?;
    } else if let Some(kind) = tag_kind {
        delete_entries_by_tag_kind(cache, &kind, force).await?;
    }

    Ok(())
}

async fn delete_url(
    cache: &SqliteCache,
    url: &str,
    namespace: Option<String>,
    force: bool,
) -> anyhow::Result<()> {
    if !prompt_confirm(&format!("Delete cached entry for {url}?"), force)? {
        eprintln!("Aborted.");
        return Ok(());
    }

    let cache_key = key_for_url(url, namespace)?;
    let key_digest = cache_key_digest(&cache_key)?;

    let result = sqlx::query("DELETE FROM cache_entries WHERE key_digest = ?")
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

async fn delete_entries_by_tag(
    cache: &SqliteCache,
    tag: &str,
    force: bool,
) -> anyhow::Result<()> {
    let tag = parse_tag(tag)?;

    if !prompt_confirm(
        &format!(
            "Delete ALL cached entries tagged {}?",
            tag.as_compound()
        ),
        force,
    )? {
        eprintln!("Aborted.");
        return Ok(());
    }

    let removed = cache.delete_entries_by_tag(&tag).await?;
    eprintln!(
        "Deleted {} cached entr{} tagged {}",
        removed,
        if removed == 1 { "y" } else { "ies" },
        tag.as_compound()
    );

    Ok(())
}

async fn delete_entries_by_tag_kind(
    cache: &SqliteCache,
    kind: &str,
    force: bool,
) -> anyhow::Result<()> {
    if !prompt_confirm(
        &format!(
            "Delete ALL cached entries carrying any `{}` tag?",
            kind
        ),
        force,
    )? {
        eprintln!("Aborted.");
        return Ok(());
    }

    let removed = cache.delete_entries_by_tag_kind(kind).await?;
    eprintln!(
        "Deleted {} cached entr{} carrying tag kind `{}`",
        removed,
        if removed == 1 { "y" } else { "ies" },
        kind
    );

    Ok(())
}

async fn remove_tag_from_all(
    cache: &SqliteCache,
    tag: &str,
    force: bool,
) -> anyhow::Result<()> {
    let tag = parse_tag(tag)?;

    if !prompt_confirm(
        &format!(
            "Remove tag {} from ALL cached entries without deleting entries?",
            tag.as_compound()
        ),
        force,
    )? {
        eprintln!("Aborted.");
        return Ok(());
    }

    let removed = cache.remove_tag_from_all(&tag).await?;
    eprintln!(
        "Removed {} tag link(s) for {}",
        removed,
        tag.as_compound()
    );

    Ok(())
}

async fn remove_tag_kind_from_all(
    cache: &SqliteCache,
    kind: &str,
    force: bool,
) -> anyhow::Result<()> {
    if !prompt_confirm(
        &format!(
            "Remove ALL `{}` tag links from cached entries without deleting entries?",
            kind
        ),
        force,
    )? {
        eprintln!("Aborted.");
        return Ok(());
    }

    let removed = cache.remove_tag_kind_from_all(kind).await?;
    eprintln!("Removed {} tag link(s) of kind `{}`", removed, kind);

    Ok(())
}

async fn clear_cache(
    cache: &SqliteCache,
    cache_db: &PathBuf,
    force: bool,
) -> anyhow::Result<()> {
    if !prompt_confirm(
        &format!("Delete ALL cache rows in {}?", cache_db.display()),
        force,
    )? {
        eprintln!("Aborted.");
        return Ok(());
    }

    let mut tx = cache.pool().begin().await?;

    sqlx::query("DELETE FROM cache_auxiliary")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM cache_entry_tags")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM cache_tags")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM cache_payloads")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM cache_entries")
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    eprintln!("Cache cleared.");
    Ok(())
}

async fn show_stats(
    cache: &SqliteCache,
    cache_db: &PathBuf,
    json: bool,
) -> anyhow::Result<()> {
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
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "cache_db": cache_db,
                "total_entries": total_entries,
                "total_payloads": total_payloads,
                "total_payload_bytes": total_payload_bytes,
                "total_tags": total_tags,
                "total_tag_links": total_tag_links,
                "total_auxiliary_values": total_auxiliary_values,
            }))?
        );
    } else {
        eprintln!("Cache DB:          {}", cache_db.display());
        eprintln!("Entries:           {}", total_entries);
        eprintln!("Payloads:          {}", total_payloads);
        eprintln!(
            "Payload bytes:     {} ({:.2} MiB)",
            total_payload_bytes,
            total_payload_bytes as f64 / 1_048_576.0
        );
        eprintln!("Tags:              {}", total_tags);
        eprintln!("Tag links:         {}", total_tag_links);
        eprintln!("Auxiliary values:  {}", total_auxiliary_values);
    }

    Ok(())
}

