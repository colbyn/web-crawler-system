//! Provenance-preserving web crawler engine.
//!
//! This crate owns crawler orchestration. It sits above `web-browser-driver`
//! and below app-specific business logic.
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
//! - tags used for durable caller/app associations,
//! - provenance carried through redirects and discovered links,
//! - frontier scheduling,
//! - URL scope policy,
//! - cache lookup and snapshot artifact evaluation,
//! - reusable crawl artifact persistence via `sqlite_cache`,
//! - browser profile assignment,
//! - page result assembly,
//! - retry/recrawl decisions at the crawler layer.
//!
//! This crate does **not** own:
//!
//! - Chromium/CDP implementation details,
//! - browser process launch mechanics,
//! - app-specific entity models,
//! - CRM/business databases,
//! - final downstream indexing,
//! - business-specific interpretation of extracted page content.
//!
//! The engine is designed around a key invariant:
//!
//! ```text
//! Runtime URL resolution must not erase upstream provenance.
//! ```
//!
//! A caller may request `http://example.com`, the browser may resolve to
//! `https://www.example.com/`, and several independent business entities may
//! have pointed at the same resolved document. The engine must preserve those
//! relationships instead of flattening them into a single final URL too early.
//!
//! ## Cache identity versus caller association
//!
//! SQLite cache artifacts are reusable crawl artifacts. They are not ownership
//! records.
//!
//! Cache identity should be based on stable request identity, such as requested
//! URL, namespace, and cache key version. Browser profile IDs are execution
//! provenance and must not fragment the shared artifact cache.
//!
//! Caller/application relationships belong in tags. Multiple seeds, entities,
//! categories, campaigns, or manual runs may all point at the same cached page.
//! Cache writes and cache hits should merge request tags onto the artifact so
//! warm-cache runs still preserve new associations.
//!
//! ## Cache evolution
//!
//! Cached artifacts should be self-contained and serializable. If cache schema,
//! policy, health thresholds, or addressing semantics change, stale artifacts
//! can be rejected and regenerated on a later crawl.

// pub mod cache; // DISABLED — TODO: REMOVE DELETE FOLDER
pub mod config;
pub mod engine;
pub mod error;
pub mod frontier;
pub mod input;
pub mod output;
pub mod policy;
pub mod scheduler;
pub mod sessions;
// pub mod sqlite_cache;
pub mod state;
pub mod store;
pub mod url;

// pub use cache::{
//     CacheDecision, CacheKey, CachePolicy, CacheProducerInfo, CacheRejectionReason, CacheSnapshot,
//     CachedExtractedFacts, CachedPageArtifact, CrawlCacheError, CrawlCacheStore, FsCrawlCacheStore,
//     SnapshotCompression,
// };

pub use config::{
    CrawlConcurrency,
    CrawlEngineConfig,
    CrawlLimits,
};

pub use engine::CrawlEngine;

pub use error::{
    CrawlEngineError,
    CrawlEngineResult,
};

pub use frontier::{
    FrontierItem,
    FrontierItemId,
    FrontierQueue,
    FrontierScore,
};

pub use input::{
    CrawlRequest,
    CrawlRequestId,
    CrawlSeed,
    SeedGroup,
    SeedGroupId,
};

pub use output::{
    CrawlPageOutcome,
    CrawlPageResult,
    CrawlRunResult,
    SnapshotDecision,
};

pub use policy::{
    CacheDecision,
    CachePolicy,
    CrawlPolicy,
    ScopeDecision,
    ScopePolicy,
    SnapshotPolicy,
    VisitDecision,
    VisitPolicy,
};

pub use scheduler::{
    BrowserProfileAssignment,
    BrowserProfileStrategy,
    SessionScheduler,
};

pub use sessions::SessionPool;

// pub use sqlite_cache::SqliteCache;

pub use state::CrawlRunState;

pub use store::{
    CrawlArtifactSink,
    NoopCrawlArtifactSink,
};

pub use url::{
    NormalizedUrl,
    UrlIdentity,
    UrlNormalizer,
};
