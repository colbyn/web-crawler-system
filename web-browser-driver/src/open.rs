//! Page opening API.
//!
//! Opening a page is not just "navigate and return HTML". The browser may
//! observe redirects, final URLs, non-critical page errors, and timing facts.
//!
//! This module defines the result shape returned by `BrowserSession::open_page`.

use std::time::Duration;

use url::Url;

use crate::{
    BrowserPage,
    LoadStrategy,
    NonCriticalBrowserError,
    PageTelemetry,
    UrlResolution,
};

#[derive(Debug, Clone)]
pub struct OpenPageOptions {
    /// The URL the caller wants the browser to open.
    ///
    /// This must be preserved as request-side evidence even if the browser later
    /// resolves to a different final URL.
    pub requested_url: Url,

    /// Load/wait strategy used after navigation.
    pub load_strategy: LoadStrategy,

    /// Optional hard timeout for this open operation.
    pub timeout: Option<Duration>,

    pub navigation_timeout: Option<Duration>,
}

impl OpenPageOptions {
    pub fn new(requested_url: Url) -> Self {
        Self {
            requested_url,
            load_strategy: LoadStrategy::Default,
            timeout: Some(Duration::from_secs(10)),
            navigation_timeout: None,
        }
    }

    pub fn simple(requested_url: Url) -> Self {
        Self::new(requested_url)
    }

    pub fn with_max_timeout(mut self, delta: Duration) -> Self {
        self.timeout = Some(delta);
        self
    }
    pub fn with_navigation_timeout(mut self, delta: Duration) -> Self {
        self.navigation_timeout = Some(delta);
        self
    }
}

pub struct OpenedPage {
    /// Live browser page handle.
    ///
    /// The caller should explicitly close this after extracting what it needs.
    pub page: BrowserPage,

    /// Request/final URL facts observed while opening the page.
    pub resolution: UrlResolution,

    /// HTTP-ish status if the browser driver can observe it.
    ///
    /// Some CDP flows make status easy to capture. Some navigations may not
    /// expose a clean single status, especially with client-side navigation.
    pub status_code: Option<u16>,

    /// Errors observed during page load that do not necessarily invalidate the
    /// page for scraping.
    pub non_critical_errors: Vec<NonCriticalBrowserError>,

    /// Low-level observations recorded while opening the page.
    ///
    /// The crawler/app layer decides whether these observations imply:
    ///
    /// - normal snapshot,
    /// - questionable snapshot,
    /// - retry later,
    /// - terminate browser session.
    pub telemetry: PageTelemetry,
}

