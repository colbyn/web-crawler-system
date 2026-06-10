//! Browser session.
//!
//! A `BrowserSession` is a running browser process/client associated with one
//! explicit browser profile.
//!
//! The crawler may choose to restart sessions after N pages or after some time
//! period to control memory leaks or Chrome process drift. That lifecycle policy
//! belongs above this crate. This crate only exposes shutdown mechanics.
//!
//! A `BrowserSession` represents one running Chromium process bound to a
//! specific `BrowserProfile` user data directory. It is the unit of isolation
//! for cookies, cache, localStorage, service workers, and other browser-owned
//! storage.
//!
//! The crawler engine is responsible for deciding when to create a new session,
//! how long to keep it alive, and which profile to assign to which seed group,
//! domain, or profile bucket.
//!
//! This module also enforces session health. If the background CDP handler
//! reports a disconnect, protocol failure, or exits unexpectedly, the session is
//! treated as unsafe and future page operations fail fast.

use std::time::Duration;

use tokio::task::JoinHandle;

use crate::{
    BrowserDriverError,
    BrowserDriverResult,
    BrowserPage,
    BrowserProfile,
    BrowserSessionHealth,
    OpenPageOptions,
    OpenedPage,
    PageTelemetryBuilder,
    UrlResolution, WaitOptions,
};

pub struct BrowserSession {
    profile: BrowserProfile,
    browser: Option<chromiumoxide::Browser>,
    handler_handle: Option<JoinHandle<()>>,
    health: BrowserSessionHealth,
}

impl BrowserSession {
    pub(crate) fn with_browser(
        profile: BrowserProfile,
        browser: chromiumoxide::Browser,
        handler_handle: JoinHandle<()>,
        health: BrowserSessionHealth,
    ) -> Self {
        Self {
            profile,
            browser: Some(browser),
            handler_handle: Some(handler_handle),
            health,
        }
    }

    pub fn profile(&self) -> &BrowserProfile {
        &self.profile
    }

    pub fn health(&self) -> &BrowserSessionHealth {
        &self.health
    }

    pub async fn open_page(
        &self,
        options: OpenPageOptions,
    ) -> BrowserDriverResult<OpenedPage> {
        if let Some(timeout_duration) = options.timeout {
            tokio::time::timeout(timeout_duration, self.open_page_impl(options))
                .await
                .map_err(|_| {
                    BrowserDriverError::OperationTimeout(format!(
                        "open_page timed out after {timeout_duration:?}"
                    ))
                })?
        } else {
            self.open_page_impl(options).await
        }
    }

    async fn open_page_impl(
        &self,
        options: OpenPageOptions,
    ) -> BrowserDriverResult<OpenedPage> {
        self.health.check_usable()?;

        let browser = self.browser.as_ref().ok_or_else(|| {
            BrowserDriverError::SessionClosed(
                "BrowserSession has no active browser".into(),
            )
        })?;

        let mut telemetry = PageTelemetryBuilder::started_now();
        telemetry.mark_navigation_started();

        let chromium_page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| {
                self.health.prefer_health_error(
                    BrowserDriverError::Navigation(format!("new_page failed: {e}")),
                )
            })?;

        self.health.check_usable()?;

        chromium_page
            .goto(options.requested_url.as_str())
            .await
            .map_err(|e| {
                telemetry.mark_navigation_command_failed(e.to_string());

                self.health.prefer_health_error(
                    BrowserDriverError::Navigation(e.to_string()),
                )
            })?;

        telemetry.mark_navigation_command_succeeded();

        self.health.check_usable()?;

        let page = BrowserPage::from_chromiumoxide(chromium_page);

        let wait_options = WaitOptions {
            timeout: Duration::from_secs(3),
            interval: Duration::from_millis(500),
        };

        options
            .load_strategy
            .wait(&page, &mut telemetry, wait_options)
            .await
            .map_err(|e| self.health.prefer_health_error(e))?;

        self.health.check_usable()?;

        let final_url_str = page
            .current_url()
            .await
            .map_err(|e| self.health.prefer_health_error(e))?;

        let final_url = url::Url::parse(&final_url_str).unwrap_or_else(|_| {
            tracing::warn!(
                requested_url = %options.requested_url,
                final_url = %final_url_str,
                "browser returned an unparseable final URL; falling back to requested URL"
            );

            options.requested_url.clone()
        });

        let resolution =
            UrlResolution::from_requested_and_final(options.requested_url.clone(), final_url);

        telemetry.mark_navigation_elapsed_now();

        self.health.check_usable()?;

        Ok(OpenedPage {
            page,
            resolution,
            status_code: None,
            non_critical_errors: vec![],
            telemetry: telemetry.into_telemetry(),
        })
    }

    pub async fn new_page(&self) -> BrowserDriverResult<BrowserPage> {
        self.health.check_usable()?;

        let browser = self.browser.as_ref().ok_or_else(|| {
            BrowserDriverError::SessionClosed(
                "BrowserSession has no active browser".into(),
            )
        })?;

        let chromium_page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| {
                self.health
                    .prefer_health_error(BrowserDriverError::Page(e.to_string()))
            })?;

        self.health.check_usable()?;

        Ok(BrowserPage::from_chromiumoxide(chromium_page))
    }

    pub async fn shutdown(mut self) -> BrowserDriverResult<()> {
        if let Some(mut browser) = self.browser.take() {
            let _ = browser.close().await;
        }

        if let Some(handle) = self.handler_handle.take() {
            handle.abort();
        }

        Ok(())
    }
}
