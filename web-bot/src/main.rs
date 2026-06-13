//! web-bot — Operational CLI for the web crawler system.
//!
//! Provides commands to crawl content into a shared SQLite cache and inspect
//! cached artifacts.
//!
//! The crawl command supports layered configuration:
//!
//! ```text
//! explicit CLI argument > TOML config file > hardcoded default
//! ```
//!
//! Crawl flags use `Option<T>` where needed, so the crawl module can distinguish
//! user-provided overrides from defaults without keeping raw Clap matches alive.

use clap::{
    Parser,
    Subcommand,
    ValueEnum,
};
use std::path::PathBuf;

mod commands;

#[derive(Parser)]
#[command(name = "web-bot", version, about = "Web crawler operations tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Directory where browser profiles are stored
    #[arg(long, default_value = ".output/web-bot/profiles")]
    profile_root: PathBuf,

    /// Postgres connection URL.
    /// Falls back to DATABASE_URL environment variable if not provided.
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    Crawl(commands::crawl::CrawlArgs),
    Cache {
        #[command(subcommand)]
        action: commands::cache::CacheCommands,
    },
    /// Database administration commands
    Db {
        #[command(subcommand)]
        action: commands::db::DbCommands,
    },
    Doc {
        #[arg(long)]
        r#type: SchemaType,
    }
}

/// Shared sort direction for list-style commands.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaType {
    #[value(alias("ExtractedAnchor"))]
    ExtractedAnchor,
    #[value(alias("PageInfo"))]
    PageInfo,
    #[value(alias("CacheEntryMetadata"))]
    CacheEntryMetadata,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Resolve database URL (CLI flag > env var)
    let database_url = cli.database_url.clone().or_else(|| {
        std::env::var("DATABASE_URL").ok()
    });

    match cli.command {
        Commands::Crawl(args) => {
            let db_url = database_url.as_deref()
                .ok_or_else(|| anyhow::anyhow!(
                    "No database URL provided. Use --database-url or set DATABASE_URL"
                ))?;
            commands::crawl::run(args, &cli.profile_root, db_url).await?;
        }

        Commands::Cache { action } => {
            let db_url = database_url.as_deref()
                .ok_or_else(|| anyhow::anyhow!(
                    "No database URL provided. Use --database-url or set DATABASE_URL"
                ))?;
            commands::cache::run(action, db_url).await?;
        }

        Commands::Db { action } => {
            let db_url = database_url.as_deref()
                .ok_or_else(|| anyhow::anyhow!(
                    "No database URL provided. Use --database-url or set DATABASE_URL"
                ))?;
            commands::db::run(action, db_url).await?
        }
        Commands::Doc { r#type } => {
            let schema = match r#type {
                SchemaType::ExtractedAnchor => {
                    schemars::schema_for!(web_browser_driver::ExtractedAnchor)
                }
                SchemaType::PageInfo => {
                    schemars::schema_for!(web_browser_driver::PageInfo)
                }
                SchemaType::CacheEntryMetadata => {
                    schemars::schema_for!(web_crawler_db::CacheEntryMetadata)
                }
            };
            let schema = serde_json::to_string_pretty(&schema).unwrap();
            println!("{schema}");
        }
    }

    Ok(())
}
