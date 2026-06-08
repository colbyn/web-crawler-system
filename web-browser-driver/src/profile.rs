//! Browser profile management.
//!
//! Browser caching happens through browser profile directories. Cookies,
//! HTTP cache, local storage, service worker state, and other browser-owned
//! storage are profile-scoped.
//!
//! This crate exposes profiles as explicit resources because the crawler may
//! want deterministic profile assignment:
//!
//! - one persistent profile per seed group,
//! - one persistent profile per domain,
//! - a small number of hash-bucketed profiles,
//! - temporary isolated profiles for one-off visits.
//!
//! The browser driver should not decide *why* a profile is chosen. It only knows
//! how to materialize and clean up the profile once the caller has selected one.
//!
//! Profile keys are part of crawler/cache identity and therefore must be
//! serializable. A cache artifact produced with one profile namespace should not
//! silently masquerade as an artifact from another namespace.

use std::path::PathBuf;

use serde::{
    Deserialize,
    Serialize,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BrowserProfileKey(pub String);

impl BrowserProfileKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BrowserProfile {
    pub key: BrowserProfileKey,
    pub kind: BrowserProfileKind,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserProfileKind {
    /// Caller-managed temporary profile directory.
    ///
    /// The driver can use this profile, but deletion policy belongs to the code
    /// that created the directory unless this crate later grows an owned temp
    /// profile guard.
    Temporary,

    /// Reused across browser sessions.
    ///
    /// This is the mode that makes browser-level caching valuable.
    Persistent,
}

impl BrowserProfile {
    pub fn temporary(key: BrowserProfileKey, path: PathBuf) -> Self {
        Self {
            key,
            kind: BrowserProfileKind::Temporary,
            path,
        }
    }

    pub fn persistent(key: BrowserProfileKey, path: PathBuf) -> Self {
        Self {
            key,
            kind: BrowserProfileKind::Persistent,
            path,
        }
    }
}
