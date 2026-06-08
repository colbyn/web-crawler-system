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
    wait::{
        All,
        BodyExists,
        DomComplete,
        DomInteractive,
        ResourceTimingIdle,
        WaitCondition,
        WaitOptions,
    },
    BrowserDriverError,
    BrowserDriverResult,
    BrowserPage,
    DocumentReadyState,
    PageTelemetryBuilder,
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

    /// Wait for an interactive DOM and a body element.
    ///
    /// This is the fastest useful scrape-oriented preset. It favors throughput
    /// over hydration completeness.
    FastDom,

    /// Wait for an interactive DOM, body presence, a short Performance API
    /// resource quiet window, then apply a small grace delay.
    ///
    /// This favors ordinary server-rendered and lightly hydrated pages without
    /// waiting for full document completion.
    Balanced,

    /// Wait longer for resource quiet and apply a longer grace delay.
    ///
    /// This is useful for slower SPA/hydrated pages, but still remains a
    /// heuristic. It does not prove all client-side work has finished.
    SpaTolerant,

    /// Wait for `document.readyState === "complete"` and body presence.
    ///
    /// This can be useful when the caller specifically wants browser load
    /// completion semantics, but it is not necessarily better for scraping than
    /// `Balanced`.
    DocumentComplete,
}

impl LoadStrategy {
    pub async fn wait(
        &self,
        page: &BrowserPage,
        telemetry: &mut PageTelemetryBuilder,
    ) -> BrowserDriverResult<()> {
        match self {
            LoadStrategy::None => {
                observe_basic_readiness(page, telemetry).await;
                Ok(())
            }

            LoadStrategy::FastDom => {
                run_wait_strategy(
                    page,
                    telemetry,
                    All(vec![
                        Box::new(DomInteractive),
                        Box::new(BodyExists),
                    ]),
                    WaitOptions {
                        timeout: Duration::from_secs(15),
                        interval: Duration::from_millis(150),
                    },
                    None,
                )
                .await
            }

            LoadStrategy::Balanced => {
                run_wait_strategy(
                    page,
                    telemetry,
                    All(vec![
                        Box::new(DomInteractive),
                        Box::new(BodyExists),
                        Box::new(ResourceTimingIdle::new(Duration::from_millis(500))),
                    ]),
                    WaitOptions {
                        timeout: Duration::from_secs(35),
                        interval: Duration::from_millis(250),
                    },
                    Some(Duration::from_millis(750)),
                )
                .await
            }

            LoadStrategy::SpaTolerant => {
                run_wait_strategy(
                    page,
                    telemetry,
                    All(vec![
                        Box::new(DomInteractive),
                        Box::new(BodyExists),
                        Box::new(ResourceTimingIdle::new(Duration::from_millis(1250))),
                    ]),
                    WaitOptions {
                        timeout: Duration::from_secs(55),
                        interval: Duration::from_millis(250),
                    },
                    Some(Duration::from_millis(1500)),
                )
                .await
            }

            LoadStrategy::DocumentComplete => {
                run_wait_strategy(
                    page,
                    telemetry,
                    All(vec![
                        Box::new(DomComplete),
                        Box::new(BodyExists),
                    ]),
                    WaitOptions {
                        timeout: Duration::from_secs(35),
                        interval: Duration::from_millis(250),
                    },
                    None,
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
