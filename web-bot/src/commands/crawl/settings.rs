//! TOML and runtime settings model for the crawl command.
//!
//! This file is intentionally independent from Clap.
//!
//! It describes the durable crawl settings format and the fully resolved runtime
//! settings used by `crawl.rs`. It does not merge command-line overrides.
//!
//! The crawl command exposes one vocabulary in two shapes:
//!
//! - TOML uses nested sections such as `[budget] pages = 10`.
//! - CLI uses flattened flags such as `--pages 10`.
//!
//! Canonical example:
//!
//! ```toml
//! [input]
//! urls = ["https://books.toscrape.com"]
//! format = "text"
//!
//! [tags]
//! global = ["run:manual-debug"]
//! pointers = []
//!
//! [output]
//! format = "human"
//!
//! [budget]
//! pages = 10
//! total-pages = 100
//! depth = 1
//! frontier-items = 100000
//!
//! [runtime]
//! jobs = 8
//! sessions = 4
//! tabs = 2
//! cache-jobs = 32
//! rotate = 150
//! timeout-secs = 45
//!
//! [profile]
//! strategy = "by-seed-host"
//! key = "default"
//!
//! [cache]
//! enabled = true
//! namespace = "default"
//! ```

use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

use super::common::{
    CrawlInputFormat,
    CrawlOutputFormat,
    CrawlProfileStrategy,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CrawlSettings {
    pub input: CrawlInputSettings,
    pub tags: CrawlTagSettings,
    pub output: CrawlOutputSettings,
    pub budget: CrawlBudgetSettings,
    pub runtime: CrawlRuntimeSettings,
    pub profile: CrawlProfileSettings,
    pub cache: CrawlCacheSettings,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CrawlInputSettings {
    /// URLs to crawl.
    ///
    /// Compatibility: older config files may use `inputs`.
    #[serde(alias = "inputs")]
    pub urls: Vec<String>,

    pub format: CrawlInputFormat,
    pub url_pointer: Option<String>,

    /// Deprecated/no-op while provenance is represented by tags.
    pub attach_provenance: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CrawlTagSettings {
    /// Global `kind:key` tags attached to every crawl seed.
    pub global: Vec<String>,

    /// JSON-derived tag specs in `kind=/json/pointer` form.
    pub pointers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CrawlOutputSettings {
    pub format: CrawlOutputFormat,

    /// Deprecated compatibility alias for older config files.
    ///
    /// If true, this resolves to `format = "ndjson"`.
    pub json: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CrawlBudgetSettings {
    /// Maximum opened pages per original seed.
    ///
    /// Compatibility: older config files may use `pages-per-seed` or
    /// `max-pages`.
    #[serde(alias = "pages-per-seed", alias = "max-pages")]
    pub pages: usize,

    /// Optional global page budget across the whole crawl invocation.
    #[serde(alias = "max-total-pages")]
    pub total_pages: Option<usize>,

    /// Maximum hop depth from each original seed.
    #[serde(alias = "max-depth")]
    pub depth: u32,

    /// Maximum URLs retained in the frontier.
    #[serde(alias = "max-frontier-items")]
    pub frontier_items: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CrawlRuntimeSettings {
    /// Global in-flight page jobs across the whole crawl.
    #[serde(alias = "page-jobs")]
    pub jobs: usize,

    /// Maximum live Chromium browser sessions.
    #[serde(alias = "browser-sessions")]
    pub sessions: usize,

    /// Maximum concurrent tabs/pages per browser session.
    #[serde(alias = "tabs-per-session")]
    pub tabs: usize,

    /// Maximum concurrent cache operations.
    #[serde(alias = "max-concurrent-cache-ops")]
    pub cache_jobs: usize,

    /// Rotate each browser session after this many page opens.
    #[serde(alias = "pages-before-session-rotation")]
    pub rotate: usize,

    /// Page open timeout in seconds.
    #[serde(alias = "page-open-timeout-secs")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CrawlProfileSettings {
    pub strategy: CrawlProfileStrategy,
    pub key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CrawlCacheSettings {
    pub enabled: bool,
    pub namespace: Option<String>,
}

impl Default for CrawlSettings {
    fn default() -> Self {
        Self {
            input: CrawlInputSettings::default(),
            tags: CrawlTagSettings::default(),
            output: CrawlOutputSettings::default(),
            budget: CrawlBudgetSettings::default(),
            runtime: CrawlRuntimeSettings::default(),
            profile: CrawlProfileSettings::default(),
            cache: CrawlCacheSettings::default(),
        }
    }
}

impl Default for CrawlInputSettings {
    fn default() -> Self {
        Self {
            urls: Vec::new(),
            format: CrawlInputFormat::default(),
            url_pointer: None,
            attach_provenance: false,
        }
    }
}

impl Default for CrawlTagSettings {
    fn default() -> Self {
        Self {
            global: Vec::new(),
            pointers: Vec::new(),
        }
    }
}

impl Default for CrawlOutputSettings {
    fn default() -> Self {
        Self {
            format: CrawlOutputFormat::default(),
            json: false,
        }
    }
}

impl Default for CrawlBudgetSettings {
    fn default() -> Self {
        Self {
            pages: 10,
            total_pages: None,
            depth: 1,
            frontier_items: 100_000,
        }
    }
}

impl Default for CrawlRuntimeSettings {
    fn default() -> Self {
        Self {
            jobs: 8,
            sessions: 4,
            tabs: 2,
            cache_jobs: 32,
            rotate: 150,
            timeout_secs: 45,
        }
    }
}

impl Default for CrawlProfileSettings {
    fn default() -> Self {
        Self {
            strategy: CrawlProfileStrategy::default(),
            key: "default".to_string(),
        }
    }
}

impl Default for CrawlCacheSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            namespace: None,
        }
    }
}

impl CrawlSettings {
    pub fn load_toml(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();

        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read crawl config {}", path.display()))?;

        let mut settings = toml::from_str::<Self>(&text)
            .with_context(|| format!("failed to parse crawl config {}", path.display()))?;

        settings.normalize();

        Ok(settings)
    }

    pub fn normalize(&mut self) {
        if self.output.json {
            self.output.format = CrawlOutputFormat::Ndjson;
        }
    }

    pub fn is_ndjson_output(&self) -> bool {
        self.output.format == CrawlOutputFormat::Ndjson
    }
}

