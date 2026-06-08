//! Error types for browser automation.
//!
//! The goal is to distinguish:
//!
//! - fatal browser/session/page failures,
//! - page-level failures,
//! - extraction failures,
//! - timeout conditions,
//! - non-critical page errors that may still allow useful scraping,
//! - environment failures that should usually be retried later.
//!
//! A page can be scrapeable even if some network requests fail, JavaScript logs
//! errors, images fail to load, or third-party scripts explode in the corner.
//! Those should often be recorded as observations, not promoted into fatal crawl
//! errors.
//!
//! But CDP disconnects, browser process death, and severe network failures are
//! different beasts. Those can contaminate snapshots and should be propagated
//! upward so the crawler/app can terminate the session, avoid cache poisoning,
//! and retry later if appropriate.

use serde::{
    Deserialize,
    Serialize,
};
use thiserror::Error;

pub type BrowserDriverResult<T> = Result<T, BrowserDriverError>;

#[derive(Debug, Error)]
pub enum BrowserDriverError {
    #[error("failed to launch browser: {0}")]
    Launch(String),

    #[error("failed to connect to browser: {0}")]
    Connect(String),

    #[error("browser session closed unexpectedly: {0}")]
    SessionClosed(String),

    #[error("CDP connection lost: {0}")]
    CdpDisconnected(String),

    #[error("browser process became unhealthy: {0}")]
    BrowserUnhealthy(String),

    #[error("page operation failed: {0}")]
    Page(String),

    #[error("navigation failed: {0}")]
    Navigation(String),

    #[error("network appears unhealthy: {0}")]
    NetworkUnhealthy(String),

    #[error("operation timed out: {0}")]
    OperationTimeout(String),

    #[error("wait condition timed out: {0}")]
    WaitTimeout(String),

    #[error("JavaScript evaluation failed: {0}")]
    JavaScriptEvaluation(String),

    #[error("page extraction failed: {0}")]
    Extraction(String),

    #[error("profile error: {0}")]
    Profile(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("URL parse error: {0}")]
    Url(#[from] url::ParseError),

    #[error("internal browser-driver error: {0}")]
    Internal(String),
}

impl BrowserDriverError {
    /// Returns true when the current browser session should be considered unsafe
    /// to reuse.
    pub fn should_terminate_session(&self) -> bool {
        matches!(
            self,
            BrowserDriverError::CdpDisconnected(_)
                | BrowserDriverError::BrowserUnhealthy(_)
                | BrowserDriverError::SessionClosed(_)
        )
    }

    /// Returns true when the crawl item should probably be retried later by a
    /// higher-level scheduler.
    pub fn is_retryable_environment_failure(&self) -> bool {
        matches!(
            self,
            BrowserDriverError::NetworkUnhealthy(_)
                | BrowserDriverError::CdpDisconnected(_)
                | BrowserDriverError::BrowserUnhealthy(_)
                | BrowserDriverError::SessionClosed(_)
                | BrowserDriverError::Connect(_)
                | BrowserDriverError::Launch(_)
                | BrowserDriverError::OperationTimeout(_)
        )
    }

    /// Returns true when the error happened at the page/navigation layer.
    pub fn is_page_level_failure(&self) -> bool {
        matches!(
            self,
            BrowserDriverError::Page(_)
                | BrowserDriverError::Navigation(_)
                | BrowserDriverError::WaitTimeout(_)
                | BrowserDriverError::JavaScriptEvaluation(_)
                | BrowserDriverError::Extraction(_)
        )
    }

    /// Returns true when this error should normally prevent persisting a page
    /// snapshot as authoritative crawl evidence.
    pub fn should_reject_snapshot_by_default(&self) -> bool {
        matches!(
            self,
            BrowserDriverError::NetworkUnhealthy(_)
                | BrowserDriverError::CdpDisconnected(_)
                | BrowserDriverError::BrowserUnhealthy(_)
                | BrowserDriverError::SessionClosed(_)
                | BrowserDriverError::Connect(_)
                | BrowserDriverError::Launch(_)
                | BrowserDriverError::OperationTimeout(_)
        )
    }
}

/// Non-fatal browser/page issues observed while opening or inspecting a page.
///
/// These should usually be attached to the page result and passed upward. The
/// crawler/app layer can decide whether they matter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NonCriticalBrowserError {
    pub kind: NonCriticalBrowserErrorKind,
    pub message: String,
}

impl NonCriticalBrowserError {
    pub fn new(kind: NonCriticalBrowserErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonCriticalBrowserErrorKind {
    Console,
    Network,
    Resource,
    JavaScript,
    Security,
    Other,
}

