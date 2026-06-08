//! Crawler policy.
//!
//! Policy modules decide whether the crawler should visit, skip, cache, reuse,
//! or expand a page. They should operate on generic crawl facts, not app-specific
//! business rules.
//!
//! Examples of appropriate crawler policy:
//!
//! - same-domain or same-registrable-domain scope,
//! - maximum hop depth,
//! - skip obvious binary assets,
//! - reject poisoned snapshots,
//! - recrawl stale or unhealthy cache artifacts.
//!
//! Examples of inappropriate crawler policy:
//!
//! - deciding whether a company is a qualified lead,
//! - associating a page with a CRM record,
//! - ranking businesses,
//! - interpreting service categories.
//!
//! This crate should expose enough policy hooks for downstream callers without
//! forcing the browser driver or cache store to know crawler intent.

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;

use crate::{
    cache::{
        CacheDecision,
        CachePolicy,
        CachedPageArtifact,
    },
    config::CrawlLimits,
    input::CrawlRequest,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlPolicy {
    pub scope: ScopePolicy,
    pub visit: VisitPolicy,
    pub snapshot: SnapshotPolicy,
    pub cache: CachePolicy,
}

impl Default for CrawlPolicy {
    fn default() -> Self {
        Self {
            scope: ScopePolicy::default(),
            visit: VisitPolicy::default(),
            snapshot: SnapshotPolicy::default(),
            cache: CachePolicy::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ScopePolicy {
    pub mode: ScopeMode,
}

impl Default for ScopePolicy {
    fn default() -> Self {
        Self {
            mode: ScopeMode::SameHost,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeMode {
    SameHost,
    SameRegistrableDomain,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VisitPolicy {
    pub skip_non_http_urls: bool,
    pub skip_common_binary_assets: bool,
}

impl Default for VisitPolicy {
    fn default() -> Self {
        Self {
            skip_non_http_urls: true,
            skip_common_binary_assets: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SnapshotPolicy {
    pub capture_html: bool,
}

impl Default for SnapshotPolicy {
    fn default() -> Self {
        Self {
            capture_html: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeDecision {
    InScope,
    OutOfScope {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisitDecision {
    Visit,
    Skip {
        reason: String,
    },
}

impl CrawlPolicy {
    pub fn evaluate_visit<P>(
        &self,
        request: &CrawlRequest<P>,
        limits: &CrawlLimits,
    ) -> VisitDecision {
        if request.hop_depth > limits.max_hop_depth {
            return VisitDecision::Skip {
                reason: format!(
                    "hop depth {} exceeds max {}",
                    request.hop_depth, limits.max_hop_depth
                ),
            };
        }

        if self.visit.skip_non_http_urls && !is_http_url(&request.requested_url) {
            return VisitDecision::Skip {
                reason: "non-http URL".into(),
            };
        }

        if self.visit.skip_common_binary_assets
            && looks_like_common_binary_asset(&request.requested_url)
        {
            return VisitDecision::Skip {
                reason: "common binary/static asset URL".into(),
            };
        }

        VisitDecision::Visit
    }

    pub fn evaluate_scope(&self, seed_url: &Url, candidate_url: &Url) -> ScopeDecision {
        match self.scope.mode {
            ScopeMode::Any => ScopeDecision::InScope,

            ScopeMode::SameHost => {
                if seed_url.host_str() == candidate_url.host_str() {
                    ScopeDecision::InScope
                } else {
                    ScopeDecision::OutOfScope {
                        reason: "different host".into(),
                    }
                }
            }

            ScopeMode::SameRegistrableDomain => {
                // Stub. Proper PSL-backed implementation belongs in `url.rs`.
                if seed_url.domain() == candidate_url.domain() {
                    ScopeDecision::InScope
                } else {
                    ScopeDecision::OutOfScope {
                        reason: "different registrable domain".into(),
                    }
                }
            }
        }
    }

    pub fn evaluate_cache(&self, artifact: &CachedPageArtifact) -> CacheDecision {
        self.cache.evaluate(artifact)
    }
}

fn is_http_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

fn looks_like_common_binary_asset(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();

    [
        ".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".ico", ".pdf", ".zip",
        ".tar", ".gz", ".mp4", ".mov", ".mp3", ".woff", ".woff2", ".ttf",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}
