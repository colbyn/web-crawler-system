//! Centralized SQL queries and schema for the Postgres-backed artifact cache.
//!
//! This module owns the raw SQL used by `web-crawler-db`.
//!
//! The schema separates four concerns:
//!
//! - `cache_entries`: one row per logical cached artifact,
//! - `cache_payloads`: zero or more payload byte blobs per cache entry,
//! - `cache_tags`: normalized tag registry, keyed by `(tag_kind, tag_key)`,
//! - `cache_entry_tags`: many-to-many association between entries and tags,
//! - `cache_auxiliary`: small JSON sidecar values keyed per cache entry.
//!
//! Cache identity is stored as both:
//!
//! - `key_digest`, the compact primary key used for joins and lookups,
//! - `key_json`, the inspectable serialized logical key.
//!
//! Payload bytes are deliberately isolated from entry metadata so hot cache
//! replay paths can read `metadata_json` without pulling large HTML bodies,
//! screenshots, or future binary artifacts through Postgres.
//!
//! Tags are secondary-index associations. Their canonical identity is the pair
//! `(tag_kind, tag_key)`. The display-oriented `kind:key` compound string is not
//! stored as a unique database key because it is ambiguous and redundant.
//!
//! Migration is intentionally explicit. This module only provides statements and
//! query helpers; callers decide when schema creation/migration should run.

use std::collections::HashSet;

use sqlx::{Postgres, QueryBuilder, Transaction};

use crate::tags::CacheTag as Tag;

/// Schema creation statements.
///
/// These are currently idempotent bootstrap statements. They will be called by
/// explicit migration/admin code, never by ordinary `PostgresCache::connect()`.
pub(crate) mod schema {
    pub const CREATE_TABLE_CACHE_ENTRIES: &str = r#"
        CREATE TABLE IF NOT EXISTS cache_entries (
            key_digest          TEXT PRIMARY KEY,
            key_json            JSONB NOT NULL,
            metadata_version    INTEGER NOT NULL,
            entry_kind          TEXT NOT NULL,
            requested_url       TEXT NOT NULL,
            requested_host      TEXT,
            final_url           TEXT,
            final_host          TEXT,
            capture_policy_json JSONB NOT NULL,
            stored_at_unix_ms   BIGINT NOT NULL,
            status_code         INTEGER,
            content_type        TEXT,
            metadata_json       JSONB NOT NULL
        );
    "#;

    pub const CREATE_INDEX_REQUESTED_HOST: &str =
        "CREATE INDEX IF NOT EXISTS idx_cache_entries_requested_host ON cache_entries(requested_host);";

    pub const CREATE_INDEX_FINAL_HOST: &str =
        "CREATE INDEX IF NOT EXISTS idx_cache_entries_final_host ON cache_entries(final_host);";

    pub const CREATE_INDEX_STORED_AT: &str =
        "CREATE INDEX IF NOT EXISTS idx_cache_entries_stored_at ON cache_entries(stored_at_unix_ms);";

    pub const CREATE_INDEX_ENTRY_KIND: &str =
        "CREATE INDEX IF NOT EXISTS idx_cache_entries_entry_kind ON cache_entries(entry_kind);";

    pub const CREATE_TABLE_CACHE_PAYLOADS: &str = r#"
        CREATE TABLE IF NOT EXISTS cache_payloads (
            key_digest  TEXT NOT NULL REFERENCES cache_entries(key_digest) ON DELETE CASCADE,
            payload_id  TEXT NOT NULL,
            role        TEXT NOT NULL,
            media_type  TEXT,
            compression TEXT NOT NULL,
            sha256_hex  TEXT NOT NULL,
            byte_len    BIGINT NOT NULL,
            body        BYTEA NOT NULL,
            PRIMARY KEY (key_digest, payload_id)
        );
    "#;

    pub const CREATE_INDEX_PAYLOADS_KEY_DIGEST: &str =
        "CREATE INDEX IF NOT EXISTS idx_cache_payloads_key_digest ON cache_payloads(key_digest);";

    pub const CREATE_INDEX_PAYLOADS_ROLE: &str =
        "CREATE INDEX IF NOT EXISTS idx_cache_payloads_role ON cache_payloads(role);";

    pub const CREATE_TABLE_CACHE_TAGS: &str = r#"
        CREATE TABLE IF NOT EXISTS cache_tags (
            tag_kind TEXT NOT NULL,
            tag_key  TEXT NOT NULL,
            PRIMARY KEY (tag_kind, tag_key)
        );
    "#;

    pub const CREATE_INDEX_TAGS_KIND: &str =
        "CREATE INDEX IF NOT EXISTS idx_cache_tags_kind ON cache_tags(tag_kind);";

    pub const CREATE_TABLE_CACHE_ENTRY_TAGS: &str = r#"
        CREATE TABLE IF NOT EXISTS cache_entry_tags (
            tag_kind   TEXT NOT NULL,
            tag_key    TEXT NOT NULL,
            key_digest TEXT NOT NULL REFERENCES cache_entries(key_digest) ON DELETE CASCADE,
            PRIMARY KEY (tag_kind, tag_key, key_digest),
            FOREIGN KEY (tag_kind, tag_key)
                REFERENCES cache_tags(tag_kind, tag_key)
                ON DELETE CASCADE
        );
    "#;

    pub const CREATE_INDEX_ENTRY_TAGS_KEY_DIGEST: &str =
        "CREATE INDEX IF NOT EXISTS idx_cache_entry_tags_key_digest ON cache_entry_tags(key_digest);";

    pub const CREATE_INDEX_ENTRY_TAGS_KIND: &str =
        "CREATE INDEX IF NOT EXISTS idx_cache_entry_tags_kind ON cache_entry_tags(tag_kind);";

    pub const CREATE_INDEX_ENTRY_TAGS_TAG: &str =
        "CREATE INDEX IF NOT EXISTS idx_cache_entry_tags_tag ON cache_entry_tags(tag_kind, tag_key);";

    pub const CREATE_TABLE_CACHE_AUXILIARY: &str = r#"
        CREATE TABLE IF NOT EXISTS cache_auxiliary (
            key_digest TEXT NOT NULL REFERENCES cache_entries(key_digest) ON DELETE CASCADE,
            aux_key    TEXT NOT NULL,
            value_json JSONB NOT NULL,
            PRIMARY KEY (key_digest, aux_key)
        );
    "#;
}

// ————————————————————————————————————————————————————————————————————————
// Core entry and payload operations
// ————————————————————————————————————————————————————————————————————————

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

pub(crate) const DELETE_PAYLOADS_FOR_ENTRY: &str = r#"
    DELETE FROM cache_payloads
    WHERE key_digest = $1
"#;

pub(crate) const INSERT_PAYLOAD: &str = r#"
    INSERT INTO cache_payloads (
        key_digest,
        payload_id,
        role,
        media_type,
        compression,
        sha256_hex,
        byte_len,
        body
    ) VALUES (
        $1, $2, $3, $4, $5, $6, $7, $8
    )
"#;

pub(crate) const SELECT_PAYLOADS_FOR_KEY: &str = r#"
    SELECT payload_id, body
    FROM cache_payloads
    WHERE key_digest = $1
    ORDER BY payload_id
"#;

pub(crate) const SELECT_PAYLOAD_FOR_KEY: &str = r#"
    SELECT payload_id, body
    FROM cache_payloads
    WHERE key_digest = $1
      AND payload_id = $2
"#;

// ————————————————————————————————————————————————————————————————————————
// Tag registry and entry/tag association operations
// ————————————————————————————————————————————————————————————————————————

pub(crate) const INSERT_TAG_REGISTRY: &str = r#"
    INSERT INTO cache_tags (tag_kind, tag_key)
    VALUES ($1, $2)
    ON CONFLICT (tag_kind, tag_key) DO NOTHING
"#;

pub(crate) const INSERT_ENTRY_TAG_LINK: &str = r#"
    INSERT INTO cache_entry_tags (tag_kind, tag_key, key_digest)
    VALUES ($1, $2, $3)
    ON CONFLICT (tag_kind, tag_key, key_digest) DO NOTHING
"#;

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

// ————————————————————————————————————————————————————————————————————————
// Auxiliary JSON storage
// ————————————————————————————————————————————————————————————————————————

pub(crate) const SELECT_AUX_VALUE: &str = r#"
    SELECT value_json
    FROM cache_auxiliary
    WHERE key_digest = $1
      AND aux_key = $2
"#;

pub(crate) const UPSERT_AUX: &str = r#"
    INSERT INTO cache_auxiliary (key_digest, aux_key, value_json)
    VALUES ($1, $2, $3)
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

// ————————————————————————————————————————————————————————————————————————
// Batch helpers
// ————————————————————————————————————————————————————————————————————————

/// Efficiently merge many tags onto one cache entry.
///
/// This helper performs two batch inserts:
///
/// 1. upsert unique tags into the global tag registry,
/// 2. upsert entry/tag links into the many-to-many association table.
///
/// The operation is idempotent. Re-adding an existing tag association is cheap
/// and does not create duplicate rows.
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

    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("INSERT INTO cache_tags (tag_kind, tag_key) ");

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

    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("INSERT INTO cache_entry_tags (tag_kind, tag_key, key_digest) ");

    qb.push_values(tags.iter(), |mut row, (kind, key)| {
        row.push_bind(kind)
            .push_bind(key)
            .push_bind(key_digest);
    });

    qb.push(" ON CONFLICT (tag_kind, tag_key, key_digest) DO NOTHING");

    qb.build().execute(&mut **tx).await?;
    Ok(())
}

