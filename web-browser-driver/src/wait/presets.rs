//! Named load strategies.
//!
//! These are practical presets, not universal truths.
//!
//! The crawler/app may pick different strategies depending on whether it wants
//! speed, SPA tolerance, or deeper load completeness.
//!
//! Load strategies remain browser/page-oriented. They should record low-level
//! readiness facts into `PageTelemetry`, but they should not decide whether a
//! page snapshot is authoritative crawl evidence.
//!
//! In other words: this module may say “the DOM was interactive and resource
//! timing was quiet for 500ms.” It must not say “this business website is done
//! crawling.”

use std::time::{
    Duration,
    Instant,
};

use crate::{
    BrowserDriverError, BrowserDriverResult, BrowserPage, DocumentReadyState, PageTelemetryBuilder, PageTelemetryEventKind, 
};

use crate::wait::{
    All,
    BodyExists,
    DomInteractive,
    ResourceTimingIdle,
    WaitCondition,
    WaitOptions,
};

/// Practical page-readiness presets.
///
/// These are intentionally heuristic. Browser automation does not expose a
/// universal state meaning “this page is now maximally useful for scraping.”
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStrategy {
    /// Do not wait after issuing the navigation command.
    ///
    /// Useful for callers that want to perform their own custom wait logic.
    None,

    /// Wait for an interactive DOM, body presence, a short Performance API
    /// resource quiet window, then apply a small grace delay.
    ///
    /// This favors ordinary server-rendered and lightly hydrated pages without
    /// waiting for full document completion.
    Default,
}

impl LoadStrategy {
    pub async fn wait(
        &self,
        page: &BrowserPage,
        telemetry: &mut PageTelemetryBuilder,
        navigation_timeout: Option<Duration>,
        max_timeout: Option<Duration>,
    ) -> BrowserDriverResult<()> {
        match self {
            LoadStrategy::None => {
                observe_basic_readiness(page, telemetry).await;
                Ok(())
            }

            LoadStrategy::Default => {
                run_wait_strategy(
                    page,
                    telemetry,
                    All(vec![
                        Box::new(DomInteractive),
                        Box::new(BodyExists),
                        Box::new(ResourceTimingIdle::new(Duration::from_millis(500))),
                    ]),
                    WaitOptions {
                        timeout: {
                            navigation_timeout.or(max_timeout).unwrap_or(Duration::from_secs(10))
                        },
                        interval: Duration::from_millis(250),
                    },
                    Some(Duration::from_millis(500)),
                )
                .await
            }
        }
    }
}

async fn run_wait_strategy<C>(
    page: &BrowserPage,
    telemetry: &mut PageTelemetryBuilder,
    condition: C,
    options: WaitOptions,
    grace_delay: Option<Duration>,
) -> BrowserDriverResult<()>
where
    C: WaitCondition,
{
    telemetry.mark_wait_started();

    observe_basic_readiness(page, telemetry).await;

    let started = Instant::now();
    let result = page.wait_until(condition, options).await;
    let elapsed = started.elapsed();

    observe_basic_readiness(page, telemetry).await;

    match result {
        Ok(()) => {
            telemetry.mark_wait_satisfied(elapsed);

            if let Some(delay) = grace_delay {
                telemetry.mark_grace_delay(delay);
                tokio::time::sleep(delay).await;

                observe_basic_readiness(page, telemetry).await;
            }

            Ok(())
        }

        Err(err) => {
            if matches!(err, BrowserDriverError::WaitTimeout(_)) {
                telemetry.mark_wait_timed_out(elapsed);

                // Lenient crawler rule:
                //
                // A wait timeout is not necessarily a bad page. Many real-world
                // sites never become "quiet" because analytics, ads, fonts,
                // chat widgets, maps, or broken third-party scripts keep
                // bubbling forever.
                //
                // If the DOM is already usable, accept the page and preserve the
                // timeout in telemetry instead of turning scrapeable evidence
                // into a hard crawl failure.
                if telemetry.telemetry().dom_was_available() {
                    telemetry.push_event(
                        PageTelemetryEventKind::Other,
                        format!("soft-accepted wait timeout because DOM was available: {err}"),
                    );

                    return Ok(());
                }
            }

            Err(err)
        }
    }
}

async fn observe_basic_readiness(
    page: &BrowserPage,
    telemetry: &mut PageTelemetryBuilder,
) {
    let value = page
        .eval_json(
            r#"
            (() => ({
                readyState: document.readyState || null,
                bodyPresent: !!document.body
            }))()
            "#,
        )
        .await;

    let Ok(value) = value else {
        return;
    };

    if let Some(ready_state) = value.get("readyState").and_then(|v| v.as_str()) {
        telemetry.observe_ready_state(DocumentReadyState::from_browser_value(
            ready_state,
        ));
    }

    if let Some(body_present) = value.get("bodyPresent").and_then(|v| v.as_bool()) {
        telemetry.observe_body_present(body_present);
    }
}
