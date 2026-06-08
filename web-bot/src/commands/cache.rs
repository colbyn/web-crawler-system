//! Cache inspection and management commands.

use clap::Subcommand;
use std::path::PathBuf;

use web_crawler_engine_v3::{
    cache::{CacheKey, CrawlCacheStore, FsCrawlCacheStore},
};

#[derive(Subcommand, Debug)]
pub enum CacheCommands {
    /// Look up whether a URL exists in the cache
    Lookup {
        /// URL to look up
        url: String,
    },

    /// Show basic statistics about the cache
    Stats,
}

pub async fn run(action: CacheCommands, cache_root: &PathBuf) -> anyhow::Result<()> {
    let store = FsCrawlCacheStore::new(cache_root);

    match action {
        CacheCommands::Lookup { url } => {
            println!("Looking up URL in cache...");
            println!("URL: {}", url);

            let profile_key = web_browser_driver::BrowserProfileKey::new("default");
            let cache_key = CacheKey::for_request(
                url::Url::parse(&url)?,
                profile_key,
                None,
            );

            match store.load(&cache_key).await {
                Ok(Some(artifact)) => {
                    println!("\n✅ Found in cache");
                    println!("  Stored at:     {}", artifact.stored_at_unix_ms);
                    println!("  Final URL:     {}", artifact.resolution.final_url);
                    println!("  Snapshot size: {} bytes", artifact.snapshot.body.len());
                    println!(
                        "  Has page info: {}",
                        artifact.extracted.page_info.is_some()
                    );
                    println!("  Anchor count:  {}", artifact.extracted.anchors.len());
                }
                Ok(None) => {
                    println!("\n❌ Not found in cache");
                }
                Err(e) => {
                    println!("\n⚠️  Error loading from cache: {}", e);
                }
            }
        }

        CacheCommands::Stats => {
            println!("Cache Statistics");
            println!("Cache root: {}", cache_root.display());

            let pages_dir = cache_root.join("pages");

            if !pages_dir.exists() {
                println!("No cache data found yet.");
                return Ok(());
            }

            let mut total_files = 0usize;
            let mut total_size: u64 = 0;

            for entry in walkdir::WalkDir::new(&pages_dir)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    total_files += 1;
                    if let Ok(metadata) = entry.metadata() {
                        total_size += metadata.len();
                    }
                }
            }

            println!("\nTotal cached pages: {}", total_files);
            println!(
                "Total size on disk: {} bytes ({:.2} MB)",
                total_size,
                total_size as f64 / 1_048_576.0
            );
        }
    }

    Ok(())
}
