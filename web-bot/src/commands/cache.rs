//! Cache inspection and management commands.

use clap::Subcommand;
use std::path::PathBuf;

use web_crawler_engine_v3::cache::{CacheKey, CrawlCacheStore, FsCrawlCacheStore};

#[derive(Subcommand, Debug)]
pub enum CacheCommands {
    /// Show metadata for a cached URL
    Lookup {
        url: String,
        #[arg(long)]
        json: bool,

        /// Print the full artifact (including HTML body and anchors)
        #[arg(long)]
        full: bool,
    },

    /// Print or save the HTML snapshot
    Snapshot {
        url: String,
        /// Write to file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Remove a URL from the cache
    Remove {
        url: String,
        #[arg(short, long)]
        force: bool,
    },

    /// Clear the entire cache
    Clear {
        #[arg(short, long)]
        force: bool,
    },

    /// Show cache statistics
    Stats {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(action: CacheCommands, cache_root: &PathBuf) -> anyhow::Result<()> {
    let store = FsCrawlCacheStore::new(cache_root);

    match action {
        CacheCommands::Lookup { url, json, full } => {
            lookup_metadata(&store, &url, json, full).await?;
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

        CacheCommands::Stats { json } => {
            show_stats(cache_root, json).await?;
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────

async fn lookup_metadata(
    store: &FsCrawlCacheStore,
    url: &str,
    json: bool,
    full: bool,           // NEW: whether to print full artifact
) -> anyhow::Result<()> {
    let profile_key = web_browser_driver::BrowserProfileKey::new("default");
    let cache_key = CacheKey::for_request(
        url::Url::parse(url)?,
        profile_key,
        None,
    );

    match store.load(&cache_key).await {
        Ok(Some(artifact)) => {
            if json {
                if full {
                    // Full artifact (can be large)
                    if let Ok(json_str) = serde_json::to_string_pretty(&artifact) {
                        println!("{}", json_str);
                    }
                } else {
                    // Light metadata only (recommended default)
                    let mut artifact = serde_json::to_value(&artifact).unwrap();
                    if let Some(data) = artifact.pointer_mut("/snapshot/body") {
                        *data = serde_json::Value::Null;
                    }
                    // println!("{}", serde_json::to_string_pretty(&artifact)?);
                    println!("{}", colored_json::to_colored_json_auto(&artifact).unwrap());
                }
            } else {
                // Human readable output (to stderr)
                eprintln!("✅ Cache Hit: {}", url);
                eprintln!("  Final URL:      {}", artifact.resolution.final_url);
                eprintln!("  Snapshot size:  {} bytes", artifact.snapshot.body.len());
                eprintln!("  Anchors:        {}", artifact.extracted.anchors.len());

                if let Some(page_info) = &artifact.extracted.page_info {
                    if let Some(title) = &page_info.title {
                        eprintln!("  Title:          {}", title);
                    }
                }
            }
        }
        Ok(None) => {
            if json {
                println!("null");
            } else {
                eprintln!("❌ Not found in cache: {}", url);
            }
        }
        Err(e) => {
            eprintln!("Error loading from cache: {}", e);
        }
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
        eprintln!("Snapshot written to {}", path.display());
    } else {
        // Actual data → stdout
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
        eprint!("Remove {} from cache? [y/N]: ", url);
        use std::io::{self, Write};
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    let profile_key = web_browser_driver::BrowserProfileKey::new("default");
    let cache_key = CacheKey::for_request(
        url::Url::parse(url)?,
        profile_key,
        None,
    );

    let path = store.path_for_key(&cache_key)?;

    if path.exists() {
        std::fs::remove_file(&path)?;
        eprintln!("✅ Removed from cache: {}", url);
    } else {
        eprintln!("Not found in cache.");
    }

    Ok(())
}

async fn clear_cache(cache_root: &PathBuf, force: bool) -> anyhow::Result<()> {
    if !force {
        eprintln!("This will delete ALL data in: {}", cache_root.display());
        eprint!("Are you sure? [y/N]: ");
        use std::io::{self, Write};
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    let pages_dir = cache_root.join("pages");
    if pages_dir.exists() {
        std::fs::remove_dir_all(&pages_dir)?;
    }

    eprintln!("✅ Cache cleared.");
    Ok(())
}

async fn show_stats(cache_root: &PathBuf, json: bool) -> anyhow::Result<()> {
    let pages_dir = cache_root.join("pages");

    if !pages_dir.exists() {
        if json {
            println!(r#"{{"total_pages": 0, "total_size_bytes": 0}}"#);
        } else {
            eprintln!("No cached data found.");
        }
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

    if json {
        // Structured output → stdout
        println!(
            r#"{{"total_pages": {}, "total_size_bytes": {}}}"#,
            total_files, total_size
        );
    } else {
        // Human output → stderr
        eprintln!("Cache root: {}", cache_root.display());
        eprintln!("Total pages:  {}", total_files);
        eprintln!(
            "Disk usage:   {} bytes ({:.2} MB)",
            total_size,
            total_size as f64 / 1_048_576.0
        );
    }

    Ok(())
}
