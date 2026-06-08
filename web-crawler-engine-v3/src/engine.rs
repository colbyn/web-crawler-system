//! Crawl engine entry point.
//!
//! This module is the orchestration layer of the crawler. It is responsible for:
//! - Managing the crawl frontier and run state
//! - Coordinating browser sessions via `SessionPool` (with health-aware rotation)
//! - Performing cache lookups with request-addressed keys
//! - Executing live browser work when needed
//! - Persisting self-contained `CachedPageArtifact`s
//!
//! The engine is intentionally generic over provenance (`P`) so that different
//! callers can attach their own business metadata without polluting the crawler core.

use std::marker::PhantomData;

use serde::{de::DeserializeOwned, Serialize};
use web_browser_driver::{
    AnchorExtractor, BrowserDriver, OpenPageOptions, PageInfoExtractor,
};

use crate::{
    cache::{CacheKey, CrawlCacheStore},
    config::CrawlEngineConfig,
    error::CrawlEngineResult,
    input::CrawlRequest,
    output::{CrawlPageOutcome, CrawlPageResult, CrawlRunResult, SnapshotDecision},
    policy::{CrawlPolicy, VisitDecision},
    scheduler::{BrowserProfileAssignment, BrowserProfileStrategy, SessionScheduler},
    sessions::SessionPool,
    state::CrawlRunState,
    store::{CrawlArtifactSink, NoopCrawlArtifactSink},
    CacheDecision,
};

pub struct CrawlEngine<P, C = crate::cache::FsCrawlCacheStore, S = NoopCrawlArtifactSink> {
    pub config: CrawlEngineConfig,
    pub policy: CrawlPolicy,
    pub browser_driver: BrowserDriver,
    pub cache_store: Option<C>,
    pub sink: S,
    pub profile_root: std::path::PathBuf,
    pub profile_strategy: BrowserProfileStrategy,
    _provenance: PhantomData<P>,
}

impl<P> CrawlEngine<P, crate::cache::FsCrawlCacheStore> {
    pub fn new(
        config: CrawlEngineConfig,
        policy: CrawlPolicy,
        browser_driver: BrowserDriver,
        cache_store: Option<crate::cache::FsCrawlCacheStore>,
        profile_root: std::path::PathBuf,
        profile_strategy: BrowserProfileStrategy,
    ) -> Self {
        Self {
            config,
            policy,
            browser_driver,
            cache_store,
            sink: NoopCrawlArtifactSink,
            profile_root,
            profile_strategy,
            _provenance: PhantomData,
        }
    }
}

impl<P, C, S> CrawlEngine<P, C, S>
where
    P: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    C: CrawlCacheStore + Send + Sync,
    S: CrawlArtifactSink<P> + Send + Sync,
{
    pub fn with_sink<S2>(self, sink: S2) -> CrawlEngine<P, C, S2> {
        CrawlEngine {
            config: self.config,
            policy: self.policy,
            browser_driver: self.browser_driver,
            cache_store: self.cache_store,
            sink,
            profile_root: self.profile_root,
            profile_strategy: self.profile_strategy,
            _provenance: PhantomData,
        }
    }

    /// Attempts to serve a request from the cache.
    /// Returns `Some(result)` only if a usable cached artifact was found.
    async fn try_cache(
        &self,
        request: &CrawlRequest<P>,
        cache_key: &CacheKey,
    ) -> CrawlEngineResult<Option<CrawlPageResult<P>>> {
        if !self.config.cache_enabled {
            return Ok(None);
        }
        let Some(store) = &self.cache_store else {
            return Ok(None);
        };

        let artifact = match store.load(cache_key).await {
            Ok(Some(a)) => a,
            _ => return Ok(None),
        };

        let decision = self.policy.evaluate_cache(&artifact);

        if matches!(decision, CacheDecision::Use) {
            let outcome = CrawlPageOutcome::Opened {
                resolution: artifact.resolution.clone(),
                status_code: artifact.status_code,
                telemetry: artifact.telemetry.clone(),
                non_critical_errors: artifact.non_critical_errors.clone(),
                page_info: artifact.extracted.page_info.clone(),
                anchors: artifact.extracted.anchors.clone(),
                snapshot: if artifact.snapshot.body.is_empty() {
                    SnapshotDecision::NotRequested
                } else {
                    SnapshotDecision::Captured {
                        html_bytes: artifact.snapshot.body.len(),
                        body_sha256_hex: artifact.snapshot.body_sha256_hex.clone(),
                    }
                },
            };

            let result = CrawlPageResult {
                request: request.clone(),
                cache_key: Some(cache_key.clone()),
                cache_decision: Some(decision),
                outcome,
            };
            return Ok(Some(result));
        }

        Ok(None)
    }

    pub async fn crawl(&self, seeds: Vec<CrawlRequest<P>>) -> CrawlEngineResult<CrawlRunResult<P>> {
        let mut state = CrawlRunState::new(self.config.limits.clone(), seeds);

        // Session pool with health-aware + page-count rotation.
        // Rotation is critical for long-running batch crawls to prevent
        // memory leaks, CDP degradation, and profile contamination.
        let mut sessions = SessionPool::new(
            &self.browser_driver,
            self.profile_strategy.clone(),
            self.profile_root.clone(),
            150, // pages per session before proactive rotation
        )
        .with_rotation_callback(|profile_key, reason| {
            tracing::info!(
                target: "crawler::session",
                profile_key = profile_key,
                reason = ?reason,
                "Browser session rotated"
            );
        });

        while state.should_continue() {
            let Some(item) = state.pop_next() else {
                break;
            };
            let request = item.request;

            if state.has_seen_fetch_key(&request) {
                continue;
            }
            state.mark_visited(&request);

            match self.policy.evaluate_visit(&request, &self.config.limits) {
                VisitDecision::Visit => {}
                VisitDecision::Skip { reason } => {
                    let result = CrawlPageResult {
                        request,
                        cache_key: None,
                        cache_decision: None,
                        outcome: CrawlPageOutcome::Skipped { reason },
                    };
                    self.sink.record_page(&result).await?;
                    state.record_page(result);
                    continue;
                }
            }

            let assignment = SessionScheduler::new(self.profile_strategy.clone())
                .assign_profile(&request);

            let cache_key = CacheKey::for_request(
                request.requested_url.clone(),
                assignment.key.clone(),
                None,
            );

            // Fast path: try cache before acquiring a browser session
            if let Some(cached_result) = self.try_cache(&request, &cache_key).await? {
                self.sink.record_page(&cached_result).await?;
                state.record_page(cached_result);
                continue;
            }

            // Live path
            let session = match sessions.get_or_start(&request).await {
                Ok(s) => s,
                Err(err) => {
                    let result = CrawlPageResult {
                        request,
                        cache_key: Some(cache_key),
                        cache_decision: None,
                        outcome: CrawlPageOutcome::Failed {
                            error: err.to_string(),
                            retryable: err.is_retryable_environment_failure(),
                            should_terminate_session: true,
                        },
                    };
                    self.sink.record_page(&result).await?;
                    state.record_page(result);
                    continue;
                }
            };

            let result = self
                .crawl_live_page(&request, cache_key, assignment, session)
                .await?;

            if matches!(
                &result.outcome,
                CrawlPageOutcome::Failed {
                    should_terminate_session: true,
                    ..
                }
            ) {
                sessions.terminate_for_request(&request).await;
            }

            self.sink.record_page(&result).await?;
            state.record_page(result);
        }

        sessions.shutdown_all().await;
        Ok(state.finish())
    }

    /// Executes a live browser crawl for a single frontier item.
    ///
    /// Responsibilities:
    /// - Open the page using the provided session
    /// - Perform generic extraction (PageInfo + Anchors)
    /// - Capture HTML snapshot when configured
    /// - Persist a self-contained `CachedPageArtifact`
    async fn crawl_live_page(
        &self,
        request: &CrawlRequest<P>,
        cache_key: CacheKey,
        _assignment: BrowserProfileAssignment,
        session: &mut web_browser_driver::BrowserSession,
    ) -> CrawlEngineResult<CrawlPageResult<P>> {
        let mut open_options = OpenPageOptions::new(request.requested_url.clone());
        open_options.timeout = Some(self.config.page_open_timeout);

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
        let anchors = AnchorExtractor::extract(&opened.page).await.unwrap_or_default();

        let html = if self.policy.snapshot.capture_html {
            opened.page.html().await.ok()
        } else {
            None
        };

        let snapshot = match &html {
            Some(h) => SnapshotDecision::Captured {
                html_bytes: h.len(),
                body_sha256_hex: crate::cache::sha256_hex(h.as_bytes()),
            },
            None if self.policy.snapshot.capture_html => SnapshotDecision::Rejected {
                reason: "failed to capture HTML".into(),
            },
            None => SnapshotDecision::NotRequested,
        };

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
                snapshot: snapshot.clone(),
            },
        };

        // Persist artifact if HTML was captured and caching is enabled
        if let (true, Some(store)) = (self.config.cache_enabled, &self.cache_store) {
            if let Some(html_str) = html {
                let body = html_str.into_bytes();
                let body_sha256_hex = crate::cache::sha256_hex(&body);

                let now_ms: i64 = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);

                let artifact = crate::cache::CachedPageArtifact {
                    artifact_version: crate::cache::CACHED_PAGE_ARTIFACT_VERSION,
                    cache_key: cache_key.clone(),
                    stored_at_unix_ms: now_ms,
                    producer: crate::cache::CacheProducerInfo {
                        engine_name: "web-crawler-engine-v3".to_string(),
                        engine_version: "0.1.0".to_string(),
                        driver_version: None,
                        cache_policy_version: self.policy.cache.policy_version,
                    },
                    resolution: opened.resolution,
                    status_code: opened.status_code,
                    telemetry: opened.telemetry,
                    non_critical_errors: opened.non_critical_errors,
                    snapshot: crate::cache::CacheSnapshot {
                        captured_at_unix_ms: now_ms,
                        content_type: Some("text/html".to_string()),
                        body,
                        compression: crate::cache::SnapshotCompression::None,
                        body_sha256_hex,
                    },
                    extracted: crate::cache::CachedExtractedFacts {
                        page_info,
                        anchors,
                    },
                };

                if let Err(e) = store.save(&cache_key, &artifact).await {
                    tracing::warn!("failed to save cache artifact for {:?}: {}", cache_key, e);
                }
            }
        }

        let _ = opened.page.close().await;
        Ok(result)
    }
}

