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

use colored_json::Paint;
use serde::{de::DeserializeOwned, Serialize};
use web_browser_driver::{
    AnchorExtractor, BrowserDriver, OpenPageOptions, PageInfoExtractor,
};




use crate::config::CrawlEngineConfig;
use crate::error::CrawlEngineResult;
use crate::input::CrawlRequest;
use crate::output::{CrawlPageOutcome, CrawlPageResult, CrawlRunResult, SnapshotDecision};
use crate::scheduler::{BrowserProfileAssignment, BrowserProfileStrategy, SessionScheduler};
use crate::sessions::SessionPool;
use crate::state::CrawlRunState;
use crate::store::{CrawlArtifactSink, NoopCrawlArtifactSink};

use crate::policy::{CacheDecision, CrawlPolicy, VisitDecision};
use crate::sqlite_cache::{
    CacheEntry, CacheEntryMetadata, CacheKey, CachePayload, CachePayloadCompression,
    CachePayloadRole, SqliteCache,
};

// ————————————————————————————————————————————————————————————————————————————
// SMALL UTILS
// ————————————————————————————————————————————————————————————————————————————

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct CrawlEngine<P, S = NoopCrawlArtifactSink> {
    pub config: CrawlEngineConfig,
    pub policy: CrawlPolicy,
    pub browser_driver: BrowserDriver,
    pub sqlite_cache: Option<SqliteCache>,
    pub sink: S,
    pub profile_root: std::path::PathBuf,
    pub profile_strategy: BrowserProfileStrategy,
    _provenance: PhantomData<P>,
}

impl<P> CrawlEngine<P> {
    pub fn new(
        config: CrawlEngineConfig,
        policy: CrawlPolicy,
        browser_driver: BrowserDriver,
        sqlite_cache: Option<SqliteCache>,
        profile_root: std::path::PathBuf,
        profile_strategy: BrowserProfileStrategy,
    ) -> Self {
        Self {
            config,
            policy,
            browser_driver,
            sqlite_cache,
            sink: NoopCrawlArtifactSink,
            profile_root,
            profile_strategy,
            _provenance: PhantomData,
        }
    }
}

impl<P, S> CrawlEngine<P, S>
where
    P: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    S: CrawlArtifactSink<P> + Send + Sync,
{
    /// Attach a `SqliteCache`
    pub fn with_sqlite_cache(mut self, cache: SqliteCache) -> Self {
        self.sqlite_cache = Some(cache);
        self
    }

    /// Remove any attached cache
    pub fn without_cache(mut self) -> Self {
        self.sqlite_cache = None;
        self
    }

    pub fn with_sink<S2>(self, sink: S2) -> CrawlEngine<P, S2> {
        CrawlEngine {
            config: self.config,
            policy: self.policy,
            browser_driver: self.browser_driver,
            sqlite_cache: self.sqlite_cache,
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

        let Some(cache) = &self.sqlite_cache else {
            return Ok(None);
        };

        let Some(entry) = cache.get(cache_key).await else {
            return Ok(None);
        };

        let decision = self.policy.evaluate_cache(&entry);

        if !matches!(decision, CacheDecision::Use) {
            return Ok(None);
        }

        let Some(primary) = entry.metadata.primary_payload() else {
            return Ok(None);
        };

        let resolution = serde_json::from_value(
            entry
                .metadata
                .extracted_json
                .get("resolution")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )
        .ok();

        let page_info = serde_json::from_value(
            entry
                .metadata
                .extracted_json
                .get("page_info")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )
        .ok();

        let anchors = serde_json::from_value(
            entry
                .metadata
                .extracted_json
                .get("anchors")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .unwrap_or_default();

        let telemetry = serde_json::from_value(entry.metadata.telemetry_json.clone()).ok();

        let non_critical_errors = entry
            .metadata
            .non_critical_errors_json
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect();

        let Some(resolution) = resolution else {
            return Ok(None);
        };

        let Some(telemetry) = telemetry else {
            return Ok(None);
        };

        let outcome = CrawlPageOutcome::Opened {
            resolution,
            status_code: entry.metadata.response.status_code,
            telemetry,
            non_critical_errors,
            page_info,
            anchors,
            snapshot: if primary.byte_len > 0 {
                SnapshotDecision::Captured {
                    html_bytes: primary.byte_len,
                    body_sha256_hex: primary.sha256_hex.clone(),
                }
            } else {
                SnapshotDecision::NotRequested
            },
        };

        Ok(Some(CrawlPageResult {
            request: request.clone(),
            cache_key: Some(cache_key.clone()),
            cache_decision: Some(decision),
            outcome,
        }))
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

            if state.has_visited_fetch_key(&request) {
                eprintln!("👻 already seen, skipping {}", request.requested_url);
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

            // Fast path: try cache before acquiring a browser session.
            //
            // Important: a cache hit should behave like replayed page evidence.
            // Cached artifacts contain extracted anchors, so they must still expand
            // the frontier; otherwise warm-cache crawls stop at the seed page.
            if let Some(cached_result) = self.try_cache(&request, &cache_key).await? {
                self.sink.record_page(&cached_result).await?;
                state.record_page(cached_result.clone());

                if let CrawlPageOutcome::Opened { anchors, .. } = &cached_result.outcome {
                    state.expand_from_anchors(&request, anchors, &self.policy);
                }

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

            // Record the result
            self.sink.record_page(&result).await?;
            state.record_page(result.clone());

            // === CRITICAL: Expand frontier from discovered anchors ===
            if let CrawlPageOutcome::Opened { anchors, .. } = &result.outcome {
                state.expand_from_anchors(&request, anchors, &self.policy);
            }
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
        assignment: BrowserProfileAssignment,
        session: &mut web_browser_driver::BrowserSession,
    ) -> CrawlEngineResult<CrawlPageResult<P>> {
        {
            eprintln!("{}", format!(
                "🌐 {}",
                request.requested_url.as_str().magenta(),
            ).cyan());
        }
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
                body_sha256_hex: crate::sqlite_cache::model::sha256_hex(h.as_bytes()),
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
        // === Save to new SqliteCache if enabled ===
        if self.config.cache_enabled {
            if let Some(cache) = &self.sqlite_cache {
                if let Some(html_str) = html {
                    let body = html_str.into_bytes();
                    let now_ms = now_unix_ms();

                    let snapshot_payload = CachePayload::new(
                        "primary",
                        CachePayloadRole::PrimarySnapshot,
                        Some("text/html".to_string()),
                        CachePayloadCompression::None,
                        body,
                    );

                    let mut metadata = CacheEntryMetadata::new_page(
                        cache_key.clone(),
                        now_ms,
                        crate::sqlite_cache::CacheProducerInfo {
                            engine_name: "web-crawler-engine-v3".to_string(),
                            engine_version: "0.1.0".to_string(),
                            driver_version: None,
                            cache_policy_version: self.policy.cache.policy_version,
                        },
                        crate::sqlite_cache::CacheRequestInfo {
                            requested_url: request.requested_url.to_string(),
                            requested_host: request.requested_url.host_str().map(|s| s.to_string()),
                            profile_key_json: serde_json::to_value(&assignment.key).unwrap_or_default(),
                            namespace: None,
                        },
                        crate::sqlite_cache::CacheResponseInfo {
                            final_url: Some(opened.resolution.final_url.to_string()),
                            final_host: opened
                                .resolution
                                .final_url
                                .host_str()
                                .map(|s| s.to_string()),
                            status_code: opened.status_code,
                            content_type: Some("text/html".to_string()),
                        },
                        vec![snapshot_payload.descriptor.clone()],
                    );

                    metadata.extracted_json = serde_json::json!({
                        "page_info": page_info,
                        "anchors": anchors,
                        "resolution": opened.resolution,
                    });

                    metadata.telemetry_json =
                        serde_json::to_value(&opened.telemetry).unwrap_or_default();

                    metadata.non_critical_errors_json = opened
                        .non_critical_errors
                        .iter()
                        .filter_map(|e| serde_json::to_value(e).ok())
                        .collect();

                    let cache_entry = CacheEntry {
                        metadata,
                        payloads: vec![snapshot_payload],
                        tags: vec![],
                    };

                    if let Err(e) = cache.put(&cache_entry).await {
                        tracing::warn!("failed to save sqlite cache entry: {}", e);
                    }
                }
            }
        }

        let _ = opened.page.close().await;
        Ok(result)
    }
}


