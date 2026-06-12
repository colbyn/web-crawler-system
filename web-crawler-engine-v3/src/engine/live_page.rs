//! Live browser page capture.
//!
//! This module owns the cold-cache browser path for one crawl request.
//!
//! A live page capture performs the browser-facing work needed to produce page
//! evidence:
//!
//! - open the requested URL in an assigned browser session,
//! - extract generic page information,
//! - extract anchors,
//! - optionally capture rendered HTML,
//! - assemble a normal `CrawlPageResult`,
//! - persist a reusable cache artifact when caching and HTML capture are
//!   available,
//! - close the browser page on a short best-effort timeout.
//!
//! ## Anchors are evidence
//!
//! Anchor extraction is deliberately part of live page evidence, not merely
//! frontier scheduling.
//!
//! A crawl with `max_hop_depth = 0` should still extract anchors from landing
//! pages. Those anchors may be used later for analysis of uncovered career
//! pages, job listings, contact paths, service pages, or other downstream
//! discovery work. Hop depth controls whether anchors become future requests,
//! not whether anchors are collected.
//!
//! ## Cache storage boundary
//!
//! This module writes through `web-crawler-db` and uses the simplified cache
//! model:
//!
//! - metadata/provenance in `CacheEntryMetadata`,
//! - one primary rendered HTML body in `CachePayload`,
//! - replay facts such as page info, anchors, resolution, and telemetry in
//!   typed metadata fields,
//! - caller associations in cache tags.
//!
//! Multi-payload support is intentionally not modeled here. If screenshots,
//! raw response bodies, network logs, or additional artifacts are needed later,
//! they should be added through an explicit `web-crawler-db` schema/API
//! migration.
//!
//! ## Throughput note
//!
//! This revision keeps cache persistence inline so the migration remains
//! compiler-guided and easy to reason about. A later writer module can move
//! cache writes behind a bounded queue without changing the page evidence shape
//! or creating a separate seed-only path.

use std::{
    sync::Arc,
    time::Duration,
};

use serde::{
    de::DeserializeOwned,
    Serialize,
};
use web_browser_driver::{
    AnchorExtractor,
    BrowserSession,
    OpenPageOptions,
    PageInfoExtractor,
};
use web_crawler_db::{
    cache_key_digest,
    now_unix_ms,
    CacheCapturePolicy,
    CacheEntry,
    CacheEntryMetadata,
    CacheKey,
    CachePayload,
    CachePayloadCompression,
    CacheProducerInfo,
    CacheRequestInfo,
    CacheResponseInfo,
};

use crate::{
    error::CrawlEngineResult,
    input::CrawlRequest,
    output::{
        CrawlPageOutcome,
        CrawlPageResult,
        SnapshotDecision,
    },
    scheduler::BrowserProfileAssignment,
    store::CrawlArtifactSink,
};

use super::CrawlEngine;

impl<P, S> CrawlEngine<P, S>
where
    P: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    S: CrawlArtifactSink<P> + Send + Sync,
{
    /// Execute a live browser crawl for a single request.
    ///
    /// This is the cold-cache path. It returns a normal page result whether the
    /// live page succeeds or fails. Ordinary browser/page failures should become
    /// `CrawlPageOutcome::Failed`, not whole-crawl errors.
    pub(crate) async fn crawl_live_page(
        &self,
        request: &CrawlRequest<P>,
        cache_key: CacheKey,
        assignment: BrowserProfileAssignment,
        session: Arc<BrowserSession>,
    ) -> CrawlEngineResult<CrawlPageResult<P>> {
        tracing::debug!(
            requested_url = %request.requested_url,
            "opening live page"
        );

        let open_options = OpenPageOptions::new(request.requested_url.clone())
            .with_max_timeout(self.config.page_open_timeout)
            .with_navigation_timeout(self.config.page_open_timeout);

        let opened = match session.open_page(open_options).await {
            Ok(opened) => opened,

            Err(err) => {
                return Ok(CrawlPageResult {
                    request: request.clone(),
                    cache_key: Some(cache_key),
                    cache_decision: None,
                    outcome: CrawlPageOutcome::Failed {
                        error: err.to_string(),
                        retryable: err.is_retryable_environment_failure(),
                        should_terminate_session: err.should_terminate_session(),
                    },
                });
            }
        };

        let page_info = PageInfoExtractor::extract(&opened.page).await.ok();

        let anchors = AnchorExtractor::extract(&opened.page)
            .await
            .unwrap_or_default();

        let html = if self.policy.snapshot.capture_html {
            opened.page.html().await.ok()
        } else {
            None
        };

        if html.is_none() && self.policy.snapshot.capture_html {
            tracing::warn!(
                requested_url = %request.requested_url,
                "page opened but html capture failed; cache entry will not be saved"
            );
        }

        let snapshot = snapshot_decision_from_html(&html, self.policy.snapshot.capture_html);

        let result = CrawlPageResult {
            request: request.clone(),
            cache_key: Some(cache_key.clone()),
            cache_decision: None,
            outcome: CrawlPageOutcome::Opened {
                resolution: opened.resolution.clone(),
                status_code: opened.status_code,
                telemetry: opened.telemetry.clone(),
                non_critical_errors: opened.non_critical_errors.clone(),
                page_info: page_info.clone(),
                anchors: anchors.clone(),
                snapshot,
            },
        };

        if self.config.cache_enabled {
            if let Some(cache) = &self.cache {
                if let Some(html) = html {
                    let cache_entry = build_page_cache_entry(
                        request,
                        &cache_key,
                        &assignment,
                        &opened,
                        page_info,
                        anchors,
                        html,
                        self.policy.cache.policy_version,
                    );

                    match cache.put(&cache_entry).await {
                        Ok(()) => {
                            let key_digest = cache_key_digest(&cache_key)
                                .unwrap_or_else(|_| "<digest-error>".to_string());

                            tracing::debug!(
                                requested_url = %request.requested_url,
                                key_digest = %key_digest,
                                tag_count = request.tags.len(),
                                "saved postgres cache entry"
                            );
                        }

                        Err(err) => {
                            tracing::warn!(
                                requested_url = %request.requested_url,
                                error = %err,
                                "failed to save postgres cache entry"
                            );
                        }
                    }
                }
            }
        }

        close_page_best_effort(
            &request.requested_url,
            opened.page.close(),
            Duration::from_secs(3),
        )
        .await;

        tracing::debug!(
            requested_url = %request.requested_url,
            "live page crawl returning result"
        );

        Ok(result)
    }
}

/// Build a snapshot decision for returned crawl output.
///
/// This helper intentionally constructs a temporary `CachePayload` for captured
/// HTML so the digest and byte length use the same descriptor logic as the
/// persisted payload path.
fn snapshot_decision_from_html(
    html: &Option<String>,
    capture_requested: bool,
) -> SnapshotDecision {
    match html {
        Some(html) => {
            let payload = CachePayload::new(
                Some("text/html".to_string()),
                CachePayloadCompression::None,
                html.as_bytes().to_vec(),
            );

            SnapshotDecision::Captured {
                html_bytes: payload.descriptor.byte_len,
                body_sha256_hex: payload.descriptor.sha256_hex,
            }
        }

        None if capture_requested => SnapshotDecision::Rejected {
            reason: "failed to capture HTML".into(),
        },

        None => SnapshotDecision::NotRequested,
    }
}

/// Build a Postgres cache entry from a live browser capture.
///
/// The simplified DB model stores one primary payload body per entry. Replay
/// facts needed for warm-cache results live in typed metadata fields so ordinary
/// cache replay can avoid loading HTML bytes.
fn build_page_cache_entry<P>(
    request: &CrawlRequest<P>,
    cache_key: &CacheKey,
    assignment: &BrowserProfileAssignment,
    opened: &web_browser_driver::OpenedPage,
    page_info: Option<web_browser_driver::PageInfo>,
    anchors: Vec<web_browser_driver::ExtractedAnchor>,
    html: String,
    cache_policy_version: u32,
) -> CacheEntry {
    let payload = CachePayload::new(
        Some("text/html".to_string()),
        CachePayloadCompression::None,
        html.into_bytes(),
    );

    let capture_policy = Some(CacheCapturePolicy {
        browser_profile_key: assignment.key.as_str().to_string(),
        cache_policy_version,
        capture_html: true,
    });

    let mut metadata = CacheEntryMetadata::new_page(
        cache_key.clone(),
        now_unix_ms(),
        CacheProducerInfo {
            engine_name: "web-crawler-engine-v3".to_string(),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            driver_version: None,
            cache_policy_version,
        },
        CacheRequestInfo::from_key(cache_key, capture_policy),
        CacheResponseInfo {
            final_url: Some(opened.resolution.final_url.to_string()),
            final_host: opened.resolution.final_url.host_str().map(ToOwned::to_owned),
            status_code: opened.status_code,
            content_type: Some("text/html".to_string()),
        },
    );

    metadata.resolution = Some(opened.resolution.clone());
    metadata.page_info = page_info;
    metadata.anchors = anchors;
    metadata.telemetry = Some(opened.telemetry.clone());
    metadata.non_critical_errors = opened.non_critical_errors.clone();

    CacheEntry {
        metadata,
        payload,
        tags: request.tags.clone(),
    }
}

/// Close a browser page without allowing cleanup to hold the crawl hostage.
async fn close_page_best_effort<F>(
    requested_url: &url::Url,
    close_future: F,
    timeout: Duration,
)
where
    F: std::future::Future<Output = Result<(), web_browser_driver::BrowserDriverError>>,
{
    match tokio::time::timeout(timeout, close_future).await {
        Ok(Ok(())) => {
            tracing::debug!(
                requested_url = %requested_url,
                "page close finished"
            );
        }

        Ok(Err(err)) => {
            tracing::debug!(
                requested_url = %requested_url,
                error = %err,
                "page close failed"
            );
        }

        Err(_) => {
            tracing::warn!(
                requested_url = %requested_url,
                timeout_ms = timeout.as_millis(),
                "page close timed out; continuing crawl"
            );
        }
    }
}
