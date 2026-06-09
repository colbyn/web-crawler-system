//! Browser driver entry point.
//!
//! A driver may launch many sessions over time. A session corresponds roughly
//! to one browser process bound to one profile directory.
//!
//! `BrowserDriver` is responsible for creating browser sessions using explicit
//! profiles. It should hide the details of Chromiumoxide/CDP/WebSocket plumbing
//! from the crawler engine.
//!
//! `BrowserDriver` is the factory for `BrowserSession`s. It owns the shared
//! launch configuration (`executable_path`, headless mode, extra args,
//! startup timeout) but **does not** decide profile policy.
//!
//! Profile selection (one persistent profile per seed group / domain, temp
//! profiles, hash-bucketed profiles, etc.) is the responsibility of the
//! caller, usually the crawler engine.
//!
//! This module performs the actual `chromiumoxide` launch and wires up the
//! CDP event handler task.
//!
//! The handler task is not incidental plumbing. It is the browser event pump.
//! If it errors or exits while the session is still expected to be live, the
//! session must be treated as unsafe to reuse.

use futures::StreamExt;
use tokio::time::timeout;

use crate::{
    BrowserDriverConfig, BrowserDriverError, BrowserDriverResult, BrowserProfile,
    BrowserSession, BrowserSessionHealth, HeadlessMode,
};

#[derive(Clone)]
pub struct BrowserDriver {
    config: BrowserDriverConfig,
}

impl BrowserDriver {
    pub fn new(config: BrowserDriverConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &BrowserDriverConfig {
        &self.config
    }

    pub async fn start_session(
        &self,
        profile: BrowserProfile,
    ) -> BrowserDriverResult<BrowserSession> {
        use chromiumoxide::browser::{Browser, BrowserConfig};

        let mut builder = BrowserConfig::builder();

        if let Some(exec) = &self.config.executable_path {
            builder = builder.chrome_executable(exec);
        }

        match self.config.launch.headless {
            HeadlessMode::False => {
                builder = builder.with_head();
            }
            HeadlessMode::New => {
                builder = builder.new_headless_mode();
            }
            HeadlessMode::True => {}
        }

        for arg in &self.config.launch.args {
            builder = builder.arg(arg.as_str());
        }

        builder = builder.user_data_dir(&profile.path);

        let browser_config = builder
            .build()
            .map_err(|e| BrowserDriverError::Launch(format!("config error: {e}")))?;

        let launch_fut = Browser::launch(browser_config);

        let (browser, mut handler) =
            match timeout(self.config.launch.startup_timeout, launch_fut).await {
                Ok(Ok(pair)) => pair,
                Ok(Err(e)) => {
                    return Err(BrowserDriverError::Launch(e.to_string()));
                }
                Err(_) => {
                    return Err(BrowserDriverError::Launch(format!(
                        "launch timed out after {:?}",
                        self.config.launch.startup_timeout
                    )));
                }
            };

        let health = BrowserSessionHealth::default();
        let health_for_handler = health.clone();

        let handler_handle = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    health_for_handler.observe_cdp_error();

                    tracing::warn!("CDP handler error: {:?}", e);

                    // Until we have more precise Chromiumoxide error
                    // classification, any handler error is treated as session
                    // contamination. This is intentionally conservative: a bad
                    // CDP transport can poison page snapshots.
                    health_for_handler.observe_cdp_disconnected();

                    break;
                }
            }

            // If the handler exits, the session can no longer reliably observe
            // browser events. For a live session, that is unsafe.
            health_for_handler.observe_handler_finished();
        });

        Ok(BrowserSession::with_browser(
            profile,
            browser,
            handler_handle,
            health,
        ))
    }
}

