//! Crawl command implementation.

use clap::Args;
use std::io::{self, BufRead};
use std::path::PathBuf;
use serde_json::Value;
use url::Url;
use web_crawler_engine_v3::{
    cache::FsCrawlCacheStore,
    config::{CrawlEngineConfig, CrawlLimits},
    input::CrawlRequest,
    policy::CrawlPolicy,
    scheduler::BrowserProfileStrategy,
    CrawlEngine,
};
use web_browser_driver::BrowserDriver;

#[derive(Args, Debug)]
pub struct CrawlArgs {
    /// URLs to crawl. Can be repeated. Use `-i -` to explicitly read from stdin.
    /// Each value can contain multiple space-separated URLs.
    #[arg(short = 'i', long = "input", action = clap::ArgAction::Append)]
    pub inputs: Vec<String>,

    /// Input format when reading from stdin or structured data
    #[arg(long, default_value = "text")]
    pub format: String,

    /// JSON Pointer to extract the URL (used with ndjson/json)
    #[arg(long)]
    pub url_pointer: Option<String>,

    /// Attach the original JSON object as provenance (ndjson/json only)
    #[arg(long)]
    pub attach_provenance: bool,

    /// Maximum pages to crawl in this run
    #[arg(long, default_value_t = 50)]
    pub max_pages: usize,

    /// Maximum hop depth from seeds
    #[arg(long, default_value_t = 1)]
    pub max_depth: u32,
}

/// Collect raw input lines from `-i` flags and/or stdin.
/// `-i -` forces reading from stdin.
fn collect_input_lines(args: &CrawlArgs) -> anyhow::Result<Vec<String>> {
    let mut lines = Vec::new();
    let mut read_stdin = false;

    for input in &args.inputs {
        if input == "-" {
            read_stdin = true;
        } else {
            for part in input.split_whitespace() {
                if !part.is_empty() {
                    lines.push(part.to_string());
                }
            }
        }
    }

    // Read stdin if `-i -` was used or if no -i flags were provided
    if read_stdin || args.inputs.is_empty() {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = line?;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }
        }
    }

    Ok(lines)
}

pub async fn run(
    args: CrawlArgs,
    profile_root: &PathBuf,
    cache_root: &PathBuf,
) -> anyhow::Result<()> {
    let raw_lines = collect_input_lines(&args)?;

    if raw_lines.is_empty() {
        println!("No URLs provided.");
        return Ok(());
    }

    // Parse raw lines into (URL, optional provenance)
    let mut parsed = Vec::new();

    for line in raw_lines {
        match args.format.as_str() {
            "text" => {
                if let Ok(url) = Url::parse(&line) {
                    parsed.push((url, None));
                } else {
                    eprintln!("Skipping invalid URL: {}", line);
                }
            }
            "ndjson" | "json" => {
                let json: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => {
                        eprintln!("Skipping invalid JSON line");
                        continue;
                    }
                };

                let url_str = if let Some(pointer) = &args.url_pointer {
                    match json.pointer(pointer).and_then(|v| v.as_str()) {
                        Some(u) => u.to_string(),
                        None => {
                            eprintln!("URL not found at pointer `{}`", pointer);
                            continue;
                        }
                    }
                } else {
                    match json.get("url").and_then(|v| v.as_str()) {
                        Some(u) => u.to_string(),
                        None => {
                            eprintln!("No `url` field found in JSON");
                            continue;
                        }
                    }
                };

                if let Ok(url) = Url::parse(&url_str) {
                    let provenance = if args.attach_provenance {
                        Some(json)
                    } else {
                        None
                    };
                    parsed.push((url, provenance));
                } else {
                    eprintln!("Invalid URL extracted: {}", url_str);
                }
            }
            other => {
                anyhow::bail!("Unsupported format: {}", other);
            }
        }
    }

    if parsed.is_empty() {
        println!("No valid URLs to crawl after parsing.");
        return Ok(());
    }

    println!("Crawling {} URLs...", parsed.len());

    let cache_store = FsCrawlCacheStore::new(cache_root);
    let driver = BrowserDriver::new(Default::default());

    let config = CrawlEngineConfig {
        limits: CrawlLimits {
            max_pages: args.max_pages,
            max_hop_depth: args.max_depth,
            max_frontier_items: 10_000,
        },
        page_open_timeout: std::time::Duration::from_secs(45),
        cache_enabled: true,
    };

    let policy = CrawlPolicy::default();
    let profile_strategy = BrowserProfileStrategy::default();

    let engine: CrawlEngine<serde_json::Value> = CrawlEngine::new(
        config,
        policy,
        driver,
        Some(cache_store),
        profile_root.clone(),
        profile_strategy,
    );

    let mut requests = Vec::new();
    for (url, provenance) in parsed {
        let prov = provenance.unwrap_or_else(|| serde_json::json!({}));
        requests.push(CrawlRequest::seed(url, prov));
    }

    let result = engine.crawl(requests).await?;

    // Summary
    let mut success = 0;
    let mut failed = 0;

    for page in &result.pages {
        match &page.outcome {
            web_crawler_engine_v3::output::CrawlPageOutcome::Opened { .. } => success += 1,
            web_crawler_engine_v3::output::CrawlPageOutcome::Failed { .. } => failed += 1,
            _ => {}
        }
    }

    println!("\n=== Crawl Complete ===");
    println!("Total processed: {}", result.pages.len());
    println!("  Successful:    {}", success);
    println!("  Failed:        {}", failed);

    Ok(())
}
