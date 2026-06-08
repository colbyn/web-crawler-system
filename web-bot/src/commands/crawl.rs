//! Crawl command implementation.

use clap::Args;
use std::io::{self, BufRead};
use std::path::PathBuf;

use anyhow::Context;
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
    /// URLs to crawl (can also be provided via stdin)
    #[arg()]
    pub urls: Vec<String>,

    /// Input format: text, ndjson, or json
    #[arg(long, default_value = "text")]
    pub format: String,

    /// JSON Pointer to extract the URL when using ndjson/json input
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

/// Parse input from stdin based on format
fn parse_input(args: &CrawlArgs) -> anyhow::Result<Vec<(Url, Option<Value>)>> {
    let mut results = Vec::new();
    let stdin = io::stdin();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match args.format.as_str() {
            "text" => {
                let url = Url::parse(line).context("Invalid URL")?;
                results.push((url, None));
            }
            "ndjson" | "json" => {
                let json: Value =
                    serde_json::from_str(&line).context("Failed to parse JSON")?;

                let url_str = if let Some(pointer) = &args.url_pointer {
                    json.pointer(pointer)
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow::anyhow!("URL not found at pointer `{}`", pointer))?
                } else {
                    json.get("url")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow::anyhow!("No `url` field found in JSON"))?
                };

                let url = Url::parse(url_str).context("Invalid URL extracted from JSON")?;

                let provenance = if args.attach_provenance {
                    Some(json)
                } else {
                    None
                };

                results.push((url, provenance));
            }
            other => anyhow::bail!("Unsupported --format: {}", other),
        }
    }

    Ok(results)
}

pub async fn run(
    args: CrawlArgs,
    profile_root: &PathBuf,
    cache_root: &PathBuf,
) -> anyhow::Result<()> {
    println!("Starting crawl...");

    let inputs = parse_input(&args)?;

    if inputs.is_empty() {
        println!("No URLs to crawl.");
        return Ok(());
    }

    println!("Found {} URLs", inputs.len());

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

    // Build requests
    let mut requests = Vec::new();
    for (url, provenance) in inputs {
        let provenance = provenance.unwrap_or_else(|| serde_json::json!({}));
        requests.push(CrawlRequest::seed(url, provenance));
    }

    // Run crawl
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

