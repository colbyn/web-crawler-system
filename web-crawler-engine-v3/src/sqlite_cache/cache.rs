//! SQLite cache implementation.
//!
//! This module owns the concrete SQLite-backed cache used by the crawler.
//!
//! The cache has two related but distinct roles:
//!
//! 1. **Performance cache**
//!    Avoid repeated browser work by reusing previously captured page artifacts.
//!
//! 2. **Persistent artifact store**
//!    Keep crawl artifacts, payloads, tags, and derived auxiliary values available
//!    for downstream tools.
//!
//! ## Cache identity
//!
//! Cache identity is defined by [`CacheKey`] and stored as a stable digest.
//! The key is request-addressed:
//!
//! - requested URL,
//! - optional namespace,
//! - key/schema versions.
//!
//! Runtime facts such as final URL, redirects, status code, telemetry, extracted
//! page data, and browser/driver profile identity are stored in metadata, not
//! used as primary lookup identity.
//!
//! This distinction matters. Browser profiles are execution/provenance details.
//! If the cache key includes a Chrome profile or worker bucket, the same URL
//! crawled by two different profiles becomes two different cache entries. That
//! fragments the shared performance cache and turns scheduler implementation
//! details into durable storage identity.
//!
//! If page content genuinely varies by crawl context, represent that
//! deliberately through namespace or a future semantic vary dimension. Do not
//! use raw driver profile IDs as artifact identity.
//!
//! ## Tags
//!
//! Tags are a secondary many-to-many index over cache entries.
//!
//! Tags are intentionally structured as:
//!
//! ```text
//! tag_kind + tag_key
//! ```
//!
//! Examples:
//!
//! ```text
//! entity:business-123
//! category:electricians
//! category:hvac
//! run:manual-debug
//! ```
//!
//! This supports two important query modes:
//!
//! - exact tag lookup: all entries tagged `entity:business-123`
//! - kind lookup: all entries with any `entity` tag
//!
//! Tags are not part of cache identity. Multiple seeds, entities, runs, and
//! categories may point at the same cached artifact. Therefore `put()` merges
//! tags instead of replacing them. Destructive replacement is available only via
//! [`SqliteCache::replace_tags`].
//!
//! ## Auxiliary values
//!
//! `cache_auxiliary` is reserved for post-processing and derived data attached
//! to an artifact, such as classifications, extracted contact info, summaries,
//! scores, or downstream analysis results.
//!
//! Do not use auxiliary storage for caller/seed associations. Use tags for that.

use std::path::Path;

use sqlx::sqlite::{
    SqliteConnectOptions,
    SqliteJournalMode,
    SqlitePoolOptions,
    SqliteRow,
    SqliteSynchronous,
};
use sqlx::{
    Row,
    Sqlite,
    SqlitePool,
    Transaction,
};

use crate::sqlite_cache::{
    cache_key_digest,
    CacheEntry,
    CacheEntryMetadata,
    CacheEntryRef,
    CacheError,
    CacheKey,
    CachePayload,
    CacheResult,
    CacheTag,
};

/// Main SQLite-backed cache for the web crawler engine.
///
/// It stores:
///
/// - cache entry metadata,
/// - binary payloads,
/// - structured tags,
/// - derived auxiliary values.
///
/// The hot path is intentionally forgiving: [`SqliteCache::get`] returns
/// `None` for entry-level problems instead of failing the whole crawl.
#[derive(Debug, Clone)]
pub struct SqliteCache {
    pool: SqlitePool,
}

impl SqliteCache {
    /// Open or create the cache database at the given path.
    ///
    /// This creates the parent directory if needed, opens SQLite in WAL mode,
    /// enables foreign keys, and runs idempotent schema creation.
    pub async fn open(path: impl AsRef<Path>) -> CacheResult<Self> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(30));

        let pool = SqlitePoolOptions::new()
            .max_connections(32)
            .acquire_timeout(std::time::Duration::from_secs(60))
            .connect_with(options)
            .await?;

        let cache = Self { pool };
        cache.migrate().await?;

        Ok(cache)
    }

    /// Return the underlying SQLx pool.
    ///
    /// This is intentionally exposed so downstream tools can issue advanced
    /// SQLite queries without forcing this module to grow every possible helper.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ————————————————————————————————————————————————————————————————————————
    // Schema
    // ————————————————————————————————————————————————————————————————————————

    async fn migrate(&self) -> CacheResult<()> {
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

                -- Browser/driver profile provenance.
                --
                -- This is metadata only. It is intentionally not part of
                -- CacheKey/key_digest. Keeping the column name avoids migration
                -- churn while the stored meaning remains "profile that produced
                -- this artifact", not "profile that identifies this artifact".
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

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_entries_requested_host ON cache_entries(requested_host);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_entries_final_host ON cache_entries(final_host);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_entries_stored_at ON cache_entries(stored_at_unix_ms);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cache_payloads (
                key_digest  TEXT NOT NULL REFERENCES cache_entries(key_digest) ON DELETE CASCADE,
                payload_id  TEXT NOT NULL,
                role        TEXT NOT NULL,
                media_type  TEXT,
                compression TEXT NOT NULL,
                sha256_hex  TEXT NOT NULL,
                byte_len    INTEGER NOT NULL,
                body        BLOB NOT NULL,
                PRIMARY KEY (key_digest, payload_id)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_payloads_key_digest ON cache_payloads(key_digest);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cache_tags (
                tag_kind TEXT NOT NULL,
                tag_key  TEXT NOT NULL,
                tag      TEXT NOT NULL UNIQUE,
                PRIMARY KEY (tag_kind, tag_key)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_tags_kind ON cache_tags(tag_kind);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_tags_tag ON cache_tags(tag);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cache_entry_tags (
                tag_kind   TEXT NOT NULL,
                tag_key    TEXT NOT NULL,
                tag        TEXT NOT NULL,
                key_digest TEXT NOT NULL REFERENCES cache_entries(key_digest) ON DELETE CASCADE,
                PRIMARY KEY (tag_kind, tag_key, key_digest),
                FOREIGN KEY (tag_kind, tag_key)
                    REFERENCES cache_tags(tag_kind, tag_key)
                    ON DELETE CASCADE
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_entry_tags_key_digest ON cache_entry_tags(key_digest);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_entry_tags_kind ON cache_entry_tags(tag_kind);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cache_entry_tags_tag ON cache_entry_tags(tag);",
        )
        .execute(&self.pool)
        .await?;

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
// Core Hot Path
// ————————————————————————————————————————————————————————————————————————————

impl SqliteCache {
    /// Retrieve a cache entry if it exists and passes basic load validation.
    ///
    /// Returns `None` on ordinary cache-entry problems:
    ///
    /// - no row,
    /// - JSON decode failure,
    /// - checksum mismatch,
    /// - corrupt payload metadata,
    /// - other per-entry load failures.
    ///
    /// This keeps cache failure from poisoning a crawl. The crawler can treat
    /// `None` as a miss and recrawl.
    pub async fn get(&self, key: &CacheKey) -> Option<CacheEntry> {
        match self.load_entry_raw(key).await {
            Ok(Some(entry)) => Some(entry),
            Ok(None) => {
                tracing::debug!(?key, "sqlite cache miss: no row");
                None
            }
            Err(err) => {
                tracing::warn!(?key, error = %err, "sqlite cache miss: load failed");
                None
            }
        }
    }

    /// Store or overwrite cache metadata and payloads, while merging tags.
    ///
    /// Important semantics:
    ///
    /// - `cache_entries` is upserted.
    /// - `cache_payloads` for this entry are replaced atomically.
    /// - tags are merged with existing tags.
    ///
    /// Tags are **not** replaced because cache entries are reusable artifacts.
    /// Multiple seeds/entities/categories/runs may point at the same artifact.
    pub async fn put(&self, entry: &CacheEntry) -> CacheResult<()> {
        let key = entry.key();
        let key_digest = cache_key_digest(key)?;

        let key_json = serde_json::to_string_pretty(key)
            .map_err(|e| CacheError::Json(e.to_string()))?;

        let metadata_json = serde_json::to_string_pretty(&entry.metadata)
            .map_err(|e| CacheError::Json(e.to_string()))?;

        // Browser profile provenance is request metadata, not cache identity.
        //
        // The database column is retained for inspectability and compatibility,
        // but the value comes from `CacheRequestInfo`, not `CacheKey`.
        let profile_key_json = serde_json::to_string_pretty(
            &entry.metadata.request.profile_key_json,
        )
        .map_err(|e| CacheError::Json(e.to_string()))?;

        let mut tx = self.pool.begin().await?;

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

        sqlx::query("DELETE FROM cache_payloads WHERE key_digest = ?")
            .bind(&key_digest)
            .execute(&mut *tx)
            .await?;

        for payload in &entry.payloads {
            let descriptor = &payload.descriptor;

            sqlx::query(
                r#"
                INSERT INTO cache_payloads (
                    key_digest, payload_id, role, media_type, compression,
                    sha256_hex, byte_len, body
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&key_digest)
            .bind(&descriptor.payload_id)
            .bind(descriptor.role.as_db_str())
            .bind(&descriptor.media_type)
            .bind(descriptor.compression.as_db_str())
            .bind(&descriptor.sha256_hex)
            .bind(descriptor.byte_len as i64)
            .bind(&payload.body)
            .execute(&mut *tx)
            .await?;
        }

        for tag in &entry.tags {
            insert_tag_link_tx(&mut tx, &key_digest, tag).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    // ————————————————————————————————————————————————————————————————————————
    // Internal Loading Logic
    // ————————————————————————————————————————————————————————————————————————

    async fn load_entry_raw(&self, key: &CacheKey) -> CacheResult<Option<CacheEntry>> {
        let key_digest = cache_key_digest(key)?;

        let row = sqlx::query(
            "SELECT metadata_json FROM cache_entries WHERE key_digest = ?",
        )
        .bind(&key_digest)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let metadata_json: String = row.try_get("metadata_json")?;
        let metadata: CacheEntryMetadata = serde_json::from_str(&metadata_json)
            .map_err(|e| CacheError::Json(e.to_string()))?;

        let payload_rows = sqlx::query(
            r#"
            SELECT payload_id, body
            FROM cache_payloads
            WHERE key_digest = ?
            ORDER BY payload_id
            "#,
        )
        .bind(&key_digest)
        .fetch_all(&self.pool)
        .await?;

        let mut payloads = Vec::new();

        for payload_row in payload_rows {
            let payload_id: String = payload_row.try_get("payload_id")?;
            let body: Vec<u8> = payload_row.try_get("body")?;

            let Some(descriptor) = metadata
                .payloads
                .iter()
                .find(|descriptor| descriptor.payload_id == payload_id)
                .cloned()
            else {
                tracing::warn!(
                    key_digest = %key_digest,
                    payload_id = %payload_id,
                    "cache payload exists without matching metadata descriptor"
                );
                continue;
            };

            let observed_sha256 = crate::sqlite_cache::model::sha256_hex(&body);

            if observed_sha256 != descriptor.sha256_hex {
                return Err(CacheError::Invariant(format!(
                    "checksum mismatch on payload {payload_id}"
                )));
            }

            if body.len() != descriptor.byte_len {
                return Err(CacheError::Invariant(format!(
                    "byte length mismatch on payload {payload_id}: observed {}, expected {}",
                    body.len(),
                    descriptor.byte_len
                )));
            }

            payloads.push(CachePayload { descriptor, body });
        }

        let tag_rows = sqlx::query(
            r#"
            SELECT tag_kind, tag_key
            FROM cache_entry_tags
            WHERE key_digest = ?
            ORDER BY tag_kind, tag_key
            "#,
        )
        .bind(&key_digest)
        .fetch_all(&self.pool)
        .await?;

        let mut tags = Vec::new();

        for tag_row in tag_rows {
            let kind: String = tag_row.try_get("tag_kind")?;
            let key: String = tag_row.try_get("tag_key")?;
            tags.push(CacheTag::raw(kind, key));
        }

        Ok(Some(CacheEntry {
            metadata,
            payloads,
            tags,
        }))
    }
}

// ————————————————————————————————————————————————————————————————————————————
// Tag Operations
// ————————————————————————————————————————————————————————————————————————————

impl SqliteCache {
    /// Add tags to a cache entry.
    ///
    /// Existing tags are preserved. This is merge/upsert behavior.
    pub async fn tag(&self, key: &CacheKey, tags: &[CacheTag]) -> CacheResult<()> {
        let key_digest = cache_key_digest(key)?;

        for tag in tags {
            insert_tag_link_pool(&self.pool, &key_digest, tag).await?;
        }

        Ok(())
    }

    /// Explicitly replace all tags for a cache entry.
    ///
    /// This is intentionally separate from [`SqliteCache::put`] because most
    /// crawler writes should merge tags. Replacing tags is destructive and may
    /// remove associations created by other seeds, entities, categories, or runs.
    pub async fn replace_tags(
        &self,
        key: &CacheKey,
        tags: &[CacheTag],
    ) -> CacheResult<()> {
        let key_digest = cache_key_digest(key)?;
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM cache_entry_tags WHERE key_digest = ?")
            .bind(&key_digest)
            .execute(&mut *tx)
            .await?;

        for tag in tags {
            insert_tag_link_tx(&mut tx, &key_digest, tag).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    /// Remove specific tags from a cache entry.
    pub async fn untag(&self, key: &CacheKey, tags: &[CacheTag]) -> CacheResult<()> {
        let key_digest = cache_key_digest(key)?;

        for tag in tags {
            sqlx::query(
                r#"
                DELETE FROM cache_entry_tags
                WHERE key_digest = ?
                  AND tag_kind = ?
                  AND tag_key = ?
                "#,
            )
            .bind(&key_digest)
            .bind(tag.kind())
            .bind(tag.key())
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    /// Remove all tags from a cache entry.
    pub async fn clear_tags(&self, key: &CacheKey) -> CacheResult<u64> {
        let key_digest = cache_key_digest(key)?;

        let result = sqlx::query(
            "DELETE FROM cache_entry_tags WHERE key_digest = ?",
        )
        .bind(&key_digest)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Delete all entries carrying the given exact tag.
    ///
    /// Payloads, auxiliary values, and tag links are removed by cascading foreign
    /// keys.
    pub async fn delete_entries_by_tag(&self, tag: &CacheTag) -> CacheResult<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM cache_entries
            WHERE key_digest IN (
                SELECT key_digest
                FROM cache_entry_tags
                WHERE tag_kind = ?
                  AND tag_key = ?
            )
            "#,
        )
        .bind(tag.kind())
        .bind(tag.key())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Delete all entries carrying any tag of the given kind.
    ///
    /// Example: delete all entries tagged with any `run` tag.
    pub async fn delete_entries_by_tag_kind(&self, kind: &str) -> CacheResult<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM cache_entries
            WHERE key_digest IN (
                SELECT key_digest
                FROM cache_entry_tags
                WHERE tag_kind = ?
            )
            "#,
        )
        .bind(kind)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Remove the given exact tag from every entry.
    ///
    /// Entries are left intact. Returns the number of tag links removed.
    pub async fn remove_tag_from_all(&self, tag: &CacheTag) -> CacheResult<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM cache_entry_tags
            WHERE tag_kind = ?
              AND tag_key = ?
            "#,
        )
        .bind(tag.kind())
        .bind(tag.key())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Remove all tag links of a given kind from every entry.
    ///
    /// Example: remove all `run` links while preserving `entity` and `category`.
    pub async fn remove_tag_kind_from_all(&self, kind: &str) -> CacheResult<u64> {
        let result = sqlx::query(
            "DELETE FROM cache_entry_tags WHERE tag_kind = ?",
        )
        .bind(kind)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// List lightweight references for entries carrying an exact tag.
    ///
    /// Does not load payloads. Use [`SqliteCache::get`] for full entries.
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
            WHERE t.tag_kind = ?
              AND t.tag_key = ?
            ORDER BY e.stored_at_unix_ms DESC
            "#,
        )
        .bind(tag.kind())
        .bind(tag.key())
        .fetch_all(&self.pool)
        .await?;

        rows_to_entry_refs(rows)
    }

    /// List lightweight references for entries carrying any tag of a kind.
    ///
    /// Example queries:
    ///
    /// - all pages associated with any entity,
    /// - all pages associated with any category,
    /// - all pages produced by any run tag.
    pub async fn list_by_tag_kind(&self, kind: &str) -> CacheResult<Vec<CacheEntryRef>> {
        let rows = sqlx::query(
            r#"
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
            WHERE t.tag_kind = ?
            ORDER BY e.stored_at_unix_ms DESC
            "#,
        )
        .bind(kind)
        .fetch_all(&self.pool)
        .await?;

        rows_to_entry_refs(rows)
    }

    /// List all known tags of a given kind.
    pub async fn list_tags_by_kind(&self, kind: &str) -> CacheResult<Vec<CacheTag>> {
        let rows = sqlx::query(
            r#"
            SELECT tag_kind, tag_key
            FROM cache_tags
            WHERE tag_kind = ?
            ORDER BY tag_key
            "#,
        )
        .bind(kind)
        .fetch_all(&self.pool)
        .await?;

        let mut tags = Vec::new();

        for row in rows {
            let kind: String = row.try_get("tag_kind")?;
            let key: String = row.try_get("tag_key")?;
            tags.push(CacheTag::raw(kind, key));
        }

        Ok(tags)
    }

    /// List every tag currently attached to a cache entry.
    pub async fn list_tags_for_entry(&self, key: &CacheKey) -> CacheResult<Vec<CacheTag>> {
        let key_digest = cache_key_digest(key)?;

        let rows = sqlx::query(
            r#"
            SELECT tag_kind, tag_key
            FROM cache_entry_tags
            WHERE key_digest = ?
            ORDER BY tag_kind, tag_key
            "#,
        )
        .bind(&key_digest)
        .fetch_all(&self.pool)
        .await?;

        let mut tags = Vec::new();

        for row in rows {
            let kind: String = row.try_get("tag_kind")?;
            let key: String = row.try_get("tag_key")?;
            tags.push(CacheTag::raw(kind, key));
        }

        Ok(tags)
    }
}

// ————————————————————————————————————————————————————————————————————————————
// Auxiliary Key/Value Storage
// ————————————————————————————————————————————————————————————————————————————

impl SqliteCache {
    /// Retrieve an auxiliary value attached to a cache entry.
    ///
    /// Returns `None` if the entry or auxiliary key does not exist.
    pub async fn get_auxiliary(
        &self,
        key: &CacheKey,
        aux_key: &str,
    ) -> CacheResult<Option<serde_json::Value>> {
        let key_digest = cache_key_digest(key)?;

        let row = sqlx::query(
            "SELECT value_json FROM cache_auxiliary WHERE key_digest = ? AND aux_key = ?",
        )
        .bind(&key_digest)
        .bind(aux_key)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let value_json: String = row.try_get("value_json")?;
        let value = serde_json::from_str(&value_json)
            .map_err(|e| CacheError::Json(e.to_string()))?;

        Ok(Some(value))
    }

    /// Store or overwrite an auxiliary value for a cache entry.
    ///
    /// The parent cache entry must already exist.
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
            "SELECT aux_key FROM cache_auxiliary WHERE key_digest = ? ORDER BY aux_key",
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
    pub async fn delete_auxiliary(
        &self,
        key: &CacheKey,
        aux_key: &str,
    ) -> CacheResult<()> {
        let key_digest = cache_key_digest(key)?;

        sqlx::query(
            "DELETE FROM cache_auxiliary WHERE key_digest = ? AND aux_key = ?",
        )
        .bind(&key_digest)
        .bind(aux_key)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

// ————————————————————————————————————————————————————————————————————————————
// Helpers
// ————————————————————————————————————————————————————————————————————————————

async fn insert_tag_link_tx(
    tx: &mut Transaction<'_, Sqlite>,
    key_digest: &str,
    tag: &CacheTag,
) -> CacheResult<()> {
    let compound = tag.as_compound();

    sqlx::query(
        r#"
        INSERT OR IGNORE INTO cache_tags (
            tag_kind, tag_key, tag
        ) VALUES (?, ?, ?)
        "#,
    )
    .bind(tag.kind())
    .bind(tag.key())
    .bind(&compound)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT OR IGNORE INTO cache_entry_tags (
            tag_kind, tag_key, tag, key_digest
        ) VALUES (?, ?, ?, ?)
        "#,
    )
    .bind(tag.kind())
    .bind(tag.key())
    .bind(&compound)
    .bind(key_digest)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

async fn insert_tag_link_pool(
    pool: &SqlitePool,
    key_digest: &str,
    tag: &CacheTag,
) -> CacheResult<()> {
    let compound = tag.as_compound();

    sqlx::query(
        r#"
        INSERT OR IGNORE INTO cache_tags (
            tag_kind, tag_key, tag
        ) VALUES (?, ?, ?)
        "#,
    )
    .bind(tag.kind())
    .bind(tag.key())
    .bind(&compound)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT OR IGNORE INTO cache_entry_tags (
            tag_kind, tag_key, tag, key_digest
        ) VALUES (?, ?, ?, ?)
        "#,
    )
    .bind(tag.kind())
    .bind(tag.key())
    .bind(&compound)
    .bind(key_digest)
    .execute(pool)
    .await?;

    Ok(())
}

fn rows_to_entry_refs(rows: Vec<SqliteRow>) -> CacheResult<Vec<CacheEntryRef>> {
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
                .map(|code| code as u16),
            content_type: row.try_get("content_type")?,
        });
    }

    Ok(refs)
}
