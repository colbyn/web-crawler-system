//! Explicit schema setup for the `web-crawler-db` artifact cache.
//!
//! Ordinary database connections must not create or migrate schema as a side
//! effect. Crawlers, workers, and services should be able to connect without
//! unexpectedly changing database structure.
//!
//! This module owns the explicit migration/setup path:
//!
//! ```text
//! connect -> use cache
//! migrate -> intentional admin/setup operation
//! ```
//!
//! The current implementation runs idempotent `CREATE TABLE IF NOT EXISTS` and
//! `CREATE INDEX IF NOT EXISTS` statements from `queries::schema`. That is good
//! enough for the crate's early schema bootstrap phase.
//!
//! Later, this module is the place to replace that bootstrap behavior with a
//! true versioned migration system, such as sqlx migrations, without changing
//! the runtime cache API.
//!
//! Intended callers:
//!
//! - a future `web-crawler-db migrate` CLI command,
//! - tests,
//! - local setup scripts,
//! - explicit administrative tooling.
//!
//! Non-callers:
//!
//! - `PostgresCache::connect()`,
//! - crawler hot paths,
//! - worker startup paths that should not mutate schema.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;

use crate::error::{DbError, DbResult};
use crate::queries;

/// Create or update the database schema using an existing Postgres pool.
///
/// This function is explicit by design. It is not called by
/// `PostgresCache::connect()`.
pub async fn migrate_pool(pool: &PgPool) -> DbResult<()> {
    use queries::schema::*;

    let statements = [
        CREATE_TABLE_CACHE_ENTRIES,
        CREATE_INDEX_REQUESTED_HOST,
        CREATE_INDEX_FINAL_HOST,
        CREATE_INDEX_STORED_AT,
        CREATE_INDEX_ENTRY_KIND,
        CREATE_TABLE_CACHE_PAYLOADS,
        CREATE_INDEX_PAYLOADS_KEY_DIGEST,
        CREATE_INDEX_PAYLOADS_ROLE,
        CREATE_TABLE_CACHE_TAGS,
        CREATE_INDEX_TAGS_KIND,
        CREATE_TABLE_CACHE_ENTRY_TAGS,
        CREATE_INDEX_ENTRY_TAGS_KEY_DIGEST,
        CREATE_INDEX_ENTRY_TAGS_KIND,
        CREATE_INDEX_ENTRY_TAGS_TAG,
        CREATE_TABLE_CACHE_AUXILIARY,
    ];

    for statement in statements {
        sqlx::query(statement).execute(pool).await?;
    }

    Ok(())
}

/// Connect to a database URL, run explicit schema setup, and close the pool when
/// the returned pool is dropped.
///
/// This helper is intended for CLI/admin setup flows.
pub async fn migrate_database_url(database_url: &str) -> DbResult<()> {
    let pool = connect_migration_pool(database_url).await?;
    migrate_pool(&pool).await?;
    Ok(())
}

async fn connect_migration_pool(database_url: &str) -> DbResult<PgPool> {
    let options: PgConnectOptions = database_url
        .parse()
        .map_err(|error| DbError::internal(format!("invalid database url: {error}")))?;

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect_with(options)
        .await?;

    Ok(pool)
}

