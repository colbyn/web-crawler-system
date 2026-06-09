//! Cache inspection and management commands.
//!
//! This command module provides operational tools for looking inside the shared
//! SQLite cache used by the crawler.
//!
//! The cache stores reusable page artifacts. Tags are a secondary association
//! layer over those artifacts. Tags are structured as:
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
//! This CLI accepts tags in their compound textual form, then converts them to
//! `CacheTag { kind, key }` before calling the cache API.

use clap::Subcommand;
use sqlx::Row;
use std::path::PathBuf;

use web_browser_driver::BrowserProfileKey;
use web_crawler_engine_v3::sqlite_cache::{
    cache_key_digest,
    CacheEntry,
    CacheKey,
    CachePayload,
    CachePayloadCompression,
    CachePayloadRole,
    CacheTag,
};
use web_crawler_engine_v3::SqliteCache;

#[derive(Subcommand, Debug)]
pub enum CacheCommands {
    /// Show metadata for a cached URL
    Lookup {
        url: String,

        /// Browser profile key used to address the cache entry
        #[arg(long, default_value = "default")]
        profile_key: String,

        /// Optional cache namespace
        #[arg(long)]
        namespace: Option<String>,

        #[arg(long)]
        json: bool,

        /// Print payload bodies when possible
        #[arg(long)]
        full: bool,
    },

    /// Print or save the HTML snapshot
    Snapshot {
        url: String,

        /// Browser profile key used to address the cache entry
        #[arg(long, default_value = "default")]
        profile_key: String,

        /// Optional cache namespace
        #[arg(long)]
        namespace: Option<String>,

        /// Write to file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Remove a URL from the cache
    Remove {
        url: String,

        /// Browser profile key used to address the cache entry
        #[arg(long, default_value = "default")]
        profile_key: String,

        /// Optional cache namespace
        #[arg(long)]
        namespace: Option<String>,

        #[arg(short, long)]
        force: bool,
    },

    /// Clear the entire cache database
    Clear {
        #[arg(short, long)]
        force: bool,
    },

    /// Show cache statistics
    Stats {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Add tags to one cached URL.
    ///
    /// Tag format: kind:key
    ///
    /// Examples:
    ///
    /// - entity:business-123
    /// - category:electricians
    /// - run:manual-debug
    Tag {
        url: String,

        #[arg(long, default_value = "default")]
        profile_key: String,

        #[arg(long)]
        namespace: Option<String>,

        /// Tags to add, in kind:key form
        tags: Vec<String>,
    },

    /// List cache entries by exact tag.
    ///
    /// Tag format: kind:key
    ListByTag {
        tag: String,

        #[arg(long)]
        json: bool,
    },

    /// List cache entries carrying any tag of this kind.
    ///
    /// Examples:
    ///
    /// - entity
    /// - category
    /// - run
    ListByTagKind {
        kind: String,

        #[arg(long)]
        json: bool,
    },

    /// List known tags of a given kind.
    ListTagsByKind {
        kind: String,

        #[arg(long)]
        json: bool,
    },
}

pub async fn run(action: CacheCommands, cache_db: &PathBuf) -> anyhow::Result<()> {
    let cache = SqliteCache::open(cache_db).await?;

    match action {
        CacheCommands::Lookup {
            url,
            profile_key,
            namespace,
            json,
            full,
        } => {
            lookup_metadata(&cache, &url, &profile_key, namespace, json, full).await?;
        }

        CacheCommands::Snapshot {
            url,
            profile_key,
            namespace,
            output,
        } => {
            get_snapshot(&cache, &url, &profile_key, namespace, output).await?;
        }

        CacheCommands::Remove {
            url,
            profile_key,
            namespace,
            force,
        } => {
            remove_url(&cache, &url, &profile_key, namespace, force).await?;
        }

        CacheCommands::Clear { force } => {
            clear_cache(&cache, cache_db, force).await?;
        }

        CacheCommands::Stats { json } => {
            show_stats(&cache, cache_db, json).await?;
        }

        CacheCommands::Tag {
            url,
            profile_key,
            namespace,
            tags,
        } => {
            tag_url(&cache, &url, &profile_key, namespace, tags).await?;
        }

        CacheCommands::ListByTag { tag, json } => {
            list_by_tag(&cache, &tag, json).await?;
        }

        CacheCommands::ListByTagKind { kind, json } => {
            list_by_tag_kind(&cache, &kind, json).await?;
        }

        CacheCommands::ListTagsByKind { kind, json } => {
            list_tags_by_kind(&cache, &kind, json).await?;
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

async fn lookup_metadata(
    cache: &SqliteCache,
    url: &str,
    profile_key: &str,
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
                eprintln!("✅ Cache hit: {}", url);
                eprintln!("  Requested URL:  {}", entry.metadata.request.requested_url);

                if let Some(final_url) = &entry.metadata.response.final_url {
                    eprintln!("  Final URL:      {}", final_url);
                }

                eprintln!("  Stored at ms:   {}", entry.metadata.stored_at_unix_ms);
                eprintln!("  Status:         {:?}", entry.metadata.response.status_code);
                eprintln!("  Content type:   {:?}", entry.metadata.response.content_type);

                if let Some(primary) = entry.metadata.primary_payload() {
                    eprintln!("  Snapshot size:  {} bytes", primary.byte_len);
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
                eprintln!("❌ Not found in cache: {}", url);
            }
        }
    }

    Ok(())
}

async fn get_snapshot(
    cache: &SqliteCache,
    url: &str,
    profile_key: &str,
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

async fn remove_url(
    cache: &SqliteCache,
    url: &str,
    profile_key: &str,
    namespace: Option<String>,
    force: bool,
) -> anyhow::Result<()> {
    if !force {
        eprint!("Remove {} from cache? [y/N]: ", url);

        use std::io::{
            self,
            Write,
        };

        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    let cache_key = key_for_url(url, namespace)?;
    let key_digest = cache_key_digest(&cache_key)?;

    let result = sqlx::query("DELETE FROM cache_entries WHERE key_digest = ?")
        .bind(&key_digest)
        .execute(cache.pool())
        .await?;

    if result.rows_affected() > 0 {
        eprintln!("✅ Removed from cache: {}", url);
    } else {
        eprintln!("Not found in cache.");
    }

    Ok(())
}

async fn clear_cache(
    cache: &SqliteCache,
    cache_db: &PathBuf,
    force: bool,
) -> anyhow::Result<()> {
    if !force {
        eprintln!("This will delete ALL cache rows in: {}", cache_db.display());
        eprint!("Are you sure? [y/N]: ");

        use std::io::{
            self,
            Write,
        };

        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("Aborted.");
            return Ok(());
        }
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

    eprintln!("✅ Cache cleared.");
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
            (SELECT COUNT(*) FROM cache_entry_tags) AS total_tag_links
        "#,
    )
    .fetch_one(cache.pool())
    .await?;

    let total_entries: i64 = row.try_get("total_entries")?;
    let total_payloads: i64 = row.try_get("total_payloads")?;
    let total_payload_bytes: i64 = row.try_get("total_payload_bytes")?;
    let total_tags: i64 = row.try_get("total_tags")?;
    let total_tag_links: i64 = row.try_get("total_tag_links")?;

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
            }))?
        );
    } else {
        eprintln!("Cache DB:       {}", cache_db.display());
        eprintln!("Entries:        {}", total_entries);
        eprintln!("Payloads:       {}", total_payloads);
        eprintln!(
            "Payload bytes:  {} ({:.2} MB)",
            total_payload_bytes,
            total_payload_bytes as f64 / 1_048_576.0
        );
        eprintln!("Tags:           {}", total_tags);
        eprintln!("Tag links:      {}", total_tag_links);
    }

    Ok(())
}

async fn tag_url(
    cache: &SqliteCache,
    url: &str,
    profile_key: &str,
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
        "✅ Tagged {} with {}",
        url,
        tags.iter()
            .map(|tag| tag.as_compound())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(())
}

async fn list_by_tag(
    cache: &SqliteCache,
    tag: &str,
    json: bool,
) -> anyhow::Result<()> {
    let tag = parse_tag(tag)?;
    let refs = cache.list_by_tag(&tag).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&refs)?);
    } else if refs.is_empty() {
        eprintln!("No cache entries found for tag: {}", tag.as_compound());
    } else {
        for entry in refs {
            println!(
                "{}\t{}\t{:?}\t{}",
                entry.stored_at_unix_ms,
                entry.requested_url,
                entry.status_code,
                entry.key_digest,
            );
        }
    }

    Ok(())
}

async fn list_by_tag_kind(
    cache: &SqliteCache,
    kind: &str,
    json: bool,
) -> anyhow::Result<()> {
    let refs = cache.list_by_tag_kind(kind).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&refs)?);
    } else if refs.is_empty() {
        eprintln!("No cache entries found for tag kind: {}", kind);
    } else {
        for entry in refs {
            println!(
                "{}\t{}\t{:?}\t{}",
                entry.stored_at_unix_ms,
                entry.requested_url,
                entry.status_code,
                entry.key_digest,
            );
        }
    }

    Ok(())
}

async fn list_tags_by_kind(
    cache: &SqliteCache,
    kind: &str,
    json: bool,
) -> anyhow::Result<()> {
    let tags = cache.list_tags_by_kind(kind).await?;

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
        eprintln!("No tags found for kind: {}", kind);
    } else {
        for tag in tags {
            println!("{}", tag.as_compound());
        }
    }

    Ok(())
}
