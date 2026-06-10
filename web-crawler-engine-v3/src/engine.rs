//! Crawl engine entry point.
//!
//! This module is the orchestration layer of the crawler.
//!
//! It coordinates:
//!
//! - crawl run state,
//! - seed-aware frontier scheduling,
//! - visit policy,
//! - browser session/profile assignment,
//! - cache lookup,
//! - live browser work,
//! - artifact persistence,
//! - sink callbacks,
//! - frontier expansion.
//!
//! ## Cache and tag semantics
//!
//! The SQLite cache stores reusable page artifacts. A cache artifact is not the
//! same thing as caller/application ownership.
//!
//! Multiple seeds, entities, categories, campaigns, or manual runs may point at
//! the same cached artifact. Therefore request tags must be merged onto cache
//! entries both:
//!
//! - when a live page is opened and saved,
//! - when a page is served from cache.
//!
//! The second case is easy to miss. If a request hits an existing artifact and
//! the engine does not merge the request tags, warm-cache crawls will fail to
//! record new associations.
//!
//! The key invariant is:
//!
//! ```text
//! seed/request tags flow downward through discovered pages
//! and are merged onto every cache entry reached by those requests.
//! ```
//!
//! ## Generic provenance lane
//!
//! The engine remains generic over `P`, but `P` is currently a phantom typed lane
//! carried by request/result shapes. Durable caller association should happen via
//! tags, not by trying to serialize arbitrary `P` into cache artifacts.
//!
//! ## Concurrency model
//!
//! The coordinator owns `CrawlRunState`. Workers do not mutate the frontier,
//! visited sets, budgets, or result list.
//!
//! The coordinator:
//!
//! - pops frontier items,
//! - dedupes,
//! - checks visit policy,
//! - reserves per-seed page slots,
//! - dispatches page jobs,
//! - records completed results,
//! - expands the frontier from completed opened pages.
//!
//! Workers:
//!
//! - try cache,
//! - lease browser page capacity,
//! - open/extract/snapshot,
//! - save cache,
//! - close the page on a short best-effort timeout,
//! - return one `CrawlPageResult`.
//!
//! This keeps the mutable crawl brain in one place while browser/page work runs
//! concurrently.
//!
//! ## Timeout safety
//!
//! Browser automation cleanup is not allowed to hold the crawl hostage.
//!
//! A live page crawl is wrapped in a crawler-level timeout. Individual browser
//! operations may have lower-level timeouts, but this outer envelope guarantees a
//! worker eventually returns a page result.
//!
//! Page close is also treated as best-effort cleanup. If Chrome/CDP wedges while
//! closing a tab, the crawler records the page result and lets session health
//! policy decide whether that browser session should be retired.

use std::{
    marker::PhantomData,
    sync::Arc,
    time::Duration,
};

use colored_json::Paint;
use futures::{
    stream::FuturesUnordered,
    StreamExt,
};
use serde::{
    de::DeserializeOwned,
    Serialize,
};
use web_browser_driver::{
    AnchorExtractor,
    BrowserDriver,
    BrowserSession,
    OpenPageOptions,
    PageInfoExtractor,
};

use crate::config::CrawlEngineConfig;
use crate::error::CrawlEngineResult;
use crate::input::CrawlRequest;
use crate::output::{
    CrawlPageOutcome,
    CrawlPageResult,
    CrawlRunResult,
    SnapshotDecision,
};
use crate::policy::{
    CacheDecision,
    CrawlPolicy,
    VisitDecision,
};
use crate::scheduler::{
    BrowserProfileAssignment,
    BrowserProfileStrategy,
    SessionScheduler,
};
use crate::sessions::SessionPool;
use crate::state::CrawlRunState;
use crate::store::{
    CrawlArtifactSink,
    NoopCrawlArtifactSink,
};

use crate::sqlite_cache::{
    CacheEntry,
    CacheEntryMetadata,
    CacheKey,
    CachePayload,
    CachePayloadCompression,
    CachePayloadRole,
    SqliteCache,
};

// ————————————————————————————————————————————————————————————————————————————
// Small utilities
// ————————————————————————————————————————————————————————————————————————————

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// Main crawler orchestration type.
///
/// `P` is retained as a typed lane for future caller-specific APIs. Current
/// durable association should be expressed through `CrawlRequest::tags`.
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
    /// Attach a SQLite cache.
    pub fn with_sqlite_cache(mut self, cache: SqliteCache) -> Self {
        self.sqlite_cache = Some(cache);
        self
    }

    /// Remove any attached cache.
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

    /// Attempt to serve a request from cache.
    ///
    /// Returns `Some(result)` only if a usable cached artifact was found.
    ///
    /// Important: cache hits still merge request tags onto the cache entry. This
    /// preserves new caller associations even when no live browser work occurs.
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

        // Cache hit association write.
        //
        // If a second seed/entity/category reaches an already-cached artifact,
        // the artifact must still gain that request's tags.
        if !request.tags.is_empty() {
            cache.tag(cache_key, &request.tags).await?;
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
            .filter_map(|value| serde_json::from_value(value.clone()).ok())
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

    pub async fn crawl(
        &self,
        seeds: Vec<CrawlRequest<P>>,
    ) -> CrawlEngineResult<CrawlRunResult<P>> {
        let mut state = CrawlRunState::new(self.config.limits.clone(), seeds);

        let sessions = SessionPool::new(
            self.browser_driver.clone(),
            self.profile_strategy.clone(),
            self.profile_root.clone(),
            self.config.concurrency.max_sessions,
            self.config.concurrency.max_pages_per_session,
            self.config.concurrency.max_concurrent_pages_per_session,
        )
        .with_rotation_callback(|profile_key, reason| {
            tracing::info!(
                target: "crawler::session",
                profile_key = profile_key,
                reason = ?reason,
                "browser session rotated"
            );
        });

        let mut inflight = FuturesUnordered::new();
        let max_concurrent_pages = self.config.concurrency.max_concurrent_pages.max(1);

        loop {
            // Fill the in-flight set up to the configured global page
            // concurrency. The coordinator remains the only owner of
            // `CrawlRunState`.
            while inflight.len() < max_concurrent_pages && state.should_continue() {
                let Some(item) = state.pop_next() else {
                    break;
                };

                let request = item.request;

                if state.has_visited_fetch_key(&request) {
                    eprintln!("👻 already seen, skipping {}", request.requested_url);
                    continue;
                }

                match self.policy.evaluate_visit(&request, &self.config.limits) {
                    VisitDecision::Visit => {}

                    VisitDecision::Skip { reason } => {
                        state.mark_visited(&request);

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

                if !state.reserve_seed_slot(&request) {
                    state.mark_visited(&request);

                    let result = CrawlPageResult {
                        request,
                        cache_key: None,
                        cache_decision: None,
                        outcome: CrawlPageOutcome::Skipped {
                            reason: "seed page budget exhausted".into(),
                        },
                    };

                    self.sink.record_page(&result).await?;
                    state.record_page(result);
                    continue;
                }

                state.mark_visited(&request);

                let assignment = SessionScheduler::new(self.profile_strategy.clone())
                    .assign_profile(&request);

                let cache_key = CacheKey::for_request(
                    request.requested_url.clone(),
                    None,
                );

                let sessions = sessions.clone();

                inflight.push(async move {
                    self.crawl_one_request(
                        request,
                        cache_key,
                        assignment,
                        sessions,
                    )
                    .await
                });
            }

            if inflight.is_empty() {
                break;
            }

            let Some(result) = inflight.next().await else {
                break;
            };

            let result = result?;

            self.sink.record_page(&result).await?;

            let opened_anchors = match &result.outcome {
                CrawlPageOutcome::Opened { anchors, .. } => Some(anchors.clone()),
                _ => None,
            };

            let request = result.request.clone();

            state.complete_reserved_page(result);

            if let Some(anchors) = opened_anchors {
                state.expand_from_anchors(&request, &anchors, &self.policy);
            }
        }

        sessions.shutdown_all().await;

        Ok(state.finish())
    }

    /// Execute one page request.
    ///
    /// This method does not mutate crawl state. It may be run concurrently with
    /// other page jobs.
    ///
    /// Flow:
    ///
    /// - try cache,
    /// - if cache misses, lease browser page capacity,
    /// - open/extract/snapshot live page under a hard crawler timeout,
    /// - save cache,
    /// - return exactly one page result.
    async fn crawl_one_request(
        &self,
        request: CrawlRequest<P>,
        cache_key: CacheKey,
        assignment: BrowserProfileAssignment,
        sessions: SessionPool,
    ) -> CrawlEngineResult<CrawlPageResult<P>> {
        // Fast path: try cache before acquiring browser page capacity.
        //
        // A cache hit behaves like replayed page evidence. Cached artifacts
        // contain extracted anchors, so they still expand the frontier once the
        // coordinator receives the result.
        if let Some(cached_result) = self.try_cache(&request, &cache_key).await? {
            return Ok(cached_result);
        }

        let lease = match sessions.lease_page_slot(&request).await {
            Ok(lease) => lease,

            Err(err) => {
                return Ok(CrawlPageResult {
                    request,
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

        let session = lease.session();

        let page_timeout = self.config.page_open_timeout;
        let requested_url = request.requested_url.clone();

        let result = match tokio::time::timeout(
            page_timeout,
            self.crawl_live_page(
                &request,
                cache_key.clone(),
                assignment,
                session,
            ),
        )
        .await
        {
            Ok(result) => result?,

            Err(_) => {
                tracing::warn!(
                    requested_url = %requested_url,
                    timeout_ms = page_timeout.as_millis(),
                    "page crawl timed out"
                );

                CrawlPageResult {
                    request: request.clone(),
                    cache_key: Some(cache_key),
                    cache_decision: None,
                    outcome: CrawlPageOutcome::Failed {
                        error: format!(
                            "page crawl timed out after {}ms",
                            page_timeout.as_millis()
                        ),
                        retryable: true,
                        should_terminate_session: true,
                    },
                }
            }
        };

        tracing::debug!(
            requested_url = %requested_url,
            "page crawl task completed"
        );

        if let CrawlPageOutcome::Failed {
            error,
            retryable,
            should_terminate_session,
        } = &result.outcome
        {
            tracing::warn!(
                requested_url = %requested_url,
                error = %error,
                retryable = *retryable,
                should_terminate_session = *should_terminate_session,
                "page crawl failed"
            );

            eprintln!(
                "{}",
                format!(
                    "❌ {} | retryable={} terminate_session={} | {}",
                    requested_url, retryable, should_terminate_session, error
                )
                .red()
            );
        }

        if matches!(
            &result.outcome,
            CrawlPageOutcome::Failed {
                should_terminate_session: true,
                ..
            }
        ) {
            sessions.terminate_for_request(&request).await;
        }

        Ok(result)
    }

    /// Execute a live browser crawl for a single frontier item.
    ///
    /// Responsibilities:
    ///
    /// - open the page using the assigned browser session,
    /// - perform generic extraction,
    /// - capture HTML snapshot if configured,
    /// - persist a SQLite cache entry,
    /// - merge request tags onto the cache entry,
    /// - close the page on a short best-effort timeout.
    async fn crawl_live_page(
        &self,
        request: &CrawlRequest<P>,
        cache_key: CacheKey,
        assignment: BrowserProfileAssignment,
        session: Arc<BrowserSession>,
    ) -> CrawlEngineResult<CrawlPageResult<P>> {
        use chrono::Local;

        eprintln!(
            "{} {}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            format!("🌐 {}", request.requested_url.as_str().magenta()).cyan()
        );

        let mut open_options = OpenPageOptions::new(request.requested_url.clone());

        // The engine already wraps the whole live page crawl in page_open_timeout.
        // Do not apply the exact same timeout again inside the browser open call,
        // or slow-but-scrapeable pages get killed twice by the same clock.
        open_options.timeout = None;

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
                "page opened but html capture failed; not saving cache entry"
            );
        }

        let snapshot = match &html {
            Some(html) => SnapshotDecision::Captured {
                html_bytes: html.len(),
                body_sha256_hex: crate::sqlite_cache::model::sha256_hex(html.as_bytes()),
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

        // Save to SQLite if HTML was captured and caching is enabled.
        //
        // Request tags are persisted here. `SqliteCache::put` must merge them
        // with any existing tags for this cache entry.
        if self.config.cache_enabled {
            if let Some(cache) = &self.sqlite_cache {
                if let Some(html) = html {
                    let body = html.into_bytes();
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
                            requested_host: request
                                .requested_url
                                .host_str()
                                .map(|host| host.to_string()),
                            profile_key_json: serde_json::to_value(&assignment.key)
                                .unwrap_or_default(),
                            namespace: None,
                        },
                        crate::sqlite_cache::CacheResponseInfo {
                            final_url: Some(opened.resolution.final_url.to_string()),
                            final_host: opened
                                .resolution
                                .final_url
                                .host_str()
                                .map(|host| host.to_string()),
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
                        .filter_map(|error| serde_json::to_value(error).ok())
                        .collect();

                    let cache_entry = CacheEntry {
                        metadata,
                        payloads: vec![snapshot_payload],
                        tags: request.tags.clone(),
                    };

                    match cache.put(&cache_entry).await {
                        Ok(()) => {
                            tracing::debug!(
                                requested_url = %request.requested_url,
                                key_digest = %crate::sqlite_cache::cache_key_digest(&cache_key)
                                    .unwrap_or_else(|_| "<digest-error>".into()),
                                tag_count = request.tags.len(),
                                "saved sqlite cache entry"
                            );
                        }

                        Err(err) => {
                            tracing::warn!(
                                requested_url = %request.requested_url,
                                error = %err,
                                "failed to save sqlite cache entry"
                            );
                        }
                    }
                }
            }
        }

        let close_timeout = Duration::from_secs(3);

        match tokio::time::timeout(close_timeout, opened.page.close()).await {
            Ok(Ok(())) => {
                tracing::debug!(
                    requested_url = %request.requested_url,
                    "page close finished"
                );
            }

            Ok(Err(err)) => {
                tracing::debug!(
                    requested_url = %request.requested_url,
                    error = %err,
                    "page close failed"
                );
            }

            Err(_) => {
                tracing::warn!(
                    requested_url = %request.requested_url,
                    timeout_ms = close_timeout.as_millis(),
                    "page close timed out; continuing crawl"
                );
            }
        }

        tracing::debug!(
            requested_url = %request.requested_url,
            "live page crawl returning result"
        );

        Ok(result)
    }
}
