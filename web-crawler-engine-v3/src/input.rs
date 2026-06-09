//! Crawl input model.
//!
//! This module defines the request-side types accepted by the crawler engine.
//!
//! The core idea is provenance preservation. A crawl request is not merely a
//! URL. It may represent a business entity, seed group, campaign, import row,
//! or any caller-owned record.
//!
//! The engine should carry that provenance through:
//!
//! - runtime redirects,
//! - final URL resolution,
//! - cache lookup,
//! - extracted links,
//! - page results,
//! - retry/recrawl decisions.
//!
//! The provenance type is generic so the engine can serve different callers
//! without learning their application model.

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;
use uuid::Uuid;
use web_browser_driver::BrowserProfileKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CrawlRequestId(pub Uuid);

impl CrawlRequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SeedGroupId(pub Uuid);

impl SeedGroupId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SeedGroup<P = serde_json::Value> {
    pub id: SeedGroupId,

    /// Optional caller-readable name.
    pub label: Option<String>,

    /// Caller-owned metadata shared by all seeds in this group.
    #[deprecated(note = "Use CrawlAssociation instead.")]
    pub provenance: P,

    pub seeds: Vec<CrawlSeed>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlSeed {
    /// URL as supplied by the caller.
    pub url: Url,

    /// Optional browser profile namespace requested by the caller.
    ///
    /// If absent, the scheduler may derive one from the configured profile
    /// strategy.
    pub profile_key: Option<BrowserProfileKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrawlRequest<P = serde_json::Value> {
    pub id: CrawlRequestId,

    /// Optional group that supplied this request.
    pub seed_group_id: Option<SeedGroupId>,

    /// Original seed URL associated with this crawl request.
    pub seed_url: Url,

    /// URL currently requested for visit.
    ///
    /// For seeds, this is normally the same as `seed_url`. For discovered links,
    /// this is the discovered target.
    pub requested_url: Url,

    /// URL that discovered this request, if any.
    pub discovered_from: Option<Url>,

    /// Number of link hops from the original seed.
    pub hop_depth: u32,

    /// Browser profile namespace requested or derived for this work.
    pub profile_key: Option<BrowserProfileKey>,

    /// Caller-owned provenance.
    #[deprecated(note = "Use CrawlAssociation instead.")]
    pub provenance: P,
}

impl<P> CrawlRequest<P> {
    pub fn seed(seed_url: Url, provenance: P) -> Self {
        Self {
            id: CrawlRequestId::new(),
            seed_group_id: None,
            requested_url: seed_url.clone(),
            seed_url,
            discovered_from: None,
            hop_depth: 0,
            profile_key: None,
            provenance,
        }
    }

    pub fn with_profile_key(mut self, profile_key: BrowserProfileKey) -> Self {
        self.profile_key = Some(profile_key);
        self
    }
}
