//! Page wait conditions.
//!
//! Waiting is inherently heuristic. There is no universal browser state called
//! "the page is fully useful for scraping".
//!
//! This module provides composable conditions and practical presets.
//!
//! Crawler defaults should be conservative enough to capture useful DOM content
//! without pretending that every SPA, analytics request, background fetch,
//! service worker, animation, and ad script can be perfectly settled.
//! 
//! Wait conditions and polling infrastructure.
//!
//! Waiting is inherently heuristic. There is no single browser state that means
//! "this page is fully ready for scraping".
//!
//! This module provides composable primitives and practical presets.

pub mod condition;
pub mod presets;

use std::time::{Duration, Instant};

pub use condition::{
    All, Any, BodyExists, DomComplete, DomInteractive, ResourceTimingIdle, WaitCondition,
};
pub use presets::LoadStrategy;

use crate::{BrowserDriverError, BrowserDriverResult, BrowserPage};

/// Configuration for the `wait_until` poller.
#[derive(Debug, Clone)]
pub struct WaitOptions {
    pub timeout: Duration,
    pub interval: Duration,
}

impl Default for WaitOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(35),
            interval: Duration::from_millis(250),
        }
    }
}

/// Polls the given condition until it is satisfied or the timeout is reached.
///
/// On success or timeout, `cleanup()` is called on the condition.
pub async fn wait_until<C>(
    page: &BrowserPage,
    condition: C,
    options: WaitOptions,
) -> BrowserDriverResult<()>
where
    C: WaitCondition,
{
    let started = Instant::now();

    loop {
        if condition.is_satisfied(page).await? {
            let _ = condition.cleanup(page).await;
            return Ok(());
        }

        if started.elapsed() >= options.timeout {
            let _ = condition.cleanup(page).await;
            return Err(BrowserDriverError::WaitTimeout(format!(
                "wait condition not satisfied within {:?}",
                options.timeout
            )));
        }

        tokio::time::sleep(options.interval).await;
    }
}