//! Optional crawl artifact sink.
//!
//! This module is intentionally not a database layer.
//!
//! The crawler engine should be able to return results directly without knowing
//! whether downstream code writes JSON, SQLite, Parquet, Tantivy indexes,
//! message queues, or nothing at all.
//!
//! This trait is a lightweight hook for callers that want streaming access to
//! crawl artifacts while the engine runs.
//!
//! Cache storage is separate. Cache storage answers “can I reuse this page
//! artifact for this request?” An artifact sink answers “where should crawl
//! results be emitted?”

use async_trait::async_trait;

use crate::{
    error::CrawlEngineResult,
    output::CrawlPageResult,
};

#[async_trait]
pub trait CrawlArtifactSink<P = serde_json::Value>: Send + Sync {
    async fn record_page(
        &self,
        page: &CrawlPageResult<P>,
    ) -> CrawlEngineResult<()>;
}

#[derive(Debug, Clone, Default)]
pub struct NoopCrawlArtifactSink;

#[async_trait]
impl<P> CrawlArtifactSink<P> for NoopCrawlArtifactSink
where
    P: Send + Sync,
{
    async fn record_page(
        &self,
        _page: &CrawlPageResult<P>,
    ) -> CrawlEngineResult<()> {
        Ok(())
    }
}

