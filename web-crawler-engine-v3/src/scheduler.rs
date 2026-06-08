//! Browser session and profile scheduling.
//!
//! This module decides which browser profile key should be used for a crawl
//! request and, later, which browser session should perform the work.
//!
//! It should not launch Chromium directly. Browser launch mechanics belong to
//! `web-browser-driver`.
//!
//! It should not decide business-specific grouping. The caller may provide
//! profile keys directly, or the scheduler may derive them from generic URL or
//! seed-group facts.
//!
//! The key design pressure is cache/profile consistency. Browser cache,
//! cookies, service workers, and local storage are profile-scoped. If the engine
//! wants persistent browser caching to help future recrawls, profile assignment
//! must be deterministic.

use serde::{
    Deserialize,
    Serialize,
};
use web_browser_driver::{
    BrowserProfile,
    BrowserProfileKey,
};

use crate::input::CrawlRequest;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserProfileStrategy {
    /// Every request uses the same profile key.
    Single {
        key: BrowserProfileKey,
    },

    /// Use caller-provided profile key when present, otherwise fallback.
    CallerProvidedOrSingle {
        fallback: BrowserProfileKey,
    },

    /// Derive a profile key from the requested URL host.
    ByHost,

    /// Derive a profile key from the seed URL host.
    BySeedHost,
}

impl Default for BrowserProfileStrategy {
    fn default() -> Self {
        Self::Single {
            key: BrowserProfileKey::new("default"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BrowserProfileAssignment {
    pub key: BrowserProfileKey,
}

#[derive(Debug, Clone)]
pub struct SessionScheduler {
    strategy: BrowserProfileStrategy,
}

impl SessionScheduler {
    pub fn new(strategy: BrowserProfileStrategy) -> Self {
        Self { strategy }
    }

    pub fn assign_profile<P>(
        &self,
        request: &CrawlRequest<P>,
    ) -> BrowserProfileAssignment {
        let key = match &self.strategy {
            BrowserProfileStrategy::Single { key } => key.clone(),

            BrowserProfileStrategy::CallerProvidedOrSingle { fallback } => {
                request
                    .profile_key
                    .clone()
                    .unwrap_or_else(|| fallback.clone())
            }

            BrowserProfileStrategy::ByHost => {
                let host = request
                    .requested_url
                    .host_str()
                    .unwrap_or("unknown-host");

                BrowserProfileKey::new(format!("host:{host}"))
            }

            BrowserProfileStrategy::BySeedHost => {
                let host = request.seed_url.host_str().unwrap_or("unknown-seed-host");

                BrowserProfileKey::new(format!("seed-host:{host}"))
            }
        };

        BrowserProfileAssignment { key }
    }

    pub fn materialize_profile(
        &self,
        assignment: &BrowserProfileAssignment,
        profile_root: &std::path::Path,
    ) -> BrowserProfile {
        let path = profile_root.join(sanitize_profile_key(assignment.key.as_str()));

        BrowserProfile::persistent(assignment.key.clone(), path)
    }
}

fn sanitize_profile_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

