//! Crawl command implementation.
//!
//! Integration layer between CLI args, TOML settings, and the crawler engine.
//!
//! This command is intentionally the app-facing layer where crawler-engine
//! defaults may be specialized for the current `web-bot` use case.
//!
//! The engine itself remains generic:
//!
//! - it does not know what careers, jobs, products, docs, or services mean,
//! - it does not make business-specific visit decisions,
//! - it does not include scoring in cache identity,
//! - it only accepts optional frontier scoring as a scheduling hint.
//!
//! This CLI currently enables the built-in `careers` frontier scoring profile
//! because the active crawl workload is biased toward finding career pages, job
//! listing pages, and likely job detail pages with small per-seed budgets.
//!
//! Frontier scoring only affects ordering inside each seed bucket. It does not
//! filter URLs. Given sufficient budget, low-scored internal URLs remain
//! eligible to be crawled.

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
    io::{self, BufRead, Write},
    path::PathBuf,
};

use serde_json::Value;
use settings::CrawlSettings;
use url::Url;
use web_browser_driver::{
    BrowserDriver,
    BrowserProfileKey,
};
use web_crawler_db::PostgresCache;
use web_crawler_engine_v3::{
    config::{
        CrawlConcurrency,
        CrawlEngineConfig,
        CrawlLimits,
        FrontierConfig,
        FrontierScoringConfig,
    },
    input::CrawlRequest,
    policy::CrawlPolicy,
    scheduler::BrowserProfileStrategy as EngineBrowserProfileStrategy,
    url_score::BuiltinUrlScoringProfile,
    CrawlEngine,
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

fn profile_strategy_from_settings(settings: &CrawlSettings) -> EngineBrowserProfileStrategy {
    let fallback = BrowserProfileKey::new(settings.profile.key.clone());

    match settings.profile.strategy {
        CrawlProfileStrategy::Single => EngineBrowserProfileStrategy::Single {
            key: fallback,
        },

        CrawlProfileStrategy::CallerProvidedOrSingle => {
            EngineBrowserProfileStrategy::CallerProvidedOrSingle { fallback }
        }

        CrawlProfileStrategy::ByHost => EngineBrowserProfileStrategy::ByHost,

        CrawlProfileStrategy::BySeedHost => EngineBrowserProfileStrategy::BySeedHost,
    }
}

/// Build the frontier scheduling configuration used by this CLI command.
///
/// The crawler engine default keeps scoring disabled. This command enables the
/// current app-layer intent: prioritize career/job-related internal URLs as
/// they are uncovered.
///
/// This does not filter anything. It only changes per-seed frontier order.
fn frontier_config_for_crawl_command() -> FrontierConfig {
    FrontierConfig {
        scoring: FrontierScoringConfig {
            enabled: true,
            builtin_profile: BuiltinUrlScoringProfile::Careers,
            retain_evidence: false,
        },
    }
}

fn parse_text_input_line(
    line: &str,
    global_tags: &[web_crawler_db::CacheTag],
    parsed: &mut Vec<(Url, Vec<web_crawler_db::CacheTag>)>,
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
        match json.pointer(pointer).and_then(|v| v.as_str()) {
            Some(url) => Ok(Some(url.to_string())),
            None => {
                eprintln!("URL not found at pointer `{}`", pointer);
                Ok(None)
            }
        }
    } else {
        match json.get("url").and_then(|v| v.as_str()) {
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
    global_tags: &[web_crawler_db::CacheTag],
    tag_pointers: &[common::TagPointer],
    parsed: &mut Vec<(Url, Vec<web_crawler_db::CacheTag>)>,
) -> anyhow::Result<()> {
    let json: Value = match serde_json::from_str(line) {
        Ok(v) => v,

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
    global_tags: &[web_crawler_db::CacheTag],
    parsed: &mut Vec<(Url, Vec<web_crawler_db::CacheTag>)>,
) -> anyhow::Result<()> {
    let bundle: SeedBundle = match serde_json::from_str(line) {
        Ok(v) => v,

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
    global_tags: &[web_crawler_db::CacheTag],
    tag_pointers: &[common::TagPointer],
    database_url: &str,
    frontier: &FrontierConfig,
) {
    eprintln!("Crawling {} seed URLs...", seed_count);

    eprintln!(
        "Budget:  pages={} depth={} total-pages={:?} frontier-items={}",
        settings.budget.pages,
        settings.budget.depth,
        settings.budget.total_pages,
        settings.budget.frontier_items
    );

    eprintln!(
        "Runtime: jobs={} sessions={} tabs={} cache-jobs={} rotate={} timeout={}s",
        settings.runtime.jobs,
        settings.runtime.sessions,
        settings.runtime.tabs,
        settings.runtime.cache_jobs,
        settings.runtime.rotate,
        settings.runtime.timeout_secs
    );

    eprintln!(
        "Profile: strategy={:?} key={}",
        settings.profile.strategy,
        settings.profile.key
    );

    eprintln!(
        "Cache:   enabled={} namespace={:?} db={}",
        settings.cache.enabled,
        settings.cache.namespace,
        database_url
    );

    eprintln!(
        "Frontier: scoring={} profile={:?} retain-evidence={}",
        frontier.scoring.enabled,
        frontier.scoring.builtin_profile,
        frontier.scoring.retain_evidence
    );

    eprintln!("Output:  {:?}", settings.output.format);

    if let Some(p) = config_path {
        eprintln!("Config:  {}", p.display());
    }

    if !global_tags.is_empty() {
        eprintln!(
            "Tags:    {}",
            global_tags
                .iter()
                .map(|t| t.as_compound())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    if !tag_pointers.is_empty() {
        eprintln!(
            "Tag pointers: {}",
            tag_pointers
                .iter()
                .map(|s| format!("{}={}", s.kind, s.pointer))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

pub async fn run(
    args: CrawlArgs,
    profile_root: &PathBuf,
    database_url: &str,
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
             use `--format ndjson` or `--format seed-bundle`"
        );
    }

    let mut parsed: Vec<(Url, Vec<web_crawler_db::CacheTag>)> = Vec::new();

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
                parse_seed_bundle_input_line(&line, &global_tags, &mut parsed)?;
            }

            CrawlInputFormat::Json => unreachable!(),
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

    let frontier = frontier_config_for_crawl_command();

    if !settings.is_ndjson_output() {
        print_run_header(
            &settings,
            config_path.as_ref(),
            parsed.len(),
            &global_tags,
            &tag_pointers,
            database_url,
            &frontier,
        );
    }

    let postgres_cache: Option<PostgresCache> = if settings.cache.enabled {
        Some(PostgresCache::connect(database_url).await?)
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

        frontier,

        page_open_timeout: std::time::Duration::from_secs(settings.runtime.timeout_secs),
        cache_enabled: settings.cache.enabled,
    };

    let policy = CrawlPolicy::default();
    let profile_strategy = profile_strategy_from_settings(&settings);

    let engine: CrawlEngine<serde_json::Value> = CrawlEngine::new(
        config,
        policy,
        driver,
        postgres_cache,
        profile_root.clone(),
        profile_strategy,
    );

    let requests: Vec<CrawlRequest<serde_json::Value>> = parsed
        .into_iter()
        .map(|(url, tags)| {
            if settings.cache.namespace.is_some() {
                // Namespace is currently only a setting; not part of CacheKey yet.
            }

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
                        Some(web_crawler_engine_v3::policy::CacheDecision::Use)
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

