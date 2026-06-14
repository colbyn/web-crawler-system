//! Crawl coordination loop.
//!
//! This module owns the mutable crawl brain.
//!
//! The coordinator is responsible for:
//!
//! - constructing per-run crawl state,
//! - creating and shutting down the browser session pool,
//! - popping requests from the frontier,
//! - applying visit policy,
//! - enforcing per-seed and global crawl budgets,
//! - dispatching request workers,
//! - recording completed page results,
//! - sending page results to the configured sink,
//! - expanding the frontier from extracted anchors when policy/budget allow it.
//!
//! It deliberately does **not** know how to replay a cache entry, open a browser
//! page, extract page evidence, or persist cache artifacts. Those phases live in
//! sibling modules.
//!
//! ## Uniform path
//!
//! The coordinator should not create a special seed-only execution path.
//!
//! A crawl with `max_hop_depth = 0` still uses the same request worker as a
//! deeper crawl. Anchors may still be extracted, persisted, returned, and sent
//! to sinks. The only difference is that expansion will not produce follow-up
//! visits once the depth budget rejects them.
//!
//! This keeps operational behavior robust: warm cache, live browser capture,
//! tag association, extraction, sink recording, and failure handling remain the
//! same across crawl depths.
//!
//! ## Online frontier scoring
//!
//! Frontier scoring is applied only when opened page results expose anchors.
//! This means cache replay and live browser capture both feed the same scoring
//! path because both produce normal `CrawlPageOutcome::Opened` evidence.
//!
//! Scoring remains a scheduling hint:
//!
//! - policy still decides whether a URL is in scope,
//! - visit policy still decides whether a request may be opened,
//! - cache identity remains based on requested URL,
//! - workers remain unaware of scoring,
//! - low-scored URLs are not filtered by scoring.
//!
//! The coordinator builds the optional scorer once for a crawl invocation and
//! passes it into state expansion. The state/frontier layer owns the actual
//! enqueue operation and priority ordering.

use futures::{
    stream::FuturesUnordered,
    StreamExt,
};
use serde::{
    de::DeserializeOwned,
    Serialize,
};
use web_crawler_db::CacheKey;

use crate::{
    config::FrontierScoringConfig,
    error::CrawlEngineResult,
    input::CrawlRequest,
    output::{
        CrawlPageOutcome,
        CrawlPageResult,
        CrawlRunResult,
    },
    policy::VisitDecision,
    scheduler::SessionScheduler,
    sessions::SessionPool,
    state::CrawlRunState,
    store::CrawlArtifactSink,
    url_score::FrontierUrlScorer,
};

use super::CrawlEngine;

impl<P, S> CrawlEngine<P, S>
where
    P: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    S: CrawlArtifactSink<P> + Send + Sync,
{
    /// Run a crawl for the supplied seed requests.
    ///
    /// This method owns crawl-state mutation. Page workers may run
    /// concurrently, but they return completed page results back to this
    /// coordinator. Workers do not mutate the frontier, visited sets, budgets,
    /// or result collection directly.
    ///
    /// The loop has three phases:
    ///
    /// 1. Fill the in-flight worker set while there is capacity and queued work.
    /// 2. Await the next completed worker.
    /// 3. Record the result and optionally expand the frontier from extracted
    ///    anchors.
    ///
    /// Cache replay and live browser capture are both hidden behind
    /// `crawl_one_request`, so warm-cache and cold-cache requests follow the
    /// same outer control flow.
    pub async fn crawl(
        &self,
        seeds: Vec<CrawlRequest<P>>,
    ) -> CrawlEngineResult<CrawlRunResult<P>> {
        let mut state = CrawlRunState::new(self.config.limits.clone(), seeds);

        let sessions = self.build_session_pool();

        let frontier_scorer = build_frontier_scorer(&self.config.frontier.scoring);
        let retain_score_evidence = self.config.frontier.scoring.retain_evidence;

        let mut inflight = FuturesUnordered::new();
        let max_concurrent_pages = self.config.concurrency.max_concurrent_pages.max(1);

        loop {
            while inflight.len() < max_concurrent_pages && state.should_continue() {
                let Some(item) = state.pop_next() else {
                    break;
                };

                let request = item.request;

                if state.has_visited_fetch_key(&request) {
                    tracing::debug!(
                        requested_url = %request.requested_url,
                        "request already visited; skipping duplicate"
                    );

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

                let cache_key = CacheKey::for_url(request.requested_url.clone());
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
                let started = std::time::Instant::now();

                state.expand_from_anchors(
                    &request,
                    &anchors,
                    &self.policy,
                    frontier_scorer.as_ref(),
                    retain_score_evidence,
                );

                tracing::debug!(
                    requested_url = %request.requested_url,
                    expansion_ms = started.elapsed().as_millis(),
                    frontier_len = state.frontier_len(),
                    inflight = inflight.len(),
                    max_concurrent_pages,
                    "completed frontier expansion"
                );
            }
        }

        sessions.shutdown_all().await;

        Ok(state.finish())
    }

    /// Build the browser session pool for one crawl invocation.
    ///
    /// Session pooling remains part of coordination because the pool lifetime is
    /// run-scoped. Individual workers receive a clone of the pool and lease page
    /// capacity from it.
    fn build_session_pool(&self) -> SessionPool {
        SessionPool::new(
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

            eprintln!(
                "{}",
                format!(
                    "{profile_key} browser session rotated: {}",
                    format!("{reason:?}")
                )
            );
        })
    }
}

fn build_frontier_scorer(
    config: &FrontierScoringConfig,
) -> Option<FrontierUrlScorer> {
    if !config.enabled {
        return None;
    }

    Some(FrontierUrlScorer::builtin(config.builtin_profile))
}

