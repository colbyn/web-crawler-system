//! Cache artifact evaluation policy.
//!
//! Cache storage retrieves artifacts. Cache policy decides whether an artifact
//! should be reused under current rules.
//!
//! This split is important because cache thresholds will change. A previously
//! stored artifact may become unacceptable if the crawler learns new health
//! rules, if network thresholds tighten, or if the artifact schema changes.
//!
//! Rejection is not exceptional. It usually means “recrawl and repair later.”
//! The cache is a performance layer, not ground truth.

use std::time::Duration;

use serde::{
    Deserialize,
    Serialize,
};

use crate::cache::{
    CachedPageArtifact,
    CACHED_PAGE_ARTIFACT_VERSION,
};


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CachePolicy {
    pub policy_version: u32,
    pub reject_if_cdp_disconnected: bool,
    pub reject_if_no_dom_evidence: bool,
    pub max_network_failure_ratio: Option<f64>,
    pub max_inflight_requests_at_snapshot: Option<usize>,
    pub min_html_bytes: Option<usize>,
    pub max_age: Option<Duration>,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            policy_version: 1,
            reject_if_cdp_disconnected: true,
            reject_if_no_dom_evidence: true,
            max_network_failure_ratio: Some(0.50),
            max_inflight_requests_at_snapshot: Some(8),
            min_html_bytes: Some(128),
            max_age: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheDecision {
    Use,
    Recrawl {
        reason: CacheRejectionReason,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheRejectionReason {
    ArtifactVersionMismatch {
        found: u32,
        expected: u32,
    },
    CachePolicyVersionChanged {
        found: u32,
        expected: u32,
    },
    CdpDisconnected,
    NoDomEvidence,
    NetworkFailureRatioTooHigh {
        observed: f64,
        max_allowed: f64,
    },
    TooManyInflightRequests {
        observed: usize,
        max_allowed: usize,
    },
    HtmlTooSmall {
        observed_bytes: usize,
        min_bytes: usize,
    },
    Expired,
}

impl CachePolicy {
    pub fn evaluate(&self, artifact: &CachedPageArtifact) -> CacheDecision {
        if artifact.artifact_version != CACHED_PAGE_ARTIFACT_VERSION {
            return CacheDecision::Recrawl {
                reason: CacheRejectionReason::ArtifactVersionMismatch {
                    found: artifact.artifact_version,
                    expected: CACHED_PAGE_ARTIFACT_VERSION,
                },
            };
        }

        if artifact.producer.cache_policy_version != self.policy_version {
            return CacheDecision::Recrawl {
                reason: CacheRejectionReason::CachePolicyVersionChanged {
                    found: artifact.producer.cache_policy_version,
                    expected: self.policy_version,
                },
            };
        }

        if self.reject_if_cdp_disconnected && artifact.telemetry.observed_cdp_disconnect() {
            return CacheDecision::Recrawl {
                reason: CacheRejectionReason::CdpDisconnected,
            };
        }

        if self.reject_if_no_dom_evidence && !artifact.telemetry.dom_was_available() {
            return CacheDecision::Recrawl {
                reason: CacheRejectionReason::NoDomEvidence,
            };
        }

        if let Some(max_allowed) = self.max_network_failure_ratio {
            if let Some(observed) = artifact.telemetry.network_failure_ratio() {
                if observed > max_allowed {
                    return CacheDecision::Recrawl {
                        reason: CacheRejectionReason::NetworkFailureRatioTooHigh {
                            observed,
                            max_allowed,
                        },
                    };
                }
            }
        }

        if let Some(max_allowed) = self.max_inflight_requests_at_snapshot {
            let observed = artifact.telemetry.network.inflight_count;

            if observed > max_allowed {
                return CacheDecision::Recrawl {
                    reason: CacheRejectionReason::TooManyInflightRequests {
                        observed,
                        max_allowed,
                    },
                };
            }
        }

        if let Some(min_bytes) = self.min_html_bytes {
            let observed_bytes = artifact.snapshot.body.len();

            if observed_bytes < min_bytes {
                return CacheDecision::Recrawl {
                    reason: CacheRejectionReason::HtmlTooSmall {
                        observed_bytes,
                        min_bytes,
                    },
                };
            }
        }

        CacheDecision::Use
    }
}

