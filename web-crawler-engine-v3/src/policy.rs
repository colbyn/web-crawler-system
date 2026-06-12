//! Crawler policy.
//!
//! Policy modules decide whether the crawler should visit, skip, cache, reuse,
//! or expand a page. They should operate on generic crawl facts, not
//! app-specific business rules.
//!
//! Examples of appropriate crawler policy:
//!
//! - same-domain or same-registrable-domain scope,
//! - maximum hop depth,
//! - skip obvious binary/static assets,
//! - reject stale or incompatible cache metadata,
//! - recrawl artifacts produced under older cache policy semantics.
//!
//! Examples of inappropriate crawler policy:
//!
//! - deciding whether a company is a qualified lead,
//! - associating a page with a CRM record,
//! - ranking businesses,
//! - interpreting extracted anchors as job postings,
//! - assigning business-specific categories.
//!
//! This crate should expose enough generic policy hooks for downstream callers
//! without forcing the browser driver or artifact cache to understand crawler
//! intent.
//!
//! ## Extraction versus expansion
//!
//! Anchor extraction is page evidence. Frontier expansion is scheduling.
//!
//! A crawl with `max_hop_depth = 0` may still extract and persist anchors from
//! landing pages for downstream analysis. The hop-depth limit only prevents
//! those anchors from becoming follow-up requests.
//!
//! ## Cache policy
//!
//! Cache acceptance is intentionally metadata-only.
//!
//! Warm-cache replay should inspect cache metadata, extracted replay JSON, and
//! payload descriptors. It should not load large payload bodies such as rendered
//! HTML just to decide whether an artifact is usable.

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;
use web_crawler_db::{
    now_unix_ms,
    CacheEntryMetadata,
    CACHE_ENTRY_KIND_PAGE,
    CACHE_METADATA_VERSION,
};

use crate::{
    config::CrawlLimits,
    input::CrawlRequest,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CachePolicy {
    /// Bump this when cache acceptance semantics change.
    ///
    /// The cache entry stores the producer policy version. If this differs,
    /// the entry should be treated as stale and regenerated.
    pub policy_version: u32,

    /// Optional maximum cache age.
    ///
    /// `None` means cache entries do not expire by age at this layer.
    pub max_age_ms: Option<i64>,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            policy_version: 1,
            max_age_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheDecision {
    Use,

    Reject {
        reason: String,
    },
}

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
    /// Whether live browser captures should persist rendered HTML snapshots.
    ///
    /// This controls snapshot capture only. It does not control page-info or
    /// anchor extraction. Anchors remain useful landing-page evidence even when
    /// the crawl depth prevents following them.
    pub capture_html: bool,
}

impl Default for SnapshotPolicy {
    fn default() -> Self {
        Self { capture_html: true }
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

    /// Evaluate whether cache metadata can be replayed as page evidence.
    ///
    /// This check intentionally accepts metadata rather than a full cache entry.
    /// The replay path should not load HTML payload bytes just to decide whether a
    /// cached artifact is usable.
    pub fn evaluate_cache_metadata(
        &self,
        metadata: &CacheEntryMetadata,
    ) -> CacheDecision {
        if metadata.metadata_version != CACHE_METADATA_VERSION {
            return CacheDecision::Reject {
                reason: format!(
                    "metadata version {} != current {}",
                    metadata.metadata_version,
                    CACHE_METADATA_VERSION
                ),
            };
        }

        if metadata.entry_kind != CACHE_ENTRY_KIND_PAGE {
            return CacheDecision::Reject {
                reason: format!("unsupported cache entry kind {}", metadata.entry_kind),
            };
        }

        if metadata.producer.cache_policy_version != self.cache.policy_version {
            return CacheDecision::Reject {
                reason: format!(
                    "cache policy version {} != current {}",
                    metadata.producer.cache_policy_version,
                    self.cache.policy_version
                ),
            };
        }

        if let Some(max_age_ms) = self.cache.max_age_ms {
            let age_ms = now_unix_ms() - metadata.stored_at_unix_ms;

            if age_ms > max_age_ms {
                return CacheDecision::Reject {
                    reason: format!("cache age {}ms exceeds max {}ms", age_ms, max_age_ms),
                };
            }
        }

        if metadata.resolution.is_none() {
            return CacheDecision::Reject {
                reason: "cache metadata has no URL resolution facts".into(),
            };
        }

        if metadata.telemetry.is_none() {
            return CacheDecision::Reject {
                reason: "cache metadata has no page telemetry".into(),
            };
        }

        CacheDecision::Use
    }
}

fn is_http_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

fn looks_like_common_binary_asset(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();

    [
        ".jpg",
        ".jpeg",
        ".png",
        ".gif",
        ".webp",
        ".svg",
        ".ico",
        ".pdf",
        ".zip",
        ".tar",
        ".gz",
        ".mp4",
        ".mov",
        ".mp3",
        ".woff",
        ".woff2",
        ".ttf",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}

