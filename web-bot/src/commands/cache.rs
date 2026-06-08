//! Cache inspection and management commands.

use clap::Subcommand;
use std::path::PathBuf;

use web_crawler_engine_v3::cache::{CacheKey, CrawlCacheStore, FsCrawlCacheStore};

#[derive(Subcommand, Debug)]
pub enum CacheCommands {
    /// Show metadata for a cached URL
    Lookup {
        url: String,
    },

    /// Print or save the HTML snapshot of a cached page
    Snapshot {
        url: String,
        /// Write to file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Remove a specific URL from the cache
    Remove {
        url: String,
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },

    /// Delete all cached data (use with caution)
    Clear {
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },

    /// Show basic cache statistics
    Stats,
}

pub async fn run(action: CacheCommands, cache_root: &PathBuf) -> anyhow::Result<()> {
    let store = FsCrawlCacheStore::new(cache_root);

    match action {
        CacheCommands::Lookup { url } => {
            lookup_metadata(&store, &url).await?;
        }

        CacheCommands::Snapshot { url, output } => {
            get_snapshot(&store, &url, output).await?;
        }

        CacheCommands::Remove { url, force } => {
            remove_url(&store, &url, force).await?;
        }

        CacheCommands::Clear { force } => {
            clear_cache(cache_root, force).await?;
        }

        CacheCommands::Stats => {
            show_stats(cache_root).await?;
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────

async fn lookup_metadata(
    store: &FsCrawlCacheStore,
    url: &str,
) -> anyhow::Result<()> {
    println!("Looking up: {}", url);

    let profile_key = web_browser_driver::BrowserProfileKey::new("default");
    let cache_key = CacheKey::for_request(
        url::Url::parse(url)?,
        profile_key,
        None,
    );

    match store.load(&cache_key).await {
        Ok(Some(artifact)) => {
            println!("\n✅ Cache Hit");
            println!("  Stored at (unix ms): {}", artifact.stored_at_unix_ms);
            println!("  Requested URL:       {}", artifact.resolution.requested_url);
            println!("  Final URL:           {}", artifact.resolution.final_url);
            println!("  Was redirected:      {}", artifact.resolution.was_redirected());
            println!("  Status code:         {:?}", artifact.status_code);
            println!("  Snapshot size:       {} bytes", artifact.snapshot.body.len());
            println!("  Compression:         {:?}", artifact.snapshot.compression);

            if let Some(page_info) = &artifact.extracted.page_info {
                println!("  Title:               {:?}", page_info.title);
            }
            println!("  Anchors found:       {}", artifact.extracted.anchors.len());
        }
        Ok(None) => println!("\n❌ Not found in cache"),
        Err(e) => eprintln!("Error: {}", e),
    }

    Ok(())
}

async fn get_snapshot(
    store: &FsCrawlCacheStore,
    url: &str,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let profile_key = web_browser_driver::BrowserProfileKey::new("default");
    let cache_key = CacheKey::for_request(
        url::Url::parse(url)?,
        profile_key,
        None,
    );

    let artifact = match store.load(&cache_key).await? {
        Some(a) => a,
        None => anyhow::bail!("URL not found in cache: {}", url),
    };

    let html = match artifact.snapshot.compression {
        web_crawler_engine_v3::cache::SnapshotCompression::None => {
            String::from_utf8_lossy(&artifact.snapshot.body).to_string()
        }
        other => anyhow::bail!("Unsupported compression: {:?}", other),
    };

    if let Some(path) = output {
        std::fs::write(&path, &html)?;
        println!("Snapshot written to {}", path.display());
    } else {
        println!("{}", html);
    }

    Ok(())
}

async fn remove_url(
    store: &FsCrawlCacheStore,
    url: &str,
    force: bool,
) -> anyhow::Result<()> {
    if !force {
        print!("Remove {} from cache? [y/N]: ", url);
        use std::io::{self, Write};
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let profile_key = web_browser_driver::BrowserProfileKey::new("default");
    let cache_key = CacheKey::for_request(
        url::Url::parse(url)?,
        profile_key,
        None,
    );

    // We don't have a direct delete API yet, so we delete the file manually
    let path = store.path_for_key(&cache_key)?;

    if path.exists() {
        std::fs::remove_file(&path)?;
        println!("✅ Removed from cache: {}", url);
    } else {
        println!("Not found in cache.");
    }

    Ok(())
}

async fn clear_cache(cache_root: &PathBuf, force: bool) -> anyhow::Result<()> {
    if !force {
        println!("This will delete ALL cached data in: {}", cache_root.display());
        print!("Are you sure? [y/N]: ");
        use std::io::{self, Write};
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let pages_dir = cache_root.join("pages");
    if pages_dir.exists() {
        std::fs::remove_dir_all(&pages_dir)?;
    }

    println!("✅ Cache cleared.");
    Ok(())
}

async fn show_stats(cache_root: &PathBuf) -> anyhow::Result<()> {
    println!("Cache root: {}", cache_root.display());

    let pages_dir = cache_root.join("pages");
    if !pages_dir.exists() {
        println!("No cached data.");
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
            if let Ok(m) = entry.metadata() {
                total_size += m.len();
            }
        }
    }

    println!("Total pages:  {}", total_files);
    println!(
        "Disk usage:   {} bytes ({:.2} MB)",
        total_size,
        total_size as f64 / 1_048_576.0
    );

    Ok(())
}

