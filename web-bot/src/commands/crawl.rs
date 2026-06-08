//! Crawl command implementation.

//! Crawl command implementation.

use clap::Args;
use std::io::{self, BufRead, Write};
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
    #[arg(short = 'i', long = "input", action = clap::ArgAction::Append)]
    pub inputs: Vec<String>,

    /// Input format when reading structured data
    #[arg(long, default_value = "text")]
    pub format: String,

    /// JSON Pointer to extract the URL from JSON input
    #[arg(long)]
    pub url_pointer: Option<String>,

    /// Attach the original JSON object as provenance (ndjson/json only)
    #[arg(long)]
    pub attach_provenance: bool,

    /// Output crawl results as NDJSON to stdout
    #[arg(long)]
    pub json: bool,

    #[arg(long, default_value_t = 50)]
    pub max_pages: usize,

    #[arg(long, default_value_t = 1)]
    pub max_depth: u32,
}

/// Collect input lines from `-i` flags and/or stdin
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

    if read_stdin || args.inputs.is_empty() {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let trimmed = line?.trim().to_string();
            if !trimmed.is_empty() {
                lines.push(trimmed);
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
        if args.json {
            // Emit empty array for machine consumers
            println!("[]");
        } else {
            eprintln!("No URLs provided.");
        }
        return Ok(());
    }

    // === Parse input ===
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
                            eprintln!("No `url` field found");
                            continue;
                        }
                    }
                };

                if let Ok(url) = Url::parse(&url_str) {
                    let provenance = if args.attach_provenance { Some(json) } else { None };
                    parsed.push((url, provenance));
                } else {
                    eprintln!("Invalid URL extracted: {}", url_str);
                }
            }
            other => anyhow::bail!("Unsupported format: {}", other),
        }
    }

    if parsed.is_empty() {
        eprintln!("No valid URLs to crawl after parsing.");
        return Ok(());
    }

    eprintln!("Crawling {} URLs...", parsed.len());

    // === Engine Setup ===
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

    // === Run Crawl ===
    let result = engine.crawl(requests).await?;

    // === Output ===
    if args.json {
        // Structured output to STDOUT (NDJSON)
        let stdout = io::stdout();
        let mut handle = stdout.lock();

        for page in &result.pages {
            if let Ok(json_line) = serde_json::to_string(page) {
                let _ = writeln!(handle, "{}", json_line);
            }
        }
    } else {
        // Human-readable summary to STDERR
        let mut success = 0;
        let mut failed = 0;
        let mut from_cache = 0;

        for page in &result.pages {
            match &page.outcome {
                web_crawler_engine_v3::output::CrawlPageOutcome::Opened { .. } => {
                    success += 1;
                    if matches!(page.cache_decision, Some(web_crawler_engine_v3::CacheDecision::Use)) {
                        from_cache += 1;
                    }
                }
                web_crawler_engine_v3::output::CrawlPageOutcome::Failed { .. } => failed += 1,
                _ => {}
            }
        }

        eprintln!("\n=== Crawl Complete ===");
        eprintln!("Total processed: {}", result.pages.len());
        eprintln!("  Successful:    {} ({} from cache)", success, from_cache);
        eprintln!("  Failed:        {}", failed);
    }

    Ok(())
}
