//! Crawl command implementation.
//!
//! This command accepts URLs from CLI args or stdin, creates crawl seed requests,
//! and runs the crawler into the shared SQLite cache.
//!
//! Tags are the CLI's durable association mechanism.
//!
//! Every seed can receive:
//!
//! - global tags from `--tag kind:key`,
//! - JSON-derived tags from `--tag-pointer kind=/json/pointer`.
//!
//! Those tags flow into `CrawlRequest::seed_with_tags(...)`, then through the
//! crawler into discovered pages and finally into SQLite cache tag links.
//!
//! This lets downstream tools ask questions such as:
//!
//! ```text
//! all pages crawled for entity:business-123
//! all pages crawled for category:electricians
//! all pages crawled for run:manual-debug
//! ```

use clap::Args;
use serde_json::Value;
use std::io::{
    self,
    BufRead,
    Write,
};
use std::path::PathBuf;
use url::Url;
use web_browser_driver::BrowserDriver;
use web_crawler_engine_v3::{
    config::{
        CrawlEngineConfig,
        CrawlLimits,
    },
    input::CrawlRequest,
    policy::CrawlPolicy,
    scheduler::BrowserProfileStrategy,
    sqlite_cache::CacheTag,
    CrawlEngine,
    SqliteCache,
};

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

    /// Attach the original JSON object as provenance.
    ///
    /// Deprecated/no-op while provenance is represented by tags.
    #[arg(long)]
    pub attach_provenance: bool,

    /// Attach a global tag to every crawl seed.
    ///
    /// Format: kind:key
    ///
    /// Examples:
    ///
    /// - run:manual-debug
    /// - category:electricians
    /// - category:hvac
    #[arg(long = "tag", action = clap::ArgAction::Append)]
    pub tags: Vec<String>,

    /// Attach a tag from a JSON value for each structured input row.
    ///
    /// Format: kind=/json/pointer
    ///
    /// Example:
    ///
    /// --tag-pointer entity=/id
    ///
    /// If the pointer resolves to an array, every scalar item becomes a tag.
    #[arg(long = "tag-pointer", action = clap::ArgAction::Append)]
    pub tag_pointers: Vec<String>,

    /// Output crawl results as NDJSON to stdout
    #[arg(long)]
    pub json: bool,

    #[arg(long, default_value_t = 50)]
    pub max_pages: usize,

    #[arg(long, default_value_t = 1)]
    pub max_depth: u32,

    /// Disable cache lookup/storage for this crawl
    #[arg(long)]
    pub no_cache: bool,
}

#[derive(Debug, Clone)]
struct TagPointer {
    kind: String,
    pointer: String,
}

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

fn parse_tag(value: &str) -> anyhow::Result<CacheTag> {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        anyhow::bail!("tag cannot be empty");
    }

    if !trimmed.contains(':') {
        anyhow::bail!(
            "tag `{}` must use kind:key format, e.g. entity:business-123",
            trimmed
        );
    }

    Ok(CacheTag::from_compound(trimmed))
}

fn parse_global_tags(values: &[String]) -> anyhow::Result<Vec<CacheTag>> {
    values.iter().map(|value| parse_tag(value)).collect()
}

fn parse_tag_pointer(value: &str) -> anyhow::Result<TagPointer> {
    let Some((kind, pointer)) = value.split_once('=') else {
        anyhow::bail!(
            "tag pointer `{}` must use kind=/json/pointer format",
            value
        );
    };

    let kind = kind.trim();
    let pointer = pointer.trim();

    if kind.is_empty() {
        anyhow::bail!("tag pointer kind cannot be empty");
    }

    if !pointer.starts_with('/') {
        anyhow::bail!(
            "tag pointer `{}` must use a JSON pointer beginning with `/`",
            value
        );
    }

    Ok(TagPointer {
        kind: kind.to_string(),
        pointer: pointer.to_string(),
    })
}

fn parse_tag_pointers(values: &[String]) -> anyhow::Result<Vec<TagPointer>> {
    values
        .iter()
        .map(|value| parse_tag_pointer(value))
        .collect()
}

fn scalar_tag_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let value = value.trim();

            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }

        Value::Number(value) => Some(value.to_string()),

        Value::Bool(value) => Some(value.to_string()),

        _ => None,
    }
}

fn tags_from_json_pointers(
    json: &Value,
    tag_pointers: &[TagPointer],
) -> anyhow::Result<Vec<CacheTag>> {
    let mut tags = Vec::new();

    for spec in tag_pointers {
        let Some(value) = json.pointer(&spec.pointer) else {
            continue;
        };

        match value {
            Value::Array(values) => {
                for item in values {
                    if let Some(key) = scalar_tag_value(item) {
                        tags.push(CacheTag::new(&spec.kind, key));
                    }
                }
            }

            other => {
                if let Some(key) = scalar_tag_value(other) {
                    tags.push(CacheTag::new(&spec.kind, key));
                }
            }
        }
    }

    Ok(tags)
}

pub async fn run(
    args: CrawlArgs,
    profile_root: &PathBuf,
    cache_db: &PathBuf,
) -> anyhow::Result<()> {
    let raw_lines = collect_input_lines(&args)?;

    if raw_lines.is_empty() {
        if args.json {
            println!("[]");
        } else {
            eprintln!("No URLs provided.");
        }

        return Ok(());
    }

    let global_tags = parse_global_tags(&args.tags)?;
    let tag_pointers = parse_tag_pointers(&args.tag_pointers)?;

    if args.attach_provenance {
        tracing::warn!(
            "--attach-provenance is currently ignored; use --tag and --tag-pointer instead"
        );
    }

    let mut parsed: Vec<(Url, Vec<CacheTag>)> = Vec::new();

    for line in raw_lines {
        match args.format.as_str() {
            "text" => {
                if let Ok(url) = Url::parse(&line) {
                    parsed.push((url, global_tags.clone()));
                } else {
                    eprintln!("Skipping invalid URL: {}", line);
                }
            }

            "ndjson" | "json" => {
                let json: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(_) => {
                        eprintln!("Skipping invalid JSON line");
                        continue;
                    }
                };

                let url_str = if let Some(pointer) = &args.url_pointer {
                    match json.pointer(pointer).and_then(|value| value.as_str()) {
                        Some(url) => url.to_string(),
                        None => {
                            eprintln!("URL not found at pointer `{}`", pointer);
                            continue;
                        }
                    }
                } else {
                    match json.get("url").and_then(|value| value.as_str()) {
                        Some(url) => url.to_string(),
                        None => {
                            eprintln!("No `url` field found");
                            continue;
                        }
                    }
                };

                let Ok(url) = Url::parse(&url_str) else {
                    eprintln!("Invalid URL extracted: {}", url_str);
                    continue;
                };

                let mut tags = global_tags.clone();
                tags.extend(tags_from_json_pointers(&json, &tag_pointers)?);

                parsed.push((url, tags));
            }

            other => anyhow::bail!("Unsupported format: {}", other),
        }
    }

    if parsed.is_empty() {
        eprintln!("No valid URLs to crawl after parsing.");
        return Ok(());
    }

    eprintln!("Crawling {} URLs...", parsed.len());

    if !global_tags.is_empty() {
        eprintln!(
            "Global tags: {}",
            global_tags
                .iter()
                .map(|tag| tag.as_compound())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    if !tag_pointers.is_empty() {
        eprintln!(
            "Tag pointers: {}",
            tag_pointers
                .iter()
                .map(|spec| format!("{}={}", spec.kind, spec.pointer))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let sqlite_cache = if args.no_cache {
        None
    } else {
        Some(SqliteCache::open(cache_db).await?)
    };

    let driver = BrowserDriver::new(Default::default());

    let config = CrawlEngineConfig {
        limits: CrawlLimits {
            max_pages: args.max_pages,
            max_hop_depth: args.max_depth,
            max_frontier_items: 10_000,
        },
        page_open_timeout: std::time::Duration::from_secs(45),
        cache_enabled: !args.no_cache,
    };

    let policy = CrawlPolicy::default();
    let profile_strategy = BrowserProfileStrategy::default();

    let engine: CrawlEngine<serde_json::Value> = CrawlEngine::new(
        config,
        policy,
        driver,
        sqlite_cache,
        profile_root.clone(),
        profile_strategy,
    );

    let requests: Vec<CrawlRequest<serde_json::Value>> = parsed
        .into_iter()
        .map(|(url, tags)| CrawlRequest::seed_with_tags(url, tags))
        .collect();

    let result = engine.crawl(requests).await?;

    if args.json {
        let stdout = io::stdout();
        let mut handle = stdout.lock();

        for page in &result.pages {
            if let Ok(json_line) = serde_json::to_string(page) {
                let _ = writeln!(handle, "{}", json_line);
            }
        }
    } else {
        let mut success = 0;
        let mut failed = 0;
        let mut from_cache = 0;

        for page in &result.pages {
            match &page.outcome {
                web_crawler_engine_v3::output::CrawlPageOutcome::Opened { .. } => {
                    success += 1;

                    if matches!(
                        page.cache_decision,
                        Some(web_crawler_engine_v3::CacheDecision::Use)
                    ) {
                        from_cache += 1;
                    }
                }

                web_crawler_engine_v3::output::CrawlPageOutcome::Failed { .. } => {
                    failed += 1;
                }

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

