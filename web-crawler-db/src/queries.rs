#![allow(unused)]
//! Centralized SQL queries and schema for the Postgres-backed artifact cache.
//!
//! This module owns the raw SQL used by the crate.
//!
//! ## Schema
//!
//! The complete database schema lives in `sql/schema.sql` and is included here
//! as [`SCHEMA`].
//!
//! The schema can be executed directly with:
//!
//! ```bash
//! psql -v ON_ERROR_STOP=1 -f sql/schema.sql -d your_database
//! ```
//!
//! It can also be applied through [`crate::migrate::migrate_pool`].
//!
//! Keep `sql/schema.sql` pure SQL. Do not add `psql`-only metacommands such as
//! `\echo`, because [`crate::migrate::migrate_pool`] executes this string
//! through `sqlx::raw_sql`.
//!
//! ## Storage model
//!
//! The cache currently stores:
//!
//! - one `cache_entries` row per logical cached artifact,
//! - one `cache_payloads` row per cache entry,
//! - many tags per cache entry,
//! - small auxiliary JSON sidecars per cache entry.
//!
//! Tags are intentionally merge-oriented. They are secondary-index
//! associations, not cache identity.

use std::collections::HashSet;

use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::tags::CacheTag as Tag;

/// The complete schema in dependency order.
pub const SCHEMA: &str = include_str!("sql/schema.sql");

// -----------------------------------------------------------------------------
// Core entry operations
// -----------------------------------------------------------------------------

pub(crate) const UPSERT_CACHE_ENTRY: &str = r#"
    INSERT INTO cache_entries (
        key_digest,
        key_json,
        metadata_version,
        entry_kind,
        requested_url,
        requested_host,
        final_url,
        final_host,
        capture_policy_json,
        stored_at_unix_ms,
        status_code,
        content_type,
        metadata_json
    ) VALUES (
        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13
    )
    ON CONFLICT (key_digest) DO UPDATE SET
        key_json = excluded.key_json,
        metadata_version = excluded.metadata_version,
        entry_kind = excluded.entry_kind,
        requested_url = excluded.requested_url,
        requested_host = excluded.requested_host,
        final_url = excluded.final_url,
        final_host = excluded.final_host,
        capture_policy_json = excluded.capture_policy_json,
        stored_at_unix_ms = excluded.stored_at_unix_ms,
        status_code = excluded.status_code,
        content_type = excluded.content_type,
        metadata_json = excluded.metadata_json
"#;

pub(crate) const SELECT_METADATA_JSON: &str = r#"
    SELECT metadata_json
    FROM cache_entries
    WHERE key_digest = $1
"#;

// -----------------------------------------------------------------------------
// Primary payload operations
// -----------------------------------------------------------------------------

pub(crate) const UPSERT_PAYLOAD: &str = r#"
    INSERT INTO cache_payloads (
        key_digest,
        media_type,
        compression,
        sha256_hex,
        byte_len,
        body
    ) VALUES (
        $1, $2, $3, $4, $5, $6
    )
    ON CONFLICT (key_digest) DO UPDATE SET
        media_type = excluded.media_type,
        compression = excluded.compression,
        sha256_hex = excluded.sha256_hex,
        byte_len = excluded.byte_len,
        body = excluded.body
"#;

pub(crate) const SELECT_PAYLOAD_FOR_KEY: &str = r#"
    SELECT
        media_type,
        compression,
        sha256_hex,
        byte_len,
        body
    FROM cache_payloads
    WHERE key_digest = $1
"#;

pub(crate) const DELETE_PAYLOAD_FOR_KEY: &str = r#"
    DELETE FROM cache_payloads
    WHERE key_digest = $1
"#;

// -----------------------------------------------------------------------------
// Tag registry and entry/tag association operations
// -----------------------------------------------------------------------------

pub(crate) const SELECT_TAGS_FOR_ENTRY: &str = r#"
    SELECT tag_kind, tag_key
    FROM cache_entry_tags
    WHERE key_digest = $1
    ORDER BY tag_kind, tag_key
"#;

pub(crate) const DELETE_SPECIFIC_TAG_FROM_ENTRY: &str = r#"
    DELETE FROM cache_entry_tags
    WHERE key_digest = $1
      AND tag_kind = $2
      AND tag_key = $3
"#;

pub(crate) const DELETE_ALL_TAGS_FOR_ENTRY: &str = r#"
    DELETE FROM cache_entry_tags
    WHERE key_digest = $1
"#;

pub(crate) const DELETE_ENTRIES_BY_TAG: &str = r#"
    DELETE FROM cache_entries
    WHERE key_digest IN (
        SELECT key_digest
        FROM cache_entry_tags
        WHERE tag_kind = $1
          AND tag_key = $2
    )
"#;

pub(crate) const DELETE_ENTRIES_BY_TAG_KIND: &str = r#"
    DELETE FROM cache_entries
    WHERE key_digest IN (
        SELECT key_digest
        FROM cache_entry_tags
        WHERE tag_kind = $1
    )
"#;

pub(crate) const REMOVE_TAG_FROM_ALL: &str = r#"
    DELETE FROM cache_entry_tags
    WHERE tag_kind = $1
      AND tag_key = $2
"#;

pub(crate) const REMOVE_TAG_KIND_FROM_ALL: &str = r#"
    DELETE FROM cache_entry_tags
    WHERE tag_kind = $1
"#;

pub(crate) const LIST_ENTRIES_BY_TAG: &str = r#"
    SELECT
        e.key_digest,
        e.key_json,
        e.requested_url,
        e.final_url,
        e.stored_at_unix_ms,
        e.entry_kind,
        e.metadata_version,
        e.status_code,
        e.content_type
    FROM cache_entries e
    JOIN cache_entry_tags t ON t.key_digest = e.key_digest
    WHERE t.tag_kind = $1
      AND t.tag_key = $2
    ORDER BY e.stored_at_unix_ms DESC
"#;

pub(crate) const LIST_ENTRIES_BY_TAG_KIND: &str = r#"
    SELECT DISTINCT
        e.key_digest,
        e.key_json,
        e.requested_url,
        e.final_url,
        e.stored_at_unix_ms,
        e.entry_kind,
        e.metadata_version,
        e.status_code,
        e.content_type
    FROM cache_entries e
    JOIN cache_entry_tags t ON t.key_digest = e.key_digest
    WHERE t.tag_kind = $1
    ORDER BY e.stored_at_unix_ms DESC
"#;

pub(crate) const LIST_TAGS_BY_KIND: &str = r#"
    SELECT tag_kind, tag_key
    FROM cache_tags
    WHERE tag_kind = $1
    ORDER BY tag_key
"#;

pub(crate) const LIST_TAGS_FOR_ENTRY: &str = r#"
    SELECT tag_kind, tag_key
    FROM cache_entry_tags
    WHERE key_digest = $1
    ORDER BY tag_kind, tag_key
"#;

// -----------------------------------------------------------------------------
// Auxiliary JSON storage
// -----------------------------------------------------------------------------

pub(crate) const SELECT_AUX_VALUE: &str = r#"
    SELECT value_json
    FROM cache_auxiliary
    WHERE key_digest = $1
      AND aux_key = $2
"#;

pub(crate) const UPSERT_AUX: &str = r#"
    INSERT INTO cache_auxiliary (
        key_digest,
        aux_key,
        value_json
    ) VALUES (
        $1, $2, $3
    )
    ON CONFLICT (key_digest, aux_key)
    DO UPDATE SET value_json = excluded.value_json
"#;

pub(crate) const LIST_AUX_KEYS: &str = r#"
    SELECT aux_key
    FROM cache_auxiliary
    WHERE key_digest = $1
    ORDER BY aux_key
"#;

pub(crate) const DELETE_AUX: &str = r#"
    DELETE FROM cache_auxiliary
    WHERE key_digest = $1
      AND aux_key = $2
"#;

// -----------------------------------------------------------------------------
// Batch helpers
// -----------------------------------------------------------------------------

/// Merge many tags onto one cache entry.
///
/// This function is idempotent:
///
/// - missing tag registry rows are inserted,
/// - existing tag registry rows are left alone,
/// - missing entry/tag links are inserted,
/// - existing entry/tag links are left alone.
///
/// This is the default behavior because crawler phases may add associations at
/// different times.
pub async fn batch_upsert_tags(
    tx: &mut Transaction<'_, Postgres>,
    key_digest: &str,
    tags: &[Tag],
) -> Result<(), sqlx::Error> {
    if tags.is_empty() {
        return Ok(());
    }

    let unique: Vec<_> = tags
        .iter()
        .map(|tag| (tag.kind().to_string(), tag.key().to_string()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    insert_tag_registry_batch(tx, &unique).await?;
    insert_entry_tag_links_batch(tx, key_digest, &unique).await?;

    Ok(())
}

async fn insert_tag_registry_batch(
    tx: &mut Transaction<'_, Postgres>,
    tags: &[(String, String)],
) -> Result<(), sqlx::Error> {
    if tags.is_empty() {
        return Ok(());
    }

    let mut qb = QueryBuilder::<Postgres>::new(
        "INSERT INTO cache_tags (tag_kind, tag_key) ",
    );

    qb.push_values(tags.iter(), |mut row, (kind, key)| {
        row.push_bind(kind).push_bind(key);
    });

    qb.push(" ON CONFLICT (tag_kind, tag_key) DO NOTHING");

    qb.build().execute(&mut **tx).await?;

    Ok(())
}

async fn insert_entry_tag_links_batch(
    tx: &mut Transaction<'_, Postgres>,
    key_digest: &str,
    tags: &[(String, String)],
) -> Result<(), sqlx::Error> {
    if tags.is_empty() {
        return Ok(());
    }

    let mut qb = QueryBuilder::<Postgres>::new(
        "INSERT INTO cache_entry_tags (key_digest, tag_kind, tag_key) ",
    );

    qb.push_values(tags.iter(), |mut row, (kind, key)| {
        row.push_bind(key_digest)
            .push_bind(kind)
            .push_bind(key);
    });

    qb.push(" ON CONFLICT (key_digest, tag_kind, tag_key) DO NOTHING");

    qb.build().execute(&mut **tx).await?;

    Ok(())
}

