//! Crawler cache API.
//!
//! The cache is a reusable page-artifact layer. It is deliberately separate
//! from durable crawl databases and downstream indexing.
//!
//! Cache lookup is request-addressed. The key should reflect what the crawler
//! asked the browser to open and the profile/cache namespace used to open it.
//! Browser-observed facts such as final URL, redirect chain, and canonical URL
//! are stored inside the artifact, but they should not become the primary cache
//! lookup identity.
//!
//! This matters because URLs resolve strangely:
//!
//! - `http://example.com` may become `https://www.example.com/`.
//! - a domain may redirect differently by region or network conditions.
//! - a bad browser/network state may produce an artifact that should be
//!   rejected under current policy.
//!
//! Cache artifacts should be self-contained, serializable, and cheap to reject.
//! If schema, policy, or health thresholds change, the engine can simply recrawl
//! and repair the cache on the next invocation.
//!
//! The default filesystem cache stores one binary artifact per key. Future
//! implementations may use SQLite, object storage, or other lookup systems
//! without changing crawler logic.

pub mod artifact;
pub mod codec;
pub mod error;
pub mod fs;
pub mod key;
pub mod policy;

pub use artifact::{
    CacheProducerInfo,
    CacheSnapshot,
    CachedExtractedFacts,
    CachedPageArtifact,
    SnapshotCompression,
    CACHED_PAGE_ARTIFACT_VERSION,
};

pub use codec::{
    BincodeCacheCodec,
    CacheCodec,
};

pub use error::{
    CrawlCacheError,
};

pub use fs::{
    FsCrawlCacheStore,
};

pub use key::{
    CacheKey,
    CACHE_KEY_SCHEMA_VERSION,
    CACHE_KEY_VERSION,
};

pub use policy::{
    CacheDecision,
    CachePolicy,
    CacheRejectionReason,
};

use async_trait::async_trait;

#[async_trait]
pub trait CrawlCacheStore: Send + Sync {
    async fn load(
        &self,
        key: &CacheKey,
    ) -> Result<Option<CachedPageArtifact>, CrawlCacheError>;

    async fn save(
        &self,
        key: &CacheKey,
        artifact: &CachedPageArtifact,
    ) -> Result<(), CrawlCacheError>;
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{
        Digest,
        Sha256,
    };

    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

