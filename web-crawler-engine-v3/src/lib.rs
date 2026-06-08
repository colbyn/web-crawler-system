//! Provenance-preserving web crawler engine.
//!
//! This crate owns crawler orchestration. It is intentionally above
//! `web-browser-driver` and below app-specific business logic.
//!
//! The browser driver answers:
//!
//! ```text
//! Given this URL and browser profile, what did the browser observe?
//! ```
//!
//! This crate answers:
//!
//! ```text
//! Given one or more crawl requests, how should URLs be scheduled, opened,
//! cached, evaluated, expanded, and returned while preserving provenance?
//! ```
//!
//! This crate owns:
//!
//! - crawl requests and seed groups,
//! - provenance carried through redirects and discovered links,
//! - frontier scheduling,
//! - URL scope policy,
//! - cache lookup and snapshot artifact evaluation,
//! - browser profile assignment,
//! - page result assembly,
//! - retry/recrawl decisions at the crawler layer.
//!
//! This crate does **not** own:
//!
//! - Chromium/CDP details,
//! - browser process launch mechanics,
//! - app-specific entity models,
//! - SQLite/database persistence,
//! - final downstream indexing,
//! - business-specific interpretation of extracted page content.
//!
//! The engine is designed around a key invariant:
//!
//! Runtime URL resolution must not erase upstream provenance.
//!
//! A caller may request `http://example.com`, the browser may resolve to
//! `https://www.example.com/`, and several independent business entities may
//! have pointed at the same resolved document. The engine must preserve those
//! relationships instead of flattening them into a single final URL too early.
//!
//! The cache layer is also intentionally policy-aware but storage-agnostic.
//! Cached artifacts should be self-contained and serializable. If cache schema,
//! policy, health thresholds, or addressing semantics change, stale artifacts
//! can simply be rejected and regenerated on a later crawl.
//! 
//! 
//! Provenance-preserving web crawler engine.
//!
//! See the module-level docs in the original design for the full philosophy.

pub mod cache;
pub mod config;
pub mod engine;
pub mod error;
pub mod frontier;
pub mod input;
pub mod output;
pub mod policy;
pub mod scheduler;
pub mod sessions;
pub mod state;
pub mod store;
pub mod url;

pub use cache::{
    CacheDecision, CacheKey, CachePolicy, CacheProducerInfo, CacheRejectionReason, CacheSnapshot,
    CachedExtractedFacts, CachedPageArtifact, CrawlCacheError, CrawlCacheStore, FsCrawlCacheStore,
    SnapshotCompression,
};

pub use config::{CrawlEngineConfig, CrawlLimits};

pub use engine::CrawlEngine;

pub use error::{CrawlEngineError, CrawlEngineResult};

pub use frontier::{FrontierItem, FrontierItemId, FrontierQueue, FrontierScore};

pub use input::{CrawlRequest, CrawlRequestId, CrawlSeed, SeedGroup, SeedGroupId};

pub use output::{CrawlPageOutcome, CrawlPageResult, CrawlRunResult, SnapshotDecision};

pub use policy::{
    CrawlPolicy, ScopeDecision, ScopePolicy, SnapshotPolicy, VisitDecision, VisitPolicy,
};

pub use scheduler::{BrowserProfileAssignment, BrowserProfileStrategy, SessionScheduler};

pub use sessions::SessionPool;

pub use state::CrawlRunState;

pub use store::{CrawlArtifactSink, NoopCrawlArtifactSink};

pub use url::{NormalizedUrl, UrlIdentity, UrlNormalizer};

