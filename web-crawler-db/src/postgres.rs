//! PostgreSQL implementation of the `web-crawler-db` artifact cache.
//!
//! `PostgresCache` is the runtime SDK for durable crawler artifacts.
//!
//! This module intentionally does not create or migrate schema as a side effect
//! of connecting. Schema setup belongs to explicit administrative paths in
//! `migrate.rs`.
//!
//! The cache model is deliberately small:
//!
//! ```text
//! CacheKey -> CacheEntry
//!
//! CacheEntry = metadata + one primary payload body + tags
//! ```
//!
//! Metadata and payload bytes are stored separately so hot replay paths can read
//! page metadata without pulling large `BYTEA` values through Postgres.
//!
//! Tags are secondary-index associations. They are merged idempotently by
//! default so crawler phases can attach new associations without erasing earlier
//! ones.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::error::{DbError, DbResult};
use crate::key::{cache_key_digest, CacheKey};
use crate::model::{
    CacheEntry, CacheEntryMetadata, CacheEntryRef, CachePayload, CachePayloadCompression,
    CachePayloadDescriptor, CacheTag,
};
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
    /// This is useful when an application already owns a Postgres pool and wants
    /// the cache SDK to share it.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Return the underlying sqlx pool for advanced usage.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

// -----------------------------------------------------------------------------
// Read API
// -----------------------------------------------------------------------------

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
    /// Loads metadata, the single primary payload body, and tags.
    pub async fn try_get_full_entry(&self, key: &CacheKey) -> DbResult<Option<CacheEntry>> {
        self.load_entry_raw(key).await
    }

    /// Read only metadata for a cache key.
    ///
    /// This is the preferred hot replay path when the caller only needs
    /// extracted page data and provenance.
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

    /// Read the single primary payload body for a cache key.
    ///
    /// Returns `Ok(None)` when the cache entry itself does not exist.
    ///
    /// Returns an invariant error when metadata exists but the payload row is
    /// missing. That means the stored entry is incomplete.
    pub async fn get_payload(&self, key: &CacheKey) -> DbResult<Option<CachePayload>> {
        let key_digest = cache_key_digest(key)?;

        let Some(_metadata) = self.get_metadata(key).await? else {
            return Ok(None);
        };

        let Some(payload) = self.load_payload_for_digest(&key_digest).await? else {
            return Err(DbError::invariant(
                "metadata row exists but payload row is missing",
            ));
        };

        Ok(Some(payload))
    }

    async fn load_entry_raw(&self, key: &CacheKey) -> DbResult<Option<CacheEntry>> {
        let key_digest = cache_key_digest(key)?;

        let Some(metadata) = self.get_metadata(key).await? else {
            return Ok(None);
        };

        let Some(payload) = self.load_payload_for_digest(&key_digest).await? else {
            return Err(DbError::invariant(
                "metadata row exists but payload row is missing",
            ));
        };

        let tags = self.load_tags_for_digest(&key_digest).await?;

        let entry = CacheEntry {
            metadata,
            payload,
            tags,
        };

        validate_loaded_entry(&entry)?;

        Ok(Some(entry))
    }

    async fn load_payload_for_digest(&self, key_digest: &str) -> DbResult<Option<CachePayload>> {
        let row = sqlx::query(queries::SELECT_PAYLOAD_FOR_KEY)
            .bind(key_digest)
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(row_to_payload(row)?))
    }

    async fn load_tags_for_digest(&self, key_digest: &str) -> DbResult<Vec<CacheTag>> {
        let rows = sqlx::query(queries::SELECT_TAGS_FOR_ENTRY)
            .bind(key_digest)
            .fetch_all(&self.pool)
            .await?;

        rows_to_tags(rows)
    }
}

// -----------------------------------------------------------------------------
// Write API
// -----------------------------------------------------------------------------

impl PostgresCache {
    /// Store or update a complete cache entry.
    ///
    /// Semantics:
    ///
    /// - validates metadata and payload before writing,
    /// - upserts the metadata row,
    /// - upserts/replaces the single primary payload row,
    /// - merges tags idempotently,
    /// - does not remove existing tags.
    ///
    /// Use [`PostgresCache::replace_tags`] when the caller wants exact tag
    /// replacement.
    pub async fn put(&self, entry: &CacheEntry) -> DbResult<()> {
        entry.validate()?;

        let key_digest = cache_key_digest(entry.key())?;

        let mut tx = self.pool.begin().await?;

        upsert_metadata_in_tx(&mut tx, &key_digest, &entry.metadata).await?;
        upsert_payload_in_tx(&mut tx, &key_digest, &entry.payload).await?;

        queries::batch_upsert_tags(&mut tx, &key_digest, &entry.tags)
            .await
            .map_err(DbError::Database)?;

        tx.commit().await?;

        Ok(())
    }

    /// Upsert metadata only.
    ///
    /// This supports incremental enrichment phases that improve page metadata
    /// without touching the stored payload bytes or tags.
    pub async fn put_metadata(&self, metadata: &CacheEntryMetadata) -> DbResult<()> {
        metadata.validate()?;

        let key_digest = cache_key_digest(metadata.key())?;

        let mut tx = self.pool.begin().await?;

        upsert_metadata_in_tx(&mut tx, &key_digest, metadata).await?;

        tx.commit().await?;

        Ok(())
    }

    /// Upsert the single primary payload only.
    ///
    /// The metadata row must already exist because `cache_payloads.key_digest`
    /// references `cache_entries.key_digest`.
    pub async fn put_payload(&self, key: &CacheKey, payload: &CachePayload) -> DbResult<()> {
        payload.validate()?;

        let key_digest = cache_key_digest(key)?;

        let mut tx = self.pool.begin().await?;

        upsert_payload_in_tx(&mut tx, &key_digest, payload).await?;

        tx.commit().await?;

        Ok(())
    }

    /// Delete the primary payload row for an entry.
    ///
    /// This leaves metadata and tags intact. A later full-entry read will report
    /// the entry as incomplete until a payload is written again.
    pub async fn delete_payload(&self, key: &CacheKey) -> DbResult<u64> {
        let key_digest = cache_key_digest(key)?;

        let result = sqlx::query(queries::DELETE_PAYLOAD_FOR_KEY)
            .bind(&key_digest)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }
}

// -----------------------------------------------------------------------------
// Tag operations
// -----------------------------------------------------------------------------

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

        self.load_tags_for_digest(&key_digest).await
    }
}

// -----------------------------------------------------------------------------
// Auxiliary storage
// -----------------------------------------------------------------------------

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

// -----------------------------------------------------------------------------
// Transaction helpers
// -----------------------------------------------------------------------------

async fn upsert_metadata_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    key_digest: &str,
    metadata: &CacheEntryMetadata,
) -> DbResult<()> {
    metadata.validate()?;

    let key_json =
        serde_json::to_value(metadata.key()).map_err(|error| DbError::json(error.to_string()))?;

    let metadata_json =
        serde_json::to_value(metadata).map_err(|error| DbError::json(error.to_string()))?;

    let capture_policy_json = match &metadata.request.capture_policy {
        Some(policy) => {
            serde_json::to_value(policy).map_err(|error| DbError::json(error.to_string()))?
        }
        None => serde_json::Value::Null,
    };

    sqlx::query(queries::UPSERT_CACHE_ENTRY)
        .bind(key_digest)
        .bind(&key_json)
        .bind(metadata.metadata_version as i32)
        .bind(&metadata.entry_kind)
        .bind(&metadata.request.requested_url)
        .bind(&metadata.request.requested_host)
        .bind(&metadata.response.final_url)
        .bind(&metadata.response.final_host)
        .bind(&capture_policy_json)
        .bind(metadata.stored_at_unix_ms)
        .bind(metadata.response.status_code.map(i32::from))
        .bind(&metadata.response.content_type)
        .bind(&metadata_json)
        .execute(&mut **tx)
        .await?;

    Ok(())
}

async fn upsert_payload_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    key_digest: &str,
    payload: &CachePayload,
) -> DbResult<()> {
    payload.validate()?;

    sqlx::query(queries::UPSERT_PAYLOAD)
        .bind(key_digest)
        .bind(&payload.descriptor.media_type)
        .bind(payload.descriptor.compression.as_db_str())
        .bind(&payload.descriptor.sha256_hex)
        .bind(payload.descriptor.byte_len as i64)
        .bind(&payload.body)
        .execute(&mut **tx)
        .await?;

    Ok(())
}

// -----------------------------------------------------------------------------
// Row mapping and stored-data validation helpers
// -----------------------------------------------------------------------------

fn rows_to_entry_refs(rows: Vec<PgRow>) -> DbResult<Vec<CacheEntryRef>> {
    let mut refs = Vec::with_capacity(rows.len());

    for row in rows {
        let key_json: serde_json::Value = row.try_get("key_json")?;

        let key: CacheKey =
            serde_json::from_value(key_json).map_err(|error| DbError::json(error.to_string()))?;

        let metadata_version: i32 = row.try_get("metadata_version")?;
        let status_code: Option<i32> = row.try_get("status_code")?;

        refs.push(CacheEntryRef {
            key_digest: row.try_get("key_digest")?,
            key,
            requested_url: row.try_get("requested_url")?,
            final_url: row.try_get("final_url")?,
            stored_at_unix_ms: row.try_get("stored_at_unix_ms")?,
            entry_kind: row.try_get("entry_kind")?,
            metadata_version: u32::try_from(metadata_version).map_err(|_| {
                DbError::invariant(format!("invalid metadata_version: {metadata_version}"))
            })?,
            status_code: status_code
                .map(u16::try_from)
                .transpose()
                .map_err(|_| DbError::invariant("invalid status_code"))?,
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

fn row_to_payload(row: PgRow) -> DbResult<CachePayload> {
    let compression: String = row.try_get("compression")?;
    let byte_len: i64 = row.try_get("byte_len")?;

    let descriptor = CachePayloadDescriptor {
        media_type: row.try_get("media_type")?,
        compression: CachePayloadCompression::from_db_str(&compression)?,
        sha256_hex: row.try_get("sha256_hex")?,
        byte_len: usize::try_from(byte_len)
            .map_err(|_| DbError::invariant(format!("invalid payload byte_len: {byte_len}")))?,
    };

    let body: Vec<u8> = row.try_get("body")?;

    let payload = CachePayload { descriptor, body };

    validate_loaded_payload(&payload)?;

    Ok(payload)
}

fn validate_loaded_metadata_for_key(
    expected_key: &CacheKey,
    metadata: &CacheEntryMetadata,
) -> DbResult<()> {
    if metadata.key() != expected_key {
        return Err(DbError::invariant(format!(
            "metadata cache key does not match requested key: metadata={:?} requested={:?}",
            metadata.key(),
            expected_key
        )));
    }

    validate_loaded_metadata(metadata)
}

fn validate_loaded_metadata(metadata: &CacheEntryMetadata) -> DbResult<()> {
    metadata.validate().map_err(invalid_entry_to_invariant)
}

fn validate_loaded_payload(payload: &CachePayload) -> DbResult<()> {
    payload.validate().map_err(invalid_entry_to_invariant)
}

fn validate_loaded_entry(entry: &CacheEntry) -> DbResult<()> {
    entry.validate().map_err(invalid_entry_to_invariant)
}

fn invalid_entry_to_invariant(error: DbError) -> DbError {
    match error {
        DbError::InvalidEntry(message) => DbError::invariant(message),
        other => other,
    }
}

