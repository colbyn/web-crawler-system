//! web-bot — Operational CLI for the web crawler system.
//!
//! Provides commands to crawl content into a shared cache and inspect
//! cached artifacts.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod commands;

#[derive(Parser)]
#[command(
    name = "web-bot",
    version,
    about = "Web crawler operations tool",
    long_about = "A CLI for preemptively crawling content and inspecting the shared cache."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Directory where browser profiles are stored
    #[arg(long, default_value = "./profiles")]
    profile_root: PathBuf,

    /// Directory where cached page artifacts are stored
    #[arg(long, default_value = "./cache")]
    cache_root: PathBuf,
}

#[derive(Subcommand)]
enum Commands {
    /// Crawl one or more URLs into the cache
    Crawl(commands::crawl::CrawlArgs),

    /// Inspect and manage the page cache
    Cache {
        #[command(subcommand)]
        action: commands::cache::CacheCommands,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing (respects RUST_LOG)
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Crawl(args) => {
            commands::crawl::run(args, &cli.profile_root, &cli.cache_root).await?;
        }
        Commands::Cache { action } => {
            commands::cache::run(action, &cli.cache_root).await?;
        }
    }

    Ok(())
}

