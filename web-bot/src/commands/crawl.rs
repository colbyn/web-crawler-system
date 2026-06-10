//! Crawl command implementation.
//!
//! This module is the integration layer for the crawl command.
//!
//! The crawl command has four separate concerns:
//!
//! - `common.rs` defines shared user-facing vocabulary and parsing helpers.
//! - `args.rs` defines the Clap-only command-line surface.
//! - `settings.rs` defines the TOML/runtime settings model.
//! - this file merges CLI arguments with settings, parses input rows, builds the
//!   engine config, runs the crawl, and prints results.
//!
//! Keep anything involving both CLI args and crawl settings in this file. That
//! keeps the two data models independent while still enforcing one coherent
//! user-facing vocabulary.
//!
//! ## Input parsing lanes
//!
//! The command intentionally supports distinct input modes instead of guessing:
//!
//! - `text`: plain URL lists, split by line/whitespace.
//! - `ndjson`: foreign newline-delimited JSON rows, adapted with JSON pointers.
//! - `seed-bundle`: WebBot-native newline-delimited JSON seed bundles shaped as
//!   `{ "urls": [...], "tags": [{ "kind": "...", "key": "..." }] }`.
//! - `json`: reserved for future full-document JSON ingestion.
//!
//! `seed-bundle` expands each URL in a bundle into its own crawl seed while
//! attaching the full bundle tag set plus any global CLI/TOML tags.

mod args;
mod common;
mod settings;

pub mod utils;

pub use args::CrawlArgs;
pub use common::{
    CrawlInputFormat,
    CrawlOutputFormat,
    CrawlProfileStrategy,
};

use std::{
    io::{
        self,
        BufRead,
        Write,
    },
    path::PathBuf,
};

use serde_json::Value;
use settings::CrawlSettings;
use url::Url;
use web_browser_driver::{
    BrowserDriver,
    BrowserProfileKey,
};
use web_crawler_engine_v3::{
    config::{
        CrawlConcurrency,
        CrawlEngineConfig,
        CrawlLimits,
    },
    input::CrawlRequest,
    policy::CrawlPolicy,
    scheduler::BrowserProfileStrategy as EngineBrowserProfileStrategy,
    sqlite_cache::CacheTag,
    CrawlEngine,
    SqliteCache,
};

use common::{
    parse_input_tags,
    parse_tag_pointers,
    parse_tags,
    tags_from_json_pointers,
    SeedBundle,
};

fn load_settings_from_args(args: &CrawlArgs) -> anyhow::Result<CrawlSettings> {
    let mut settings = if let Some(path) = &args.config {
        CrawlSettings::load_toml(path)?
    } else {
        CrawlSettings::default()
    };

    // Repeated values are additive: config first, CLI second.
    settings.input.urls.extend(args.inputs.clone());
    settings.tags.global.extend(args.tags.clone());
    settings.tags.pointers.extend(args.tag_pointers.clone());

    if let Some(format) = args.format {
        settings.input.format = format;
    }

    if args.url_pointer.is_some() {
        settings.input.url_pointer = args.url_pointer.clone();
    }

    if args.attach_provenance {
        settings.input.attach_provenance = true;
    }

    if let Some(output) = args.output {
        settings.output.format = output;
        settings.output.json = output == CrawlOutputFormat::Ndjson;
    }

    if args.json {
        settings.output.format = CrawlOutputFormat::Ndjson;
        settings.output.json = true;
    }

    if let Some(value) = args.pages {
        settings.budget.pages = value;
    }

    if let Some(value) = args.total_pages {
        settings.budget.total_pages = Some(value);
    }

    if let Some(value) = args.depth {
        settings.budget.depth = value;
    }

    if let Some(value) = args.frontier_items {
        settings.budget.frontier_items = value;
    }

    if let Some(value) = args.jobs {
        settings.runtime.jobs = value;
    }

    if let Some(value) = args.sessions {
        settings.runtime.sessions = value;
    }

    if let Some(value) = args.tabs {
        settings.runtime.tabs = value;
    }

    if let Some(value) = args.cache_jobs {
        settings.runtime.cache_jobs = value;
    }

    if let Some(value) = args.rotate {
        settings.runtime.rotate = value;
    }

    if let Some(value) = args.timeout_secs {
        settings.runtime.timeout_secs = value;
    }

    if let Some(value) = args.profile_strategy {
        settings.profile.strategy = value;
    }

    if let Some(value) = &args.profile_key {
        settings.profile.key = value.clone();
    }

    if let Some(value) = &args.namespace {
        settings.cache.namespace = Some(value.clone());
    }

    if args.no_cache {
        settings.cache.enabled = false;
    }

    settings.normalize();

    Ok(settings)
}

fn collect_input_lines(settings: &CrawlSettings) -> anyhow::Result<Vec<String>> {
    let mut lines = Vec::new();
    let mut read_stdin = false;

    for input in &settings.input.urls {
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

    if read_stdin || settings.input.urls.is_empty() {
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

fn profile_strategy_from_settings(
    settings: &CrawlSettings,
) -> EngineBrowserProfileStrategy {
    let fallback = BrowserProfileKey::new(settings.profile.key.clone());

    match settings.profile.strategy {
        CrawlProfileStrategy::Single => {
            EngineBrowserProfileStrategy::Single { key: fallback }
        }

        CrawlProfileStrategy::CallerProvidedOrSingle => {
            EngineBrowserProfileStrategy::CallerProvidedOrSingle { fallback }
        }

        CrawlProfileStrategy::ByHost => EngineBrowserProfileStrategy::ByHost,

        CrawlProfileStrategy::BySeedHost => EngineBrowserProfileStrategy::BySeedHost,
    }
}

fn parse_text_input_line(
    line: &str,
    global_tags: &[CacheTag],
    parsed: &mut Vec<(Url, Vec<CacheTag>)>,
) {
    if let Ok(url) = Url::parse(line) {
        parsed.push((url, global_tags.to_vec()));
    } else {
        eprintln!("Skipping invalid URL: {}", line);
    }
}

fn extract_url_from_json(
    json: &Value,
    url_pointer: Option<&String>,
) -> anyhow::Result<Option<String>> {
    if let Some(pointer) = url_pointer {
        match json.pointer(pointer).and_then(|value| value.as_str()) {
            Some(url) => Ok(Some(url.to_string())),
            None => {
                eprintln!("URL not found at pointer `{}`", pointer);
                Ok(None)
            }
        }
    } else {
        match json.get("url").and_then(|value| value.as_str()) {
            Some(url) => Ok(Some(url.to_string())),
            None => {
                eprintln!("No `url` field found");
                Ok(None)
            }
        }
    }
}

fn parse_json_input_line(
    line: &str,
    settings: &CrawlSettings,
    global_tags: &[CacheTag],
    tag_pointers: &[common::TagPointer],
    parsed: &mut Vec<(Url, Vec<CacheTag>)>,
) -> anyhow::Result<()> {
    let json: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("Skipping invalid JSON line");
            return Ok(());
        }
    };

    let Some(url_str) = extract_url_from_json(&json, settings.input.url_pointer.as_ref())? else {
        return Ok(());
    };

    let Ok(url) = Url::parse(&url_str) else {
        eprintln!("Invalid URL extracted: {}", url_str);
        return Ok(());
    };

    let mut tags = global_tags.to_vec();
    tags.extend(tags_from_json_pointers(&json, tag_pointers)?);

    parsed.push((url, tags));

    Ok(())
}

fn parse_seed_bundle_input_line(
    line: &str,
    global_tags: &[CacheTag],
    parsed: &mut Vec<(Url, Vec<CacheTag>)>,
) -> anyhow::Result<()> {
    let bundle: SeedBundle = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("Skipping invalid seed-bundle JSON line: {}", err);
            return Ok(());
        }
    };

    if bundle.urls.is_empty() {
        eprintln!("Skipping seed bundle with no URLs");
        return Ok(());
    }

    let mut tags = global_tags.to_vec();
    tags.extend(parse_input_tags(bundle.tags)?);

    for url_str in bundle.urls {
        let trimmed = url_str.trim();

        match Url::parse(trimmed) {
            Ok(url) => parsed.push((url, tags.clone())),
            Err(_) => eprintln!("Skipping invalid URL in seed bundle: {}", trimmed),
        }
    }

    Ok(())
}

fn print_run_header(
    settings: &CrawlSettings,
    config_path: Option<&PathBuf>,
    seed_count: usize,
    global_tags: &[CacheTag],
    tag_pointers: &[common::TagPointer],
    cache_db: &PathBuf,
) {
    eprintln!("Crawling {} seed URLs...", seed_count);

    eprintln!(
        "Budget:  pages={} depth={} total-pages={:?} frontier-items={}",
        settings.budget.pages,
        settings.budget.depth,
        settings.budget.total_pages,
        settings.budget.frontier_items,
    );

    eprintln!(
        "Runtime: jobs={} sessions={} tabs={} cache-jobs={} rotate={} timeout={}s",
        settings.runtime.jobs,
        settings.runtime.sessions,
        settings.runtime.tabs,
        settings.runtime.cache_jobs,
        settings.runtime.rotate,
        settings.runtime.timeout_secs,
    );

    eprintln!(
        "Profile: strategy={:?} key={}",
        settings.profile.strategy,
        settings.profile.key,
    );

    eprintln!(
        "Cache:   enabled={} namespace={:?} db={}",
        settings.cache.enabled,
        settings.cache.namespace,
        cache_db.display(),
    );

    eprintln!("Output:  {:?}", settings.output.format);

    if let Some(config_path) = config_path {
        eprintln!("Config:  {}", config_path.display());
    }

    if !global_tags.is_empty() {
        eprintln!(
            "Tags:    {}",
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
}

pub async fn run(
    args: CrawlArgs,
    profile_root: &PathBuf,
    cache_db: &PathBuf,
) -> anyhow::Result<()> {
    let config_path = args.config.clone();
    let settings = load_settings_from_args(&args)?;

    let raw_lines = collect_input_lines(&settings)?;

    if raw_lines.is_empty() {
        if settings.is_ndjson_output() {
            return Ok(());
        }

        eprintln!("No URLs provided.");
        return Ok(());
    }

    let global_tags = parse_tags(&settings.tags.global)?;
    let tag_pointers = parse_tag_pointers(&settings.tags.pointers)?;

    if settings.input.attach_provenance {
        tracing::warn!(
            "--attach-provenance is currently ignored; use --tag and --tag-pointer instead"
        );
    }

    if settings.input.format == CrawlInputFormat::Json {
        anyhow::bail!(
            "`--format json` is reserved for future full-document JSON input; \
             use `--format ndjson` for pointer-mapped JSON lines or \
             `--format seed-bundle` for WebBot seed-bundle NDJSON"
        );
    }

    let mut parsed: Vec<(Url, Vec<CacheTag>)> = Vec::new();

    for line in raw_lines {
        match settings.input.format {
            CrawlInputFormat::Text => {
                parse_text_input_line(&line, &global_tags, &mut parsed);
            }

            CrawlInputFormat::Ndjson => {
                parse_json_input_line(
                    &line,
                    &settings,
                    &global_tags,
                    &tag_pointers,
                    &mut parsed,
                )?;
            }

            CrawlInputFormat::SeedBundle => {
                parse_seed_bundle_input_line(
                    &line,
                    &global_tags,
                    &mut parsed,
                )?;
            }

            CrawlInputFormat::Json => {
                unreachable!("json input format is rejected before parsing");
            }
        }
    }

    if parsed.is_empty() {
        if !settings.is_ndjson_output() {
            eprintln!("No valid URLs to crawl after parsing.");
        }

        return Ok(());
    }

    let input_order_seed = utils::randomize_parsed_seed_order(&mut parsed);
    eprintln!("Input order: randomized seed={}", input_order_seed);

    if !settings.is_ndjson_output() {
        print_run_header(
            &settings,
            config_path.as_ref(),
            parsed.len(),
            &global_tags,
            &tag_pointers,
            cache_db,
        );
    }

    let sqlite_cache = if settings.cache.enabled {
        Some(SqliteCache::open(cache_db).await?)
    } else {
        None
    };

    let driver = BrowserDriver::new(Default::default());

    let config = CrawlEngineConfig {
        limits: CrawlLimits {
            max_pages_per_seed: settings.budget.pages,
            max_hop_depth: settings.budget.depth,
            max_frontier_items: settings.budget.frontier_items,
            max_total_pages: settings.budget.total_pages,
        },

        concurrency: CrawlConcurrency {
            max_concurrent_pages: settings.runtime.jobs,
            max_sessions: settings.runtime.sessions,
            max_concurrent_pages_per_session: settings.runtime.tabs,
            max_concurrent_cache_ops: settings.runtime.cache_jobs,
            max_pages_per_session: settings.runtime.rotate,
        },

        page_open_timeout: std::time::Duration::from_secs(
            settings.runtime.timeout_secs,
        ),
        cache_enabled: settings.cache.enabled,
    };

    let policy = CrawlPolicy::default();
    let profile_strategy = profile_strategy_from_settings(&settings);

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
        .map(|(url, tags)| {
            let url = if let Some(namespace) = &settings.cache.namespace {
                // Cache namespace is intentionally part of the cache key, not the
                // URL itself. The current engine API receives namespace through
                // cache-key construction elsewhere if supported. Until the engine
                // accepts a namespace on requests, keep this variable read so the
                // public setting is not silently forgotten in this integration
                // layer.
                let _ = namespace;
                url
            } else {
                url
            };

            CrawlRequest::seed_with_tags(url, tags)
        })
        .collect();

    let result = engine.crawl(requests).await?;

    if settings.is_ndjson_output() {
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
        let mut skipped = 0;
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

                web_crawler_engine_v3::output::CrawlPageOutcome::Skipped { .. } => {
                    skipped += 1;
                }
            }
        }

        eprintln!("\n=== Crawl Complete ===");
        eprintln!("Total results:  {}", result.pages.len());
        eprintln!("  Successful:   {} ({} from cache)", success, from_cache);
        eprintln!("  Failed:       {}", failed);
        eprintln!("  Skipped:      {}", skipped);
    }

    Ok(())
}

