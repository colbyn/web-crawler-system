//! URL resolution facts.
//!
//! Runtime URL resolution is a first-class browser observation.
//!
//! A crawler/app may associate the original requested URL with a business
//! entity, seed, campaign, or other provenance record. Browser redirects must
//! not erase that association.
//!
//! Therefore this crate reports:
//!
//! - requested URL,
//! - final URL,
//! - redirect chain when available,
//! - optional page-declared canonical URL.
//!
//! The crawler/app can then decide how to associate the final document with its
//! original seeds.
//!
//! Resolution facts are persistable crawl evidence. They should serialize
//! without requiring downstream crates to invent mirror types.

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;
use schemars::JsonSchema;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct UrlResolution {
    /// URL the caller requested.
    pub requested_url: Url,

    /// URL the browser ended on after navigation.
    pub final_url: Url,

    /// Redirect hops observed during navigation.
    pub redirect_chain: Vec<RedirectHop>,

    /// Page-declared canonical URL, if extracted.
    ///
    /// This is not the same thing as `final_url`. It is a claim made by the
    /// page, not necessarily a transport-level resolution.
    pub canonical_url: Option<Url>,
}

impl UrlResolution {
    pub fn from_requested_and_final(requested_url: Url, final_url: Url) -> Self {
        Self {
            requested_url,
            final_url,
            redirect_chain: Vec::new(),
            canonical_url: None,
        }
    }

    pub fn no_redirect(requested_url: Url) -> Self {
        Self {
            final_url: requested_url.clone(),
            requested_url,
            redirect_chain: Vec::new(),
            canonical_url: None,
        }
    }

    pub fn was_redirected(&self) -> bool {
        self.requested_url != self.final_url || !self.redirect_chain.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct RedirectHop {
    pub from_url: Url,
    pub to_url: Url,

    /// HTTP status code when known.
    ///
    /// Browser-level navigation may not always expose this cleanly for every
    /// hop. Keep it optional.
    pub status_code: Option<u16>,
}

