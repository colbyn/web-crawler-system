//! Crawl input model.
//!
//! This module defines the request-side types accepted by the crawler engine.
//!
//! A crawl request is not merely a URL. It may originate from a business entity,
//! seed group, campaign, import row, manual debug run, or any other caller-owned
//! context.
//!
//! The crawler core should not understand caller-specific models directly.
//! Instead, caller context is carried through stable, queryable tags.
//!
//! Tags are inherited from seeds to discovered pages so downstream systems can
//! later ask questions like:
//!
//! - which cached pages were reached for entity X?
//! - which pages were scraped for category Y?
//! - which pages belong to a manual debug run?
//!
//! The generic `P` parameter remains as a phantom lane for future typed APIs,
//! but it is not currently persisted or interpreted by this crate.

use std::marker::PhantomData;

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;
use uuid::Uuid;
use web_browser_driver::BrowserProfileKey;

use web_crawler_db::CacheTag;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CrawlRequestId(pub Uuid);

impl CrawlRequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CrawlRequestId {
    fn default() -> Self {
        Self::new()
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

impl Default for SeedGroupId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SeedGroup<P = serde_json::Value> {
    pub id: SeedGroupId,

    /// Optional caller-readable name.
    pub label: Option<String>,

    /// Caller-owned metadata shared by all seeds in this group.
    ///
    /// This remains as a deprecated compatibility field while tags become the
    /// primary durable association mechanism.
    #[deprecated(note = "Use tags / CacheTag associations instead.")]
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

    /// Caller/app tags attached to this seed.
    ///
    /// These tags are inherited by all crawl requests discovered from this seed
    /// and eventually merged onto any cache entries reached by those requests.
    pub tags: Vec<CacheTag>,
}

impl CrawlSeed {
    pub fn new(url: Url) -> Self {
        Self {
            url,
            profile_key: None,
            tags: Vec::new(),
        }
    }

    pub fn with_profile_key(mut self, profile_key: BrowserProfileKey) -> Self {
        self.profile_key = Some(profile_key);
        self
    }

    pub fn with_tag(mut self, tag: CacheTag) -> Self {
        self.tags.push(tag);
        self
    }

    pub fn with_tags<I>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = CacheTag>,
    {
        self.tags.extend(tags);
        self
    }
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

    /// Caller/app tags inherited from the seed.
    ///
    /// These are not page-derived facts. They describe why the caller cares
    /// about this crawl path, such as:
    ///
    /// - entity/business identity,
    /// - category,
    /// - import batch,
    /// - campaign,
    /// - manual/debug run.
    pub tags: Vec<CacheTag>,

    /// Reserved typed lane for future caller APIs.
    ///
    /// The crawler currently does not persist or interpret `P`.
    pub provenance: PhantomData<P>,
}

impl<P> CrawlRequest<P> {
    pub fn seed(seed_url: Url) -> Self {
        Self::seed_with_tags(seed_url, [])
    }

    pub fn seed_with_tags<I>(seed_url: Url, tags: I) -> Self
    where
        I: IntoIterator<Item = CacheTag>,
    {
        Self {
            id: CrawlRequestId::new(),
            seed_group_id: None,
            requested_url: seed_url.clone(),
            seed_url,
            discovered_from: None,
            hop_depth: 0,
            profile_key: None,
            tags: tags.into_iter().collect(),
            provenance: PhantomData,
        }
    }

    pub fn from_seed(seed: CrawlSeed) -> Self {
        Self {
            id: CrawlRequestId::new(),
            seed_group_id: None,
            requested_url: seed.url.clone(),
            seed_url: seed.url,
            discovered_from: None,
            hop_depth: 0,
            profile_key: seed.profile_key,
            tags: seed.tags,
            provenance: PhantomData,
        }
    }

    pub fn discovered_from(parent: &Self, requested_url: Url) -> Self {
        Self {
            id: CrawlRequestId::new(),
            seed_group_id: parent.seed_group_id,
            seed_url: parent.seed_url.clone(),
            requested_url,
            discovered_from: Some(parent.requested_url.clone()),
            hop_depth: parent.hop_depth + 1,
            profile_key: parent.profile_key.clone(),
            tags: parent.tags.clone(),
            provenance: PhantomData,
        }
    }

    pub fn with_seed_group_id(mut self, seed_group_id: SeedGroupId) -> Self {
        self.seed_group_id = Some(seed_group_id);
        self
    }

    pub fn with_profile_key(mut self, profile_key: BrowserProfileKey) -> Self {
        self.profile_key = Some(profile_key);
        self
    }

    pub fn with_tag(mut self, tag: CacheTag) -> Self {
        self.tags.push(tag);
        self
    }

    pub fn with_tags<I>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = CacheTag>,
    {
        self.tags.extend(tags);
        self
    }
}
