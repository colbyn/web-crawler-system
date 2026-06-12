//! Cache replay path.
//!
//! This module reconstructs crawl page results from reusable cache metadata.
//!
//! Cache replay is part of the normal request pipeline. It is not a separate
//! crawler mode. A warm-cache request should produce the same kind of page
//! evidence as a live browser request:
//!
//! - URL resolution,
//! - response status,
//! - telemetry,
//! - non-critical browser errors,
//! - page info,
//! - extracted anchors,
//! - snapshot descriptor.
//!
//! ## Metadata-only hot path
//!
//! Replay intentionally reads only `CacheEntryMetadata` from Postgres.
//!
//! Full cache entries include payload bodies such as rendered HTML. Those bytes
//! are useful for diagnostics, export, and downstream artifact inspection, but
//! they are unnecessary for ordinary crawl replay. The replay path needs only
//! extracted JSON and payload descriptors.
//!
//! ## Tag association invariant
//!
//! Cache hits still merge request tags onto the cache entry.
//!
//! This is crucial for bulk crawls. A later seed, entity, category, dataset, or
//! manual run may reach an already-cached artifact. Even though no live browser
//! work occurs, the artifact must gain the new caller association.
//!
//! ## Anchors are evidence
//!
//! Anchors replayed from cache remain page evidence even when the active crawl
//! has `max_hop_depth = 0`. Hop depth controls whether anchors become future
//! requests. It must not control whether anchors are replayed, returned, or sent
//! to sinks.

use serde::{
    de::DeserializeOwned,
    Serialize,
};
use web_crawler_db::{
    CacheEntryMetadata,
    CacheKey,
};

use crate::{
    error::CrawlEngineResult,
    input::CrawlRequest,
    output::{
        CrawlPageOutcome,
        CrawlPageResult,
        SnapshotDecision,
    },
    policy::CacheDecision,
    store::CrawlArtifactSink,
};

use super::CrawlEngine;

impl<P, S> CrawlEngine<P, S>
where
    P: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    S: CrawlArtifactSink<P> + Send + Sync,
{
    /// Attempt to serve a request from reusable cache metadata.
    ///
    /// Returns `Some(result)` only when a usable cached artifact exists and can
    /// be decoded into a normal opened-page result.
    ///
    /// This method treats malformed or stale cache metadata as a cache miss.
    /// Ordinary cache weirdness should not fail the whole crawl. If metadata is
    /// missing replay-critical fields, the caller proceeds to live browser
    /// capture instead.
    pub(crate) async fn try_cache(
        &self,
        request: &CrawlRequest<P>,
        cache_key: &CacheKey,
    ) -> CrawlEngineResult<Option<CrawlPageResult<P>>> {
        if !self.config.cache_enabled {
            return Ok(None);
        }

        let Some(cache) = &self.cache else {
            return Ok(None);
        };

        let Some(metadata) = cache.get_metadata(cache_key).await? else {
            tracing::debug!(
                requested_url = %request.requested_url,
                "cache miss"
            );

            return Ok(None);
        };

        let decision = self.policy.evaluate_cache_metadata(&metadata);

        if !matches!(&decision, CacheDecision::Use) {
            tracing::debug!(
                requested_url = %request.requested_url,
                decision = ?decision,
                "cache metadata rejected"
            );

            return Ok(None);
        }

        // Cache-hit association write.
        //
        // A warm-cache crawl must still preserve new request ownership. This
        // operation is idempotent in `web-crawler-db`.
        if !request.tags.is_empty() {
            cache.add_tags(cache_key, &request.tags).await?;
        }

        let Some(outcome) = replay_opened_outcome(&metadata) else {
            tracing::debug!(
                requested_url = %request.requested_url,
                "cache metadata missing replay-critical fields"
            );

            return Ok(None);
        };

        tracing::debug!(
            requested_url = %request.requested_url,
            "cache hit replayed from metadata"
        );

        Ok(Some(CrawlPageResult {
            request: request.clone(),
            cache_key: Some(cache_key.clone()),
            cache_decision: Some(decision),
            outcome,
        }))
    }
}

/// Reconstruct an opened-page outcome from cache metadata.
///
/// This is intentionally forgiving. Missing optional page info or missing
/// anchors are acceptable. Missing resolution or telemetry means the entry
/// cannot faithfully replay an opened page, so the caller should treat it as a
/// miss and fall back to live capture.
///
/// The simplified cache metadata no longer carries a payload descriptor. Replay
/// therefore does not report captured snapshot byte/hash details from metadata
/// alone.
fn replay_opened_outcome(metadata: &CacheEntryMetadata) -> Option<CrawlPageOutcome> {
    let resolution = metadata.resolution.clone()?;
    let telemetry = metadata.telemetry.clone()?;

    Some(CrawlPageOutcome::Opened {
        resolution,
        status_code: metadata.response.status_code,
        telemetry,
        non_critical_errors: metadata.non_critical_errors.clone(),
        page_info: metadata.page_info.clone(),
        anchors: metadata.anchors.clone(),
        snapshot: SnapshotDecision::NotRequested,
    })
}


// fn replay_opened_outcome(metadata: &CacheEntryMetadata) -> Option<CrawlPageOutcome> {
//     let primary = metadata.primary_payload()?;

//     let resolution = serde_json::from_value(
//         metadata
//             .extracted_json
//             .get("resolution")
//             .cloned()
//             .unwrap_or(serde_json::Value::Null),
//     )
//     .ok()?;

//     let page_info = serde_json::from_value(
//         metadata
//             .extracted_json
//             .get("page_info")
//             .cloned()
//             .unwrap_or(serde_json::Value::Null),
//     )
//     .ok();

//     let anchors = serde_json::from_value(
//         metadata
//             .extracted_json
//             .get("anchors")
//             .cloned()
//             .unwrap_or_else(|| serde_json::json!([])),
//     )
//     .unwrap_or_default();

//     let telemetry = serde_json::from_value(metadata.telemetry_json.clone()).ok()?;

//     let non_critical_errors = metadata
//         .non_critical_errors_json
//         .iter()
//         .filter_map(|value| serde_json::from_value(value.clone()).ok())
//         .collect();

//     let snapshot = if primary.byte_len > 0 {
//         SnapshotDecision::Captured {
//             html_bytes: primary.byte_len,
//             body_sha256_hex: primary.sha256_hex.clone(),
//         }
//     } else {
//         SnapshotDecision::NotRequested
//     };

//     Some(CrawlPageOutcome::Opened {
//         resolution,
//         status_code: metadata.response.status_code,
//         telemetry,
//         non_critical_errors,
//         page_info,
//         anchors,
//         snapshot,
//     })
// }

