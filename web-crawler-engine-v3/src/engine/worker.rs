//! Single-request worker path.
//!
//! This module owns the state machine for one crawl request.
//!
//! A worker does not mutate crawl-wide state. It receives one request and
//! returns exactly one `CrawlPageResult` to the coordinator.
//!
//! The worker path is intentionally uniform:
//!
//! ```text
//! request
//!   -> try reusable cache replay
//!   -> lease browser page capacity on cache miss
//!   -> perform live browser capture
//!   -> apply session termination policy if the page result requires it
//!   -> return page result
//! ```
//!
//! This is the same path for seed pages, discovered pages, warm-cache hits, and
//! cold-cache browser visits. The worker must not grow a special seed-only or
//! depth-zero fast path. Hop depth affects scheduling/expansion policy, not the
//! evidence collected for an opened page.
//!
//! ## Failure semantics
//!
//! Ordinary page failures are returned as `CrawlPageOutcome::Failed`. They do
//! not fail the whole crawl.
//!
//! Critical orchestration failures may still return `CrawlEngineError`, but the
//! worker should prefer page-level failure results for normal web weirdness:
//! browser navigation errors, environment failures, timeouts, blocked pages, or
//! unhealthy sessions.
//!
//! ## Session health
//!
//! Browser session termination is driven by the page result flag
//! `should_terminate_session`. This keeps the decision visible in output while
//! allowing the session pool to retire poisoned browser state before more work
//! is assigned to it.

use serde::{
    de::DeserializeOwned,
    Serialize,
};
use web_crawler_db::CacheKey;

use crate::{
    error::CrawlEngineResult,
    input::CrawlRequest,
    output::{
        CrawlPageOutcome,
        CrawlPageResult,
    },
    scheduler::BrowserProfileAssignment,
    sessions::SessionPool,
    store::CrawlArtifactSink,
};

use super::CrawlEngine;

impl<P, S> CrawlEngine<P, S>
where
    P: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    S: CrawlArtifactSink<P> + Send + Sync,
{
    /// Execute one page request through cache replay or live browser capture.
    ///
    /// This method does not mutate crawl state. It may be run concurrently with
    /// other page jobs.
    ///
    /// Flow:
    ///
    /// 1. Try metadata-backed cache replay before acquiring browser capacity.
    /// 2. Lease one browser page slot on cache miss.
    /// 3. Open/extract/capture the live page.
    /// 4. Retire the browser session if the returned page failure requires it.
    /// 5. Return exactly one page result.
    pub(crate) async fn crawl_one_request(
        &self,
        request: CrawlRequest<P>,
        cache_key: CacheKey,
        assignment: BrowserProfileAssignment,
        sessions: SessionPool,
    ) -> CrawlEngineResult<CrawlPageResult<P>> {
        let requested_url = request.requested_url.clone();

        // Fast path: cache replay happens before browser capacity is claimed.
        //
        // A cache hit is still page evidence. Cached anchors remain available to
        // sinks and to optional frontier expansion by the coordinator.
        if let Some(cached_result) = self.try_cache(&request, &cache_key).await? {
            tracing::debug!(
                requested_url = %requested_url,
                "page request served from cache"
            );

            return Ok(cached_result);
        }

        let lease = match sessions.lease_page_slot(&request).await {
            Ok(lease) => lease,

            Err(err) => {
                tracing::warn!(
                    requested_url = %requested_url,
                    error = %err,
                    retryable = err.is_retryable_environment_failure(),
                    should_terminate_session = err.should_terminate_session(),
                    "failed to lease browser page slot"
                );

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

        let result = self
            .crawl_live_page(
                &request,
                cache_key.clone(),
                assignment,
                session,
            )
            .await?;

        tracing::debug!(
            requested_url = %requested_url,
            "page request worker completed"
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
                "page request failed"
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
}

