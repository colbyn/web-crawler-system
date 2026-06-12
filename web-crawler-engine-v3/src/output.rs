//! Crawl output model.
//!
//! This module defines the result types returned by the crawler engine.
//!
//! Results must preserve both request-side and browser-observed facts:
//!
//! - original seed URL,
//! - requested URL,
//! - final URL,
//! - redirects if observed,
//! - caller provenance,
//! - page telemetry,
//! - cache decision,
//! - extraction results,
//! - snapshot accept/reject hints.
//!
//! The output types should be serializable because downstream tools may consume
//! crawl results via JSON, NDJSON, binary artifacts, or future APIs.
//!
//! This module should not decide app-specific interpretation. A page with a
//! careers link, contact form, product page, or malformed HTML remains a crawl
//! result, not a business decision.

use serde::{
    Deserialize,
    Serialize,
};
use web_browser_driver::{
    ExtractedAnchor,
    NonCriticalBrowserError,
    PageInfo,
    PageTelemetry,
    UrlResolution,
};

use crate::{
    input::CrawlRequest,
    policy::CacheDecision,
};

use web_crawler_db::CacheKey;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlRunResult<P = serde_json::Value> {
    pub pages: Vec<CrawlPageResult<P>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlPageResult<P = serde_json::Value> {
    pub request: CrawlRequest<P>,
    pub cache_key: Option<CacheKey>,
    pub cache_decision: Option<CacheDecision>,
    pub outcome: CrawlPageOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrawlPageOutcome {
    Opened {
        resolution: UrlResolution,
        status_code: Option<u16>,
        telemetry: PageTelemetry,
        non_critical_errors: Vec<NonCriticalBrowserError>,
        page_info: Option<PageInfo>,
        anchors: Vec<ExtractedAnchor>,
        snapshot: SnapshotDecision,
    },

    Failed {
        error: String,
        retryable: bool,
        should_terminate_session: bool,
    },

    Skipped {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotDecision {
    Captured {
        html_bytes: usize,
        body_sha256_hex: String,
    },

    Rejected {
        reason: String,
    },

    NotRequested,
}
