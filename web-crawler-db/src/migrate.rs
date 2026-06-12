//! Explicit schema setup for the `web-crawler-db` artifact cache.
//!
//! This module owns the **intentional** migration path.
//!
//! Normal cache connections must never mutate schema as a side effect.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use crate::error::{DbError, DbResult};
use crate::queries;

/// Apply the full schema to an existing pool.
///
/// Uses `raw_sql` so we can execute the entire `sql/schema.sql` file
/// (multiple statements) in one go.
pub async fn migrate_pool(pool: &PgPool) -> DbResult<()> {
    sqlx::raw_sql(queries::SCHEMA)
        .execute(pool)
        .await?;
    Ok(())
}

/// Connect to a database URL, apply schema, and return a pool.
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
