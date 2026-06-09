//! `SqliteCache` — main implementation and public API surface.
//!
//! This is the primary type used by the crawler and downstream components.
//! It provides:
//!
//! - A simple, forgiving hot path (`get` / `put`)
//! - Rich tag-based grouping and bulk lifecycle operations
//! - Per-entry auxiliary key/value storage for derived data
//! - Diagnostic capabilities via `inspect` (when implemented)
//! - Direct access to the underlying `SqlitePool` for advanced use
//!
//! The implementation uses a single connection pool with WAL mode and foreign
//! key support. Writes that modify multiple tables (entry + payloads + tags)
//! are performed inside transactions where appropriate.
//!
//! See `mod.rs` for the overall design philosophy of the cache layer.


use std::path::Path;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use sqlx::Row;

// use crate::sqlite_cache::now_unix_ms;
use crate::sqlite_cache::CacheEntry;
use crate::sqlite_cache::CacheEntryRef;
use crate::sqlite_cache::CacheError;
use crate::sqlite_cache::CacheKey;
use crate::sqlite_cache::CacheResult;
use crate::sqlite_cache::CacheTag;
// use crate::sqlite_cache::CACHE_METADATA_VERSION;
use crate::sqlite_cache::CacheEntryMetadata;
use crate::sqlite_cache::cache_key_digest;
use crate::sqlite_cache::CachePayload;

/// Main SQLite-backed cache for the web crawler engine.
///
/// It stores:
/// - Core crawl metadata and binary payloads
/// - Tags for grouping and application-level linking
/// - Auxiliary key/value data for downstream post-processing
#[derive(Debug, Clone)]
pub struct SqliteCache {
    pool: SqlitePool,
}

impl SqliteCache {
    /// Open (or create) the cache database at the given path.
    ///
    /// This will create the database file if it doesn't exist and run
    /// all necessary migrations.
    pub async fn open(path: impl AsRef<Path>) -> CacheResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5)); // ← Added

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;

        let cache = Self { pool };
        cache.migrate().await?;
        Ok(cache)
    }

    /// Returns a reference to the underlying connection pool.
    /// Useful for advanced queries or integration with other systems.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ============================================================
    // Schema
    // ============================================================

    async fn migrate(&self) -> CacheResult<()> {
        // cache_entries
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cache_entries (
                key_digest          TEXT PRIMARY KEY,
                key_json            TEXT NOT NULL,
                metadata_version    INTEGER NOT NULL,
                entry_kind          TEXT NOT NULL,
                requested_url       TEXT NOT NULL,
                requested_host      TEXT,
                final_url           TEXT,
                final_host          TEXT,
                namespace           TEXT,
                profile_key_json    TEXT NOT NULL,
                stored_at_unix_ms   INTEGER NOT NULL,
                status_code         INTEGER,
                content_type        TEXT,
                metadata_json       TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Useful indexes
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_entries_requested_host ON cache_entries(requested_host);"
        ).execute(&self.pool).await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_entries_final_host ON cache_entries(final_host);"
        ).execute(&self.pool).await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_entries_stored_at ON cache_entries(stored_at_unix_ms);"
        ).execute(&self.pool).await?;

        // cache_payloads
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cache_payloads (
                payload_id  TEXT PRIMARY KEY,
                key_digest  TEXT NOT NULL REFERENCES cache_entries(key_digest) ON DELETE CASCADE,
                role        TEXT NOT NULL,
                media_type  TEXT,
                compression TEXT NOT NULL,
                sha256_hex  TEXT NOT NULL,
                byte_len    INTEGER NOT NULL,
                body        BLOB NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_payloads_key_digest ON cache_payloads(key_digest);"
        ).execute(&self.pool).await?;

        // Tags
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cache_tags (
                tag TEXT PRIMARY KEY
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cache_entry_tags (
                tag         TEXT NOT NULL REFERENCES cache_tags(tag) ON DELETE CASCADE,
                key_digest  TEXT NOT NULL REFERENCES cache_entries(key_digest) ON DELETE CASCADE,
                PRIMARY KEY (tag, key_digest)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_entry_tags_key_digest ON cache_entry_tags(key_digest);"
        ).execute(&self.pool).await?;

        // Auxiliary key/value data
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cache_auxiliary (
                key_digest  TEXT NOT NULL REFERENCES cache_entries(key_digest) ON DELETE CASCADE,
                aux_key     TEXT NOT NULL,
                value_json  TEXT NOT NULL,
                PRIMARY KEY (key_digest, aux_key)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

// ————————————————————————————————————————————————————————————————————————————
// Core Hot Path Implementation
// ————————————————————————————————————————————————————————————————————————————

impl SqliteCache {

    /// Retrieve a cache entry if it exists and is valid.
    /// Returns `None` on any per-entry problem (missing, decode failure, checksum mismatch, etc.).
    pub async fn get(&self, key: &CacheKey) -> Option<CacheEntry> {
        match self.load_entry_raw(key).await {
            Ok(Some(entry)) => Some(entry),
            Ok(None) => None,
            Err(_) => None, // graceful degradation
        }
    }

    /// Store or overwrite a complete cache entry (metadata + payloads + current tags).
    pub async fn put(&self, entry: &CacheEntry) -> CacheResult<()> {
        let key = entry.key();
        let key_digest = cache_key_digest(key)?;

        let key_json = serde_json::to_string_pretty(key)
            .map_err(|e| CacheError::Json(e.to_string()))?;

        let metadata_json = serde_json::to_string_pretty(&entry.metadata)
            .map_err(|e| CacheError::Json(e.to_string()))?;

        let profile_key_json = serde_json::to_string_pretty(&key.profile_key)
            .map_err(|e| CacheError::Json(e.to_string()))?;

        let mut tx = self.pool.begin().await?;

        // Upsert main entry
        sqlx::query(
            r#"
            INSERT INTO cache_entries (
                key_digest, key_json, metadata_version, entry_kind,
                requested_url, requested_host, final_url, final_host,
                namespace, profile_key_json, stored_at_unix_ms,
                status_code, content_type, metadata_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(key_digest) DO UPDATE SET
                key_json = excluded.key_json,
                metadata_version = excluded.metadata_version,
                entry_kind = excluded.entry_kind,
                requested_url = excluded.requested_url,
                requested_host = excluded.requested_host,
                final_url = excluded.final_url,
                final_host = excluded.final_host,
                namespace = excluded.namespace,
                profile_key_json = excluded.profile_key_json,
                stored_at_unix_ms = excluded.stored_at_unix_ms,
                status_code = excluded.status_code,
                content_type = excluded.content_type,
                metadata_json = excluded.metadata_json
            "#,
        )
        .bind(&key_digest)
        .bind(key_json)
        .bind(entry.metadata.metadata_version as i64)
        .bind(&entry.metadata.entry_kind)
        .bind(&entry.metadata.request.requested_url)
        .bind(&entry.metadata.request.requested_host)
        .bind(&entry.metadata.response.final_url)
        .bind(&entry.metadata.response.final_host)
        .bind(&entry.metadata.request.namespace)
        .bind(profile_key_json)
        .bind(entry.metadata.stored_at_unix_ms)
        .bind(entry.metadata.response.status_code.map(|c| c as i64))
        .bind(&entry.metadata.response.content_type)
        .bind(metadata_json)
        .execute(&mut *tx)
        .await?;

        // Replace payloads atomically
        sqlx::query("DELETE FROM cache_payloads WHERE key_digest = ?")
            .bind(&key_digest)
            .execute(&mut *tx)
            .await?;

        for payload in &entry.payloads {
            let d = &payload.descriptor;
            sqlx::query(
                r#"
                INSERT INTO cache_payloads (
                    payload_id, key_digest, role, media_type, compression,
                    sha256_hex, byte_len, body
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&d.payload_id)
            .bind(&key_digest)
            .bind(d.role.as_db_str())
            .bind(&d.media_type)
            .bind(d.compression.as_db_str())
            .bind(&d.sha256_hex)
            .bind(d.byte_len as i64)
            .bind(&payload.body)
            .execute(&mut *tx)
            .await?;
        }

        // Replace tag set for this entry
        sqlx::query("DELETE FROM cache_entry_tags WHERE key_digest = ?")
            .bind(&key_digest)
            .execute(&mut *tx)
            .await?;

        for tag in &entry.tags {
            sqlx::query("INSERT OR IGNORE INTO cache_tags (tag) VALUES (?)")
                .bind(tag.as_str())
                .execute(&mut *tx)
                .await?;

            sqlx::query(
                "INSERT OR IGNORE INTO cache_entry_tags (tag, key_digest) VALUES (?, ?)"
            )
            .bind(tag.as_str())
            .bind(&key_digest)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    // ———————————————————————————————————————————————————————————————————————-
    // Internal Loading Logic
    // ———————————————————————————————————————————————————————————————————————-

    async fn load_entry_raw(&self, key: &CacheKey) -> CacheResult<Option<CacheEntry>> {
        let key_digest = cache_key_digest(key)?;

        let row = sqlx::query(
            "SELECT metadata_json FROM cache_entries WHERE key_digest = ?"
        )
        .bind(&key_digest)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else { return Ok(None); };

        let metadata_json: String = row.try_get("metadata_json")?;
        let metadata: CacheEntryMetadata = serde_json::from_str(&metadata_json)
            .map_err(|e| CacheError::Json(e.to_string()))?;

        // Load and validate payloads
        let payload_rows = sqlx::query(
            r#"
            SELECT payload_id, role, media_type, compression, sha256_hex, byte_len, body
            FROM cache_payloads WHERE key_digest = ? ORDER BY payload_id
            "#,
        )
        .bind(&key_digest)
        .fetch_all(&self.pool)
        .await?;

        let mut payloads = Vec::new();
        for prow in payload_rows {
            let payload_id: String = prow.try_get("payload_id")?;
            let body: Vec<u8> = prow.try_get("body")?;

            if let Some(descriptor) = metadata
                .payloads
                .iter()
                .find(|d| d.payload_id == payload_id)
                .cloned()
            {
                let observed = crate::sqlite_cache::model::sha256_hex(&body);
                if observed != descriptor.sha256_hex {
                    return Err(CacheError::Invariant(format!(
                        "checksum mismatch on payload {}", payload_id
                    )));
                }
                payloads.push(CachePayload { descriptor, body });
            }
        }

        // Load tags
        let tag_rows = sqlx::query(
            "SELECT tag FROM cache_entry_tags WHERE key_digest = ? ORDER BY tag"
        )
        .bind(&key_digest)
        .fetch_all(&self.pool)
        .await?;

        let mut tags = Vec::new();
        for trow in tag_rows {
            let t: String = trow.try_get("tag")?;
            tags.push(CacheTag::new(t));
        }

        Ok(Some(CacheEntry { metadata, payloads, tags }))
    }
}



// ————————————————————————————————————————————————————————————————————————————
// Tag Operations
// ————————————————————————————————————————————————————————————————————————————

impl SqliteCache {

    /// Add one or more tags to a cache entry.
    /// Tags are normalized on insertion. Existing tags are preserved.
    pub async fn tag(&self, key: &CacheKey, tags: &[CacheTag]) -> CacheResult<()> {
        let key_digest = cache_key_digest(key)?;

        for tag in tags {
            sqlx::query("INSERT OR IGNORE INTO cache_tags (tag) VALUES (?)")
                .bind(tag.as_str())
                .execute(&self.pool)
                .await?;

            sqlx::query(
                "INSERT OR IGNORE INTO cache_entry_tags (tag, key_digest) VALUES (?, ?)"
            )
            .bind(tag.as_str())
            .bind(&key_digest)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Remove specific tags from a cache entry.
    pub async fn untag(&self, key: &CacheKey, tags: &[CacheTag]) -> CacheResult<()> {
        let key_digest = cache_key_digest(key)?;

        for tag in tags {
            sqlx::query(
                "DELETE FROM cache_entry_tags WHERE key_digest = ? AND tag = ?"
            )
            .bind(&key_digest)
            .bind(tag.as_str())
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Delete all entries (and their payloads + auxiliary data) that carry the given tag.
    /// Returns the number of entries deleted.
    pub async fn delete_entries_by_tag(&self, tag: &CacheTag) -> CacheResult<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM cache_entries
            WHERE key_digest IN (
                SELECT key_digest FROM cache_entry_tags WHERE tag = ?
            )
            "#,
        )
        .bind(tag.as_str())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Remove the given tag from every entry that currently has it.
    /// The entries themselves are left intact. Returns number of tag links removed.
    pub async fn remove_tag_from_all(&self, tag: &CacheTag) -> CacheResult<u64> {
        let result = sqlx::query(
            "DELETE FROM cache_entry_tags WHERE tag = ?"
        )
        .bind(tag.as_str())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// List lightweight references for all entries carrying the given tag.
    /// Does not load payloads (use `get` for full entries).
    pub async fn list_by_tag(&self, tag: &CacheTag) -> CacheResult<Vec<CacheEntryRef>> {
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
                e.content_type
            FROM cache_entries e
            JOIN cache_entry_tags t ON t.key_digest = e.key_digest
            WHERE t.tag = ?
            ORDER BY e.stored_at_unix_ms DESC
            "#,
        )
        .bind(tag.as_str())
        .fetch_all(&self.pool)
        .await?;

        let mut refs = Vec::new();
        for row in rows {
            let key_json: String = row.try_get("key_json")?;
            let key: CacheKey = serde_json::from_str(&key_json)
                .map_err(|e| CacheError::Json(e.to_string()))?;

            refs.push(CacheEntryRef {
                key_digest: row.try_get("key_digest")?,
                key,
                requested_url: row.try_get("requested_url")?,
                final_url: row.try_get("final_url")?,
                stored_at_unix_ms: row.try_get("stored_at_unix_ms")?,
                entry_kind: row.try_get("entry_kind")?,
                metadata_version: row.try_get::<i64, _>("metadata_version")? as u32,
                status_code: row
                    .try_get::<Option<i64>, _>("status_code")?
                    .map(|c| c as u16),
                content_type: row.try_get("content_type")?,
            });
        }
        Ok(refs)
    }
}


// ————————————————————————————————————————————————————————————————————————————
// Auxiliary Key/Value Storage
// ————————————————————————————————————————————————————————————————————————————
//
// This provides a flexible per-entry key/value store for derived/post-processed
// data. It is intentionally separate from `CacheEntryMetadata` so that
// downstream pipelines can evolve independently without touching core crawl data.

impl SqliteCache {

    /// Retrieve an auxiliary value attached to a cache entry.
    ///
    /// Returns `None` if the entry or the specific auxiliary key does not exist.
    pub async fn get_auxiliary(
        &self,
        key: &CacheKey,
        aux_key: &str,
    ) -> CacheResult<Option<serde_json::Value>> {
        let key_digest = cache_key_digest(key)?;

        let row = sqlx::query(
            "SELECT value_json FROM cache_auxiliary WHERE key_digest = ? AND aux_key = ?"
        )
        .bind(&key_digest)
        .bind(aux_key)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let json_str: String = row.try_get("value_json")?;
                let value: serde_json::Value = serde_json::from_str(&json_str)
                    .map_err(|e| CacheError::Json(e.to_string()))?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// Store or overwrite an auxiliary value for a cache entry.
    ///
    /// The parent cache entry **must already exist** (created via `put`).
    /// If the entry does not exist, this will fail due to the foreign key constraint.
    pub async fn put_auxiliary(
        &self,
        key: &CacheKey,
        aux_key: &str,
        value: &serde_json::Value,
    ) -> CacheResult<()> {
        let key_digest = cache_key_digest(key)?;

        let value_json = serde_json::to_string(value)
            .map_err(|e| CacheError::Json(e.to_string()))?;

        sqlx::query(
            r#"
            INSERT INTO cache_auxiliary (key_digest, aux_key, value_json)
            VALUES (?, ?, ?)
            ON CONFLICT(key_digest, aux_key) DO UPDATE SET
                value_json = excluded.value_json
            "#,
        )
        .bind(&key_digest)
        .bind(aux_key)
        .bind(value_json)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// List all auxiliary keys currently attached to a cache entry.
    pub async fn list_auxiliary_keys(&self, key: &CacheKey) -> CacheResult<Vec<String>> {
        let key_digest = cache_key_digest(key)?;

        let rows = sqlx::query(
            "SELECT aux_key FROM cache_auxiliary WHERE key_digest = ? ORDER BY aux_key"
        )
        .bind(&key_digest)
        .fetch_all(&self.pool)
        .await?;

        let mut keys = Vec::new();
        for row in rows {
            keys.push(row.try_get("aux_key")?);
        }
        Ok(keys)
    }

    /// Delete a specific auxiliary key from a cache entry.
    pub async fn delete_auxiliary(&self, key: &CacheKey, aux_key: &str) -> CacheResult<()> {
        let key_digest = cache_key_digest(key)?;

        sqlx::query(
            "DELETE FROM cache_auxiliary WHERE key_digest = ? AND aux_key = ?"
        )
        .bind(&key_digest)
        .bind(aux_key)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}


