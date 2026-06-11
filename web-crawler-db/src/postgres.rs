//! PostgreSQL implementation of the `web-crawler-db` artifact cache.
//!
//! `PostgresCache` is the main runtime SDK type for cached crawl artifacts.
//!
//! This type intentionally does **not** create or migrate database schema when
//! connecting. Schema setup belongs to explicit admin paths in `migrate.rs`.
//! Crawlers and workers should be able to connect without accidentally changing
//! database structure.
//!
//! The API separates hot-path replay from heavyweight artifact inspection:
//!
//! - [`PostgresCache::get_metadata`] reads only metadata JSON.
//! - [`PostgresCache::get_payload`] reads one payload body by ID.
//! - [`PostgresCache::try_get_full_entry`] reads metadata, payloads, and tags.
//! - [`PostgresCache::get`] is the forgiving hot-path wrapper that converts
//!   per-entry corruption or decode failures into `None`.
//!
//! Writes validate [`CacheEntry`] before touching storage. This keeps caller-made
//! impossible states out of Postgres.
//!
//! Tags remain secondary-index associations. They are merged idempotently through
//! `(tag_kind, tag_key)` pairs and are not part of cache identity.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgRow};
use sqlx::{PgPool, Row};

use crate::error::{DbError, DbResult};
use crate::key::{cache_key_digest, CacheKey};
use crate::model::{CacheEntry, CacheEntryMetadata, CacheEntryRef, CachePayload, CacheTag};
use crate::queries;

/// Main Postgres-backed cache for crawler artifacts.
#[derive(Debug, Clone)]
pub struct PostgresCache {
    pool: PgPool,
}

impl PostgresCache {
    /// Connect using a Postgres connection string.
    ///
    /// This method does not create or migrate schema. Run explicit setup through
    /// `web_crawler_db::migrate_pool` or `web_crawler_db::migrate_database_url`.
    pub async fn connect(database_url: &str) -> DbResult<Self> {
        let options: PgConnectOptions = database_url
            .parse()
            .map_err(|error| DbError::internal(format!("invalid database url: {error}")))?;

        let pool = PgPoolOptions::new()
            .max_connections(32)
            .acquire_timeout(std::time::Duration::from_secs(30))
            .connect_with(options)
            .await?;

        Ok(Self { pool })
    }

    /// Construct a cache handle from an existing pool.
    ///
    /// This is useful for applications that own their own Postgres pool and want
    /// the cache SDK to share it.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Return the underlying sqlx pool for advanced usage.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

// ————————————————————————————————————————————————————————————————————————
// Read API
// ————————————————————————————————————————————————————————————————————————

impl PostgresCache {
    /// Forgiving hot-path read.
    ///
    /// Returns `None` on:
    ///
    /// - cache miss,
    /// - metadata decode failure,
    /// - payload checksum mismatch,
    /// - incomplete/corrupt stored entry,
    /// - transient load error.
    ///
    /// Use [`PostgresCache::try_get_full_entry`] for diagnostic paths that need
    /// exact errors.
    pub async fn get(&self, key: &CacheKey) -> Option<CacheEntry> {
        match self.try_get_full_entry(key).await {
            Ok(Some(entry)) => Some(entry),

            Ok(None) => {
                tracing::debug!(?key, "postgres cache miss: no row");
                None
            }

            Err(error) => {
                tracing::warn!(
                    ?key,
                    error = %error,
                    "postgres cache miss: load failed"
                );
                None
            }
        }
    }

    /// Strict full-entry read.
    ///
    /// Loads metadata, all payload bodies, and tags. This is useful for
    /// inspection, export, debugging, and consumers that truly need the complete
    /// cache entry.
    pub async fn try_get_full_entry(&self, key: &CacheKey) -> DbResult<Option<CacheEntry>> {
        self.load_entry_raw(key).await
    }

    /// Read only metadata for a cache key.
    ///
    /// This is the preferred hot replay path. It avoids pulling HTML bodies,
    /// screenshots, or other large payload bytes through Postgres when the caller
    /// only needs extracted replay data and payload descriptors.
    pub async fn get_metadata(&self, key: &CacheKey) -> DbResult<Option<CacheEntryMetadata>> {
        let key_digest = cache_key_digest(key)?;

        let row = sqlx::query(queries::SELECT_METADATA_JSON)
            .bind(&key_digest)
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let metadata_json: serde_json::Value = row.try_get("metadata_json")?;

        let metadata: CacheEntryMetadata = serde_json::from_value(metadata_json)
            .map_err(|error| DbError::json(error.to_string()))?;

        validate_loaded_metadata_for_key(key, &metadata)?;

        Ok(Some(metadata))
    }

    /// Read one payload body by ID.
    ///
    /// The descriptor is loaded from metadata, then the payload row is fetched
    /// and verified against the descriptor checksum and byte length.
    pub async fn get_payload(
        &self,
        key: &CacheKey,
        payload_id: &str,
    ) -> DbResult<Option<CachePayload>> {
        let key_digest = cache_key_digest(key)?;

        let Some(metadata) = self.get_metadata(key).await? else {
            return Ok(None);
        };

        let Some(descriptor) = metadata
            .payloads
            .iter()
            .find(|descriptor| descriptor.payload_id == payload_id)
            .cloned()
        else {
            return Ok(None);
        };

        let row = sqlx::query(queries::SELECT_PAYLOAD_FOR_KEY)
            .bind(&key_digest)
            .bind(payload_id)
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let body: Vec<u8> = row.try_get("body")?;

        validate_payload_body(
            payload_id,
            &descriptor.sha256_hex,
            descriptor.byte_len,
            &body,
        )?;

        Ok(Some(CachePayload { descriptor, body }))
    }

    async fn load_entry_raw(&self, key: &CacheKey) -> DbResult<Option<CacheEntry>> {
        let key_digest = cache_key_digest(key)?;

        let Some(metadata) = self.get_metadata(key).await? else {
            return Ok(None);
        };

        let payload_rows = sqlx::query(queries::SELECT_PAYLOADS_FOR_KEY)
            .bind(&key_digest)
            .fetch_all(&self.pool)
            .await?;

        let mut payloads = Vec::with_capacity(payload_rows.len());

        for row in payload_rows {
            let payload_id: String = row.try_get("payload_id")?;
            let body: Vec<u8> = row.try_get("body")?;

            let Some(descriptor) = metadata
                .payloads
                .iter()
                .find(|descriptor| descriptor.payload_id == payload_id)
                .cloned()
            else {
                return Err(DbError::invariant(format!(
                    "payload row has no matching metadata descriptor: {payload_id}"
                )));
            };

            validate_payload_body(
                &payload_id,
                &descriptor.sha256_hex,
                descriptor.byte_len,
                &body,
            )?;

            payloads.push(CachePayload { descriptor, body });
        }

        let tag_rows = sqlx::query(queries::SELECT_TAGS_FOR_ENTRY)
            .bind(&key_digest)
            .fetch_all(&self.pool)
            .await?;

        let mut tags = Vec::with_capacity(tag_rows.len());

        for row in tag_rows {
            let kind: String = row.try_get("tag_kind")?;
            let key: String = row.try_get("tag_key")?;

            tags.push(CacheTag::raw(kind, key));
        }

        let entry = CacheEntry {
            metadata,
            payloads,
            tags,
        };

        validate_loaded_entry(&entry)?;

        Ok(Some(entry))
    }
}

// ————————————————————————————————————————————————————————————————————————
// Write API
// ————————————————————————————————————————————————————————————————————————

impl PostgresCache {
    /// Store or update a cache entry.
    ///
    /// Semantics:
    ///
    /// - validates the entry before writing,
    /// - upserts metadata,
    /// - replaces all payload rows for the entry,
    /// - merges tags idempotently.
    ///
    /// Payload replacement plus tag merging is intentional but asymmetric. Use
    /// `replace_tags` when the caller wants exact tag replacement.
    pub async fn put(&self, entry: &CacheEntry) -> DbResult<()> {
        entry.validate()?;

        let key = entry.key();
        let key_digest = cache_key_digest(key)?;

        let key_json = serde_json::to_value(key)
            .map_err(|error| DbError::json(error.to_string()))?;

        let metadata_json = serde_json::to_value(&entry.metadata)
            .map_err(|error| DbError::json(error.to_string()))?;

        let mut tx = self.pool.begin().await?;

        sqlx::query(queries::UPSERT_CACHE_ENTRY)
            .bind(&key_digest)
            .bind(&key_json)
            .bind(entry.metadata.metadata_version as i64)
            .bind(&entry.metadata.entry_kind)
            .bind(&entry.metadata.request.requested_url)
            .bind(&entry.metadata.request.requested_host)
            .bind(&entry.metadata.response.final_url)
            .bind(&entry.metadata.response.final_host)
            .bind(&entry.metadata.request.capture_policy_json)
            .bind(entry.metadata.stored_at_unix_ms)
            .bind(entry.metadata.response.status_code.map(|code| code as i64))
            .bind(&entry.metadata.response.content_type)
            .bind(&metadata_json)
            .execute(&mut *tx)
            .await?;

        sqlx::query(queries::DELETE_PAYLOADS_FOR_ENTRY)
            .bind(&key_digest)
            .execute(&mut *tx)
            .await?;

        for payload in &entry.payloads {
            let descriptor = &payload.descriptor;

            sqlx::query(queries::INSERT_PAYLOAD)
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

        queries::batch_upsert_tags(&mut tx, &key_digest, &entry.tags)
            .await
            .map_err(DbError::Database)?;

        tx.commit().await?;

        Ok(())
    }
}

// ————————————————————————————————————————————————————————————————————————
// Tag Operations
// ————————————————————————————————————————————————————————————————————————

impl PostgresCache {
    /// Merge tags onto a cache entry.
    ///
    /// This is idempotent. Existing associations are left as-is.
    pub async fn add_tags(&self, key: &CacheKey, tags: &[CacheTag]) -> DbResult<()> {
        let key_digest = cache_key_digest(key)?;
        let mut tx = self.pool.begin().await?;

        queries::batch_upsert_tags(&mut tx, &key_digest, tags)
            .await
            .map_err(DbError::Database)?;

        tx.commit().await?;

        Ok(())
    }

    /// Compatibility alias for older engine call sites.
    ///
    /// New code should prefer [`PostgresCache::add_tags`].
    pub async fn tag(&self, key: &CacheKey, tags: &[CacheTag]) -> DbResult<()> {
        self.add_tags(key, tags).await
    }

    /// Replace all tags for a cache entry.
    pub async fn replace_tags(&self, key: &CacheKey, tags: &[CacheTag]) -> DbResult<()> {
        let key_digest = cache_key_digest(key)?;
        let mut tx = self.pool.begin().await?;

        sqlx::query(queries::DELETE_ALL_TAGS_FOR_ENTRY)
            .bind(&key_digest)
            .execute(&mut *tx)
            .await?;

        queries::batch_upsert_tags(&mut tx, &key_digest, tags)
            .await
            .map_err(DbError::Database)?;

        tx.commit().await?;

        Ok(())
    }

    /// Remove specific tags from one cache entry.
    pub async fn remove_tags(&self, key: &CacheKey, tags: &[CacheTag]) -> DbResult<()> {
        let key_digest = cache_key_digest(key)?;

        for tag in tags {
            sqlx::query(queries::DELETE_SPECIFIC_TAG_FROM_ENTRY)
                .bind(&key_digest)
                .bind(tag.kind())
                .bind(tag.key())
                .execute(&self.pool)
                .await?;
        }

        Ok(())
    }

    /// Compatibility alias for older engine call sites.
    ///
    /// New code should prefer [`PostgresCache::remove_tags`].
    pub async fn untag(&self, key: &CacheKey, tags: &[CacheTag]) -> DbResult<()> {
        self.remove_tags(key, tags).await
    }

    /// Remove all tags from one cache entry.
    pub async fn clear_tags(&self, key: &CacheKey) -> DbResult<u64> {
        let key_digest = cache_key_digest(key)?;

        let result = sqlx::query(queries::DELETE_ALL_TAGS_FOR_ENTRY)
            .bind(&key_digest)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    /// Delete all cache entries associated with a specific tag.
    pub async fn delete_entries_by_tag(&self, tag: &CacheTag) -> DbResult<u64> {
        let result = sqlx::query(queries::DELETE_ENTRIES_BY_TAG)
            .bind(tag.kind())
            .bind(tag.key())
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    /// Delete all cache entries associated with any tag of the given kind.
    pub async fn delete_entries_by_tag_kind(&self, kind: &str) -> DbResult<u64> {
        let result = sqlx::query(queries::DELETE_ENTRIES_BY_TAG_KIND)
            .bind(kind)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    /// Remove one tag association from all entries while keeping entries.
    pub async fn remove_tag_from_all(&self, tag: &CacheTag) -> DbResult<u64> {
        let result = sqlx::query(queries::REMOVE_TAG_FROM_ALL)
            .bind(tag.kind())
            .bind(tag.key())
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    /// Remove every tag association of a given kind from all entries.
    pub async fn remove_tag_kind_from_all(&self, kind: &str) -> DbResult<u64> {
        let result = sqlx::query(queries::REMOVE_TAG_KIND_FROM_ALL)
            .bind(kind)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    /// List entries associated with a specific tag.
    pub async fn list_entries_by_tag(&self, tag: &CacheTag) -> DbResult<Vec<CacheEntryRef>> {
        let rows = sqlx::query(queries::LIST_ENTRIES_BY_TAG)
            .bind(tag.kind())
            .bind(tag.key())
            .fetch_all(&self.pool)
            .await?;

        rows_to_entry_refs(rows)
    }

    /// Compatibility alias for older call sites.
    ///
    /// New code should prefer [`PostgresCache::list_entries_by_tag`].
    pub async fn list_by_tag(&self, tag: &CacheTag) -> DbResult<Vec<CacheEntryRef>> {
        self.list_entries_by_tag(tag).await
    }

    /// List entries associated with any tag of a given kind.
    pub async fn list_entries_by_tag_kind(&self, kind: &str) -> DbResult<Vec<CacheEntryRef>> {
        let rows = sqlx::query(queries::LIST_ENTRIES_BY_TAG_KIND)
            .bind(kind)
            .fetch_all(&self.pool)
            .await?;

        rows_to_entry_refs(rows)
    }

    /// Compatibility alias for older call sites.
    ///
    /// New code should prefer [`PostgresCache::list_entries_by_tag_kind`].
    pub async fn list_by_tag_kind(&self, kind: &str) -> DbResult<Vec<CacheEntryRef>> {
        self.list_entries_by_tag_kind(kind).await
    }

    /// List known tags of a given kind.
    pub async fn list_tags_by_kind(&self, kind: &str) -> DbResult<Vec<CacheTag>> {
        let rows = sqlx::query(queries::LIST_TAGS_BY_KIND)
            .bind(kind)
            .fetch_all(&self.pool)
            .await?;

        rows_to_tags(rows)
    }

    /// List tags associated with one cache entry.
    pub async fn list_tags_for_entry(&self, key: &CacheKey) -> DbResult<Vec<CacheTag>> {
        let key_digest = cache_key_digest(key)?;

        let rows = sqlx::query(queries::LIST_TAGS_FOR_ENTRY)
            .bind(&key_digest)
            .fetch_all(&self.pool)
            .await?;

        rows_to_tags(rows)
    }
}

// ————————————————————————————————————————————————————————————————————————
// Auxiliary Storage
// ————————————————————————————————————————————————————————————————————————

impl PostgresCache {
    /// Get a JSON auxiliary value for one cache entry.
    pub async fn get_auxiliary(
        &self,
        key: &CacheKey,
        aux_key: &str,
    ) -> DbResult<Option<serde_json::Value>> {
        let key_digest = cache_key_digest(key)?;

        let row = sqlx::query(queries::SELECT_AUX_VALUE)
            .bind(&key_digest)
            .bind(aux_key)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(row) => {
                let value: serde_json::Value = row.try_get("value_json")?;
                Ok(Some(value))
            }

            None => Ok(None),
        }
    }

    /// Upsert a JSON auxiliary value for one cache entry.
    pub async fn put_auxiliary(
        &self,
        key: &CacheKey,
        aux_key: &str,
        value: &serde_json::Value,
    ) -> DbResult<()> {
        let key_digest = cache_key_digest(key)?;

        sqlx::query(queries::UPSERT_AUX)
            .bind(&key_digest)
            .bind(aux_key)
            .bind(value)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// List auxiliary keys stored for one cache entry.
    pub async fn list_auxiliary_keys(&self, key: &CacheKey) -> DbResult<Vec<String>> {
        let key_digest = cache_key_digest(key)?;

        let rows = sqlx::query(queries::LIST_AUX_KEYS)
            .bind(&key_digest)
            .fetch_all(&self.pool)
            .await?;

        let mut keys = Vec::with_capacity(rows.len());

        for row in rows {
            keys.push(row.try_get("aux_key")?);
        }

        Ok(keys)
    }

    /// Delete one auxiliary JSON value from a cache entry.
    pub async fn delete_auxiliary(&self, key: &CacheKey, aux_key: &str) -> DbResult<()> {
        let key_digest = cache_key_digest(key)?;

        sqlx::query(queries::DELETE_AUX)
            .bind(&key_digest)
            .bind(aux_key)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}

// ————————————————————————————————————————————————————————————————————————
// Row mapping and stored-data validation helpers
// ————————————————————————————————————————————————————————————————————————

fn rows_to_entry_refs(rows: Vec<PgRow>) -> DbResult<Vec<CacheEntryRef>> {
    let mut refs = Vec::with_capacity(rows.len());

    for row in rows {
        let key_json: serde_json::Value = row.try_get("key_json")?;

        let key: CacheKey = serde_json::from_value(key_json)
            .map_err(|error| DbError::json(error.to_string()))?;

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

fn rows_to_tags(rows: Vec<PgRow>) -> DbResult<Vec<CacheTag>> {
    let mut tags = Vec::with_capacity(rows.len());

    for row in rows {
        let kind: String = row.try_get("tag_kind")?;
        let key: String = row.try_get("tag_key")?;

        tags.push(CacheTag::raw(kind, key));
    }

    Ok(tags)
}

fn validate_loaded_metadata_for_key(
    expected_key: &CacheKey,
    metadata: &CacheEntryMetadata,
) -> DbResult<()> {
    if &metadata.cache_key != expected_key {
        return Err(DbError::invariant(format!(
            "metadata cache key does not match requested key: metadata={:?} requested={:?}",
            metadata.cache_key, expected_key
        )));
    }

    let key_url = metadata.cache_key.requested_url.as_str();
    let request_url = metadata.request.requested_url.as_str();

    if key_url != request_url {
        return Err(DbError::invariant(format!(
            "metadata request URL does not match cache key URL: request={} key={}",
            request_url, key_url
        )));
    }

    Ok(())
}

fn validate_payload_body(
    payload_id: &str,
    expected_sha256_hex: &str,
    expected_byte_len: usize,
    body: &[u8],
) -> DbResult<()> {
    if body.len() != expected_byte_len {
        return Err(DbError::invariant(format!(
            "payload byte length mismatch for {}: descriptor={} actual={}",
            payload_id,
            expected_byte_len,
            body.len()
        )));
    }

    let observed_sha256_hex = crate::model::sha256_hex(body);

    if observed_sha256_hex != expected_sha256_hex {
        return Err(DbError::invariant(format!(
            "payload checksum mismatch for {}",
            payload_id
        )));
    }

    Ok(())
}

fn validate_loaded_entry(entry: &CacheEntry) -> DbResult<()> {
    entry
        .validate()
        .map_err(|error| DbError::invariant(error.to_string()))
}

