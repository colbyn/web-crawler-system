//! Crawl engine facade.
//!
//! This module owns the public `CrawlEngine` type and the small amount of
//! construction API needed to wire the crawler together.
//!
//! The old single-file engine mixed several runtime phases in one place:
//!
//! - crawl coordination,
//! - cache replay,
//! - live browser work,
//! - artifact construction,
//! - persistence,
//! - sink recording,
//! - frontier expansion.
//!
//! This directory module keeps the public facade here and moves phase-specific
//! behavior into sibling modules. The goal is one uniform crawl pipeline, not
//! separate special paths for seed-only crawls, warm-cache crawls, or future
//! depth crawls.
//!
//! ## Uniform pipeline
//!
//! Every crawl request should flow through the same conceptual path:
//!
//! ```text
//! request
//!   -> cache replay
//!   -> live browser capture if cache misses
//!   -> extraction
//!   -> artifact persistence
//!   -> sink/result recording
//!   -> optional frontier expansion
//! ```
//!
//! `max_hop_depth = 0` should only prevent discovered anchors from becoming
//! future requests. It must not prevent anchor extraction, cache replay of
//! anchors, artifact persistence, or downstream landing-page analysis.
//!
//! ## Cache boundary
//!
//! Durable artifact caching is provided by `web-crawler-db` through
//! `PostgresCache`.
//!
//! The engine receives an already-constructed cache handle. It does not create,
//! migrate, or administer database schema. Schema setup belongs to explicit
//! application/admin paths such as a CLI command or setup script.
//!
//! Tags remain caller/application associations. They are not cache identity.
//! Browser profile keys remain execution provenance. They are not cache
//! identity either.
//!
//! ## Module map
//!
//! - `coordinator`: owns crawl state, inflight work, sink recording, and
//!   frontier expansion.
//! - `worker`: executes one request through cache-or-live resolution.
//! - `cache_replay`: reconstructs crawl results from cached metadata.
//! - `live_page`: opens browser pages, extracts evidence, captures snapshots,
//!   and builds live crawl results.
//!
//! Later throughput work can add a bounded writer lane without changing the
//! public facade or creating a second crawler species.

use std::{
    marker::PhantomData,
    path::PathBuf,
};

use web_browser_driver::BrowserDriver;
use web_crawler_db::PostgresCache;

use crate::{
    config::CrawlEngineConfig,
    policy::CrawlPolicy,
    scheduler::BrowserProfileStrategy,
    store::{
        CrawlArtifactSink,
        NoopCrawlArtifactSink,
    },
};

mod cache_replay;
mod coordinator;
mod live_page;
mod worker;

/// Main crawler orchestration type.
///
/// `CrawlEngine` is intentionally a facade. The mutable crawl brain lives in
/// the coordinator module, cache replay lives in the cache replay module, and
/// browser work lives in the live page module.
///
/// `P` is retained as a typed lane for caller-specific APIs and result shapes.
/// The engine does not persist or interpret arbitrary `P`; durable caller
/// association should flow through request tags.
pub struct CrawlEngine<P, S = NoopCrawlArtifactSink> {
    /// Engine limits, concurrency knobs, timeouts, and runtime feature flags.
    pub config: CrawlEngineConfig,

    /// Crawl policy for visit decisions, scope decisions, snapshots, and cache
    /// acceptance.
    pub policy: CrawlPolicy,

    /// Browser driver used to launch or connect to browser sessions.
    pub browser_driver: BrowserDriver,

    /// Optional Postgres-backed reusable artifact cache.
    ///
    /// The cache handle is supplied by the caller. The engine does not migrate
    /// database schema or own database administration.
    pub cache: Option<PostgresCache>,

    /// Sink used to record page results as they are produced.
    ///
    /// The sink is the preferred durable output path for large crawls. The
    /// in-memory run result should remain useful for small/debug crawls, but
    /// should not be treated as the only output mechanism.
    pub sink: S,

    /// Root directory for materialized browser profiles.
    ///
    /// Browser profiles are execution state. They help with browser cache,
    /// cookies, local storage, consent state, and similar runtime behavior. They
    /// are not artifact cache identity.
    pub profile_root: PathBuf,

    /// Strategy for assigning crawl requests to browser profile keys.
    pub profile_strategy: BrowserProfileStrategy,

    _provenance: PhantomData<P>,
}

impl<P> CrawlEngine<P> {
    /// Construct a crawler with the default no-op artifact sink.
    ///
    /// Passing `None` for `cache` disables reusable artifact caching without
    /// changing the rest of the crawl pipeline. Requests still flow through the
    /// same worker path; they simply miss the cache layer and proceed to live
    /// browser capture.
    pub fn new(
        config: CrawlEngineConfig,
        policy: CrawlPolicy,
        browser_driver: BrowserDriver,
        cache: Option<PostgresCache>,
        profile_root: PathBuf,
        profile_strategy: BrowserProfileStrategy,
    ) -> Self {
        Self {
            config,
            policy,
            browser_driver,
            cache,
            sink: NoopCrawlArtifactSink,
            profile_root,
            profile_strategy,
            _provenance: PhantomData,
        }
    }
}

impl<P, S> CrawlEngine<P, S> {
    /// Attach or replace the reusable artifact cache.
    ///
    /// This is intentionally named generically. The engine should not expose old
    /// SQLite-specific vocabulary after the Postgres cache migration.
    pub fn with_cache(mut self, cache: PostgresCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Remove the reusable artifact cache.
    ///
    /// Disabling the cache does not create a separate crawl mode. It only makes
    /// the cache replay phase return a miss for every request.
    pub fn without_cache(mut self) -> Self {
        self.cache = None;
        self
    }

    /// Attach a different artifact sink while preserving all other engine
    /// configuration.
    pub fn with_sink<S2>(self, sink: S2) -> CrawlEngine<P, S2>
    where
        S2: CrawlArtifactSink<P>,
    {
        CrawlEngine {
            config: self.config,
            policy: self.policy,
            browser_driver: self.browser_driver,
            cache: self.cache,
            sink,
            profile_root: self.profile_root,
            profile_strategy: self.profile_strategy,
            _provenance: PhantomData,
        }
    }
}
