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
//! ## Browser profile semantics
//!
//! Browser profiles are execution state. They control things like:
//!
//! - browser HTTP cache,
//! - cookies,
//! - service workers,
//! - local storage,
//! - consent/login/session state,
//! - profile-specific browser warmup.
//!
//! Persistent browser profiles can make repeated live crawls faster or more
//! realistic, especially when the same host is consistently assigned to the same
//! profile.
//!
//! ## Important cache boundary
//!
//! Browser profile keys are **not** SQLite artifact cache identity.
//!
//! A profile key answers:
//!
//! ```text
//! Which browser state should execute this request?
//! ```
//!
//! It does not answer:
//!
//! ```text
//! Is this the same reusable page artifact?
//! ```
//!
//! The SQLite cache should use stable request identity such as requested URL,
//! namespace, and cache key version. The browser profile used to produce an
//! artifact belongs in cache metadata as provenance.
//!
//! If the observable page genuinely varies by crawl context, callers should use
//! an explicit semantic namespace or future vary dimension. Do not use raw
//! Chrome profile IDs as artifact identity.
//!
//! ## Determinism
//!
//! Profile assignment should still be deterministic. Even though profile keys do
//! not define SQLite cache identity, deterministic assignment improves browser
//! cache reuse and makes crawl behavior easier to inspect.

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
    /// Every request uses the same browser profile key.
    Single {
        key: BrowserProfileKey,
    },

    /// Use caller-provided browser profile key when present, otherwise fallback.
    CallerProvidedOrSingle {
        fallback: BrowserProfileKey,
    },

    /// Derive a browser profile key from the requested URL host.
    ///
    /// This can improve browser-cache locality for live crawls, but it should
    /// not affect SQLite artifact cache identity.
    ByHost,

    /// Derive a browser profile key from the original seed URL host.
    ///
    /// This keeps all discovered pages from a seed in the same browser profile.
    /// That may be useful for cookies, service workers, and browser cache
    /// locality. It should not affect SQLite artifact cache identity.
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
    /// Browser profile key assigned for live execution.
    ///
    /// This is execution provenance, not SQLite cache identity.
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

            BrowserProfileStrategy::CallerProvidedOrSingle { fallback } => request
                .profile_key
                .clone()
                .unwrap_or_else(|| fallback.clone()),

            BrowserProfileStrategy::ByHost => {
                let host = request
                    .requested_url
                    .host_str()
                    .unwrap_or("unknown-host");

                BrowserProfileKey::new(format!("host:{host}"))
            }

            BrowserProfileStrategy::BySeedHost => {
                let host = request
                    .seed_url
                    .host_str()
                    .unwrap_or("unknown-seed-host");

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

