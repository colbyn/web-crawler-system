-- ============================================================================
-- web-crawler-db Schema
-- ============================================================================
--
-- PostgreSQL schema for the `web-crawler-db` artifact cache.
--
-- This file is intentionally pure SQL.
--
-- It can be executed by:
--
--   psql -v ON_ERROR_STOP=1 -f sql/schema.sql -d your_database
--
-- It can also be embedded and executed by sqlx, for example:
--
--   sqlx::raw_sql(include_str!("../sql/schema.sql")).execute(pool).await?;
--
-- Do not add psql-only metacommands such as `\echo` to this file. If a richer
-- psql wrapper is desired, create a separate wrapper script that includes this
-- schema.
--
-- Design summary:
--
--   cache_entries
--     One logical cached artifact entry. For the current cache model, this is
--     effectively one requested URL / page artifact identity.
--
--   cache_payloads
--     One physical payload body for each cache entry. The payload is kept in a
--     separate table so metadata-only reads do not pull large BYTEA values.
--
--     Multi-payload support is intentionally not modeled here. If screenshots,
--     raw bodies, rendered HTML, network logs, or other independent artifacts
--     are needed later, this table can be migrated to add `payload_id` and a
--     composite primary key.
--
--   cache_tags
--     Registry of known tag pairs `(tag_kind, tag_key)`.
--
--   cache_entry_tags
--     Many-to-many associations between cache entries and tags. Tags are
--     secondary indexes / caller-owned associations. They are not cache
--     identity and are expected to be merged idempotently over time.
--
--   cache_auxiliary
--     Small JSON sidecars associated with an entry. This is for extension data
--     that should be queryable/editable separately from the main metadata blob.
--
-- ============================================================================

BEGIN;

-- cache_entries --------------------------------------------------------------
--
-- One row per logical cached artifact.
--
-- `key_digest` is the compact stable primary key used for joins.
--
-- `key_json` stores the original logical key for inspection, debugging, and
-- future migrations.
--
-- `metadata_json` stores the full versioned Rust metadata shape. Queryable
-- summary columns are duplicated here intentionally so common administrative
-- queries do not need to inspect JSONB.
--
-- `capture_policy_json` is JSONB rather than nullable JSONB. When no capture
-- policy exists, store JSON null (`'null'::jsonb`) rather than SQL NULL.

CREATE TABLE IF NOT EXISTS cache_entries (
    key_digest TEXT PRIMARY KEY
        CHECK (btrim(key_digest) <> ''),

    key_json JSONB NOT NULL,

    metadata_version INTEGER NOT NULL
        CHECK (metadata_version > 0),

    entry_kind TEXT NOT NULL
        CHECK (btrim(entry_kind) <> ''),

    requested_url TEXT NOT NULL
        CHECK (btrim(requested_url) <> ''),

    requested_host TEXT
        CHECK (requested_host IS NULL OR btrim(requested_host) <> ''),

    final_url TEXT
        CHECK (final_url IS NULL OR btrim(final_url) <> ''),

    final_host TEXT
        CHECK (final_host IS NULL OR btrim(final_host) <> ''),

    capture_policy_json JSONB NOT NULL
        DEFAULT 'null'::jsonb,

    stored_at_unix_ms BIGINT NOT NULL
        CHECK (stored_at_unix_ms >= 0),

    status_code INTEGER
        CHECK (status_code IS NULL OR status_code BETWEEN 100 AND 599),

    content_type TEXT
        CHECK (content_type IS NULL OR btrim(content_type) <> ''),

    metadata_json JSONB NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cache_entries_requested_host
    ON cache_entries(requested_host);

CREATE INDEX IF NOT EXISTS idx_cache_entries_final_host
    ON cache_entries(final_host);

CREATE INDEX IF NOT EXISTS idx_cache_entries_stored_at
    ON cache_entries(stored_at_unix_ms);

CREATE INDEX IF NOT EXISTS idx_cache_entries_entry_kind
    ON cache_entries(entry_kind);

CREATE INDEX IF NOT EXISTS idx_cache_entries_status_code
    ON cache_entries(status_code);

-- cache_payloads -------------------------------------------------------------
--
-- One payload row per cache entry.
--
-- This deliberately does not support multiple payloads yet. The current cache
-- stores one primary artifact body per entry.
--
-- Payload bytes live outside `cache_entries` so callers can read metadata
-- without loading the body.
--
-- `sha256_hex` and `byte_len` describe the stored bytes exactly as stored. If
-- compression is not `none`, these fields describe the compressed bytes.

CREATE TABLE IF NOT EXISTS cache_payloads (
    key_digest TEXT PRIMARY KEY
        REFERENCES cache_entries(key_digest)
        ON DELETE CASCADE,

    media_type TEXT
        CHECK (media_type IS NULL OR btrim(media_type) <> ''),

    compression TEXT NOT NULL
        CHECK (compression IN ('none', 'gzip', 'zstd')),

    sha256_hex TEXT NOT NULL
        CHECK (sha256_hex ~ '^[0-9a-fA-F]{64}$'),

    byte_len BIGINT NOT NULL
        CHECK (byte_len >= 0),

    body BYTEA NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cache_payloads_compression
    ON cache_payloads(compression);

CREATE INDEX IF NOT EXISTS idx_cache_payloads_media_type
    ON cache_payloads(media_type);

-- cache_tags -----------------------------------------------------------------
--
-- Registry of known tag identities.
--
-- Tags are canonicalized as `(tag_kind, tag_key)`.
--
-- Examples:
--
--   entity.place_id : ChIJ...
--   category.id     : software_company
--   crawl.batch     : 2026-06-11-utah
--   source.dataset  : google_places
--   host            : example.com
--
-- Tags should remain lightweight associations. Rich structured facts belong in
-- metadata JSON, auxiliary JSON, or a higher-level application table.

CREATE TABLE IF NOT EXISTS cache_tags (
    tag_kind TEXT NOT NULL
        CHECK (btrim(tag_kind) <> ''),

    tag_key TEXT NOT NULL
        CHECK (btrim(tag_key) <> ''),

    PRIMARY KEY (tag_kind, tag_key)
);

CREATE INDEX IF NOT EXISTS idx_cache_tags_kind
    ON cache_tags(tag_kind);

-- cache_entry_tags -----------------------------------------------------------
--
-- Many-to-many join table between entries and tags.
--
-- Existing tag associations are expected to be merged idempotently:
--
--   INSERT ... ON CONFLICT DO NOTHING
--
-- This supports incremental crawler phases. For example, an early prefetch
-- phase can attach source/batch tags and a later enrichment phase can attach
-- extraction/category/entity tags without erasing earlier associations.

CREATE TABLE IF NOT EXISTS cache_entry_tags (
    key_digest TEXT NOT NULL
        REFERENCES cache_entries(key_digest)
        ON DELETE CASCADE,

    tag_kind TEXT NOT NULL,

    tag_key TEXT NOT NULL,

    PRIMARY KEY (key_digest, tag_kind, tag_key),

    FOREIGN KEY (tag_kind, tag_key)
        REFERENCES cache_tags(tag_kind, tag_key)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_cache_entry_tags_tag
    ON cache_entry_tags(tag_kind, tag_key, key_digest);

CREATE INDEX IF NOT EXISTS idx_cache_entry_tags_kind
    ON cache_entry_tags(tag_kind);

-- cache_auxiliary ------------------------------------------------------------
--
-- Small JSON sidecar values for a cache entry.
--
-- This table is useful for optional derived data, temporary inspection notes,
-- migration breadcrumbs, diagnostics, or feature-specific data that should not
-- force a metadata schema bump.
--
-- Keep large binary/text artifacts out of this table. Use the primary payload,
-- or introduce explicit multi-payload support later.

CREATE TABLE IF NOT EXISTS cache_auxiliary (
    key_digest TEXT NOT NULL
        REFERENCES cache_entries(key_digest)
        ON DELETE CASCADE,

    aux_key TEXT NOT NULL
        CHECK (btrim(aux_key) <> ''),

    value_json JSONB NOT NULL,

    PRIMARY KEY (key_digest, aux_key)
);

CREATE INDEX IF NOT EXISTS idx_cache_auxiliary_aux_key
    ON cache_auxiliary(aux_key);

COMMIT;

