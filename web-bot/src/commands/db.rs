//! Database administration commands.

use web_crawler_db::migrate_database_url;

#[derive(clap::Subcommand, Debug)]
pub enum DbCommands {
    /// Apply database schema migrations (idempotent)
    Migrate,
}

pub async fn run(action: DbCommands, database_url: &str) -> anyhow::Result<()> {
    match action {
        DbCommands::Migrate => migrate(database_url).await,
    }
}

async fn migrate(database_url: &str) -> anyhow::Result<()> {
    eprintln!("Applying database migrations to Postgres...");
    migrate_database_url(database_url).await?;
    eprintln!("✅ Migrations completed successfully.");
    Ok(())
}
