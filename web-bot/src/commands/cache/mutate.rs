//! Cache mutation commands.
//!
//! This module owns cache operations that change data:
//!
//! - add tag links,
//! - remove tag links,
//! - delete cache entries,
//! - remove tag associations globally,
//! - clear all cache tables.
//!
//! Destructive entry deletion and non-destructive tag unlinking are deliberately
//! separate. This preserves the operational distinction between:
//!
//! - "remove this cached artifact"
//! - "remove this secondary association"
//!
//! Most URL-targeted operations use metadata-only existence checks so the CLI
//! does not load large payload bodies just to decide whether a row exists.

use sqlx::Executor;

use web_crawler_db::cache_key_digest;

use super::common::{
    key_for_url,
    parse_tag,
    parse_tags,
    prompt_confirm,
    redact_database_url,
    CacheHandle,
};

pub(crate) async fn tag_url(
    cache: &CacheHandle,
    url: &str,
    namespace: Option<String>,
    tags: Vec<String>,
) -> anyhow::Result<()> {
    if tags.is_empty() {
        anyhow::bail!("at least one tag is required");
    }

    let cache_key = key_for_url(url, namespace)?;

    if cache.get_metadata(&cache_key).await?.is_none() {
        anyhow::bail!("URL not found in cache: {}", url);
    }

    let tags = parse_tags(tags)?;
    cache.add_tags(&cache_key, &tags).await?;

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

pub(crate) async fn untag_url(
    cache: &CacheHandle,
    url: &str,
    namespace: Option<String>,
    all: bool,
    tags: Vec<String>,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;

    if cache.get_metadata(&cache_key).await?.is_none() {
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
            cache.remove_tags(&cache_key, &tags).await?;

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

pub(crate) async fn delete_entries(
    cache: &CacheHandle,
    url: Option<String>,
    tag: Option<String>,
    tag_kind: Option<String>,
    namespace: Option<String>,
    force: bool,
) -> anyhow::Result<()> {
    let target_count = u8::from(url.is_some())
        + u8::from(tag.is_some())
        + u8::from(tag_kind.is_some());

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
    cache: &CacheHandle,
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

    let result = sqlx::query(
        r#"
        DELETE FROM cache_entries
        WHERE key_digest = $1
        "#,
    )
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
    cache: &CacheHandle,
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
    cache: &CacheHandle,
    kind: &str,
    force: bool,
) -> anyhow::Result<()> {
    if kind.trim().is_empty() {
        anyhow::bail!("tag kind cannot be empty");
    }

    if !prompt_confirm(
        &format!("Delete ALL cached entries carrying any `{kind}` tag?"),
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

pub(crate) async fn remove_tag_from_all(
    cache: &CacheHandle,
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

pub(crate) async fn remove_tag_kind_from_all(
    cache: &CacheHandle,
    kind: &str,
    force: bool,
) -> anyhow::Result<()> {
    if kind.trim().is_empty() {
        anyhow::bail!("tag kind cannot be empty");
    }

    if !prompt_confirm(
        &format!(
            "Remove ALL `{kind}` tag links from cached entries without deleting entries?"
        ),
        force,
    )? {
        eprintln!("Aborted.");
        return Ok(());
    }

    let removed = cache.remove_tag_kind_from_all(kind).await?;

    eprintln!(
        "Removed {} tag link(s) of kind `{}`",
        removed,
        kind
    );

    Ok(())
}

pub(crate) async fn clear_cache(
    cache: &CacheHandle,
    database_url: &str,
    force: bool,
) -> anyhow::Result<()> {
    let display_url = redact_database_url(database_url);

    if !prompt_confirm(&format!("Delete ALL cache rows in {display_url}?"), force)? {
        eprintln!("Aborted.");
        return Ok(());
    }

    let mut tx = cache.pool().begin().await?;

    tx.execute("DELETE FROM cache_auxiliary").await?;
    tx.execute("DELETE FROM cache_entry_tags").await?;
    tx.execute("DELETE FROM cache_tags").await?;
    tx.execute("DELETE FROM cache_payloads").await?;
    tx.execute("DELETE FROM cache_entries").await?;

    tx.commit().await?;

    eprintln!("Cache cleared.");

    Ok(())
}
