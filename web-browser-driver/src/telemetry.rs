//! Browser/page telemetry captured during navigation and extraction.
//!
//! This module records low-level observations. It should avoid deciding whether
//! a page is "good", "bad", "retryable", or "contaminated".
//!
//! Telemetry is intentionally persistable. The crawler/app may later reevaluate
//! whether a saved snapshot should be trusted, refreshed, discarded, or retried
//! under newer policy.
//!
//! Runtime-only timing helpers belong in `PageTelemetryBuilder`, not in
//! `PageTelemetry`.

use std::time::{
    Duration,
    Instant,
};
use schemars::JsonSchema;
use serde::{
    Deserialize,
    Serialize,
};

/// Low-level telemetry recorded while opening, waiting on, and inspecting a page.
///
/// This type is intended to be persisted with the crawl result.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct PageTelemetry {
    /// Overall navigation/open telemetry.
    pub navigation: NavigationTelemetry,

    /// Browser document readiness observations.
    pub readiness: ReadinessTelemetry,

    /// Network activity observations.
    ///
    /// Early implementations may fill this from browser-side Performance API
    /// facts. Later implementations can enrich it with CDP Network events.
    pub network: NetworkTelemetry,

    /// Chrome/CDP/session health observations.
    pub browser_health: BrowserHealthTelemetry,

    /// Low-level lifecycle events useful for later debugging and policy tuning.
    pub events: Vec<PageTelemetryEvent>,
}

impl PageTelemetry {
    /// Convenience helper only.
    ///
    /// This is intentionally not a policy decision. The crawler/app may use a
    /// stricter or looser definition depending on the crawl mode.
    pub fn dom_was_available(&self) -> bool {
        self.readiness.body_present == Some(true)
            || matches!(
                self.readiness.final_ready_state,
                Some(DocumentReadyState::Interactive | DocumentReadyState::Complete)
            )
    }

    /// Convenience helper only.
    ///
    /// CDP disconnection usually means the browser session should not be reused.
    pub fn observed_cdp_disconnect(&self) -> bool {
        self.browser_health.disconnect_count > 0
            || self.events.iter().any(|event| {
                event.kind == PageTelemetryEventKind::CdpDisconnected
            })
    }

    /// Convenience helper only.
    ///
    /// A nonzero failure ratio is not automatically fatal. Many pages have noisy
    /// third-party resources. Treat this as a feature for later scoring.
    pub fn network_failure_ratio(&self) -> Option<f64> {
        let total = self.network.finished_count + self.network.failed_count;

        if total == 0 {
            return None;
        }

        Some(self.network.failed_count as f64 / total as f64)
    }
}

/// Runtime helper for building persistable `PageTelemetry`.
///
/// This type should not be persisted. It owns an `Instant`, which is only useful
/// inside the current process.
#[derive(Debug)]
pub struct PageTelemetryBuilder {
    telemetry: PageTelemetry,
    started_at: Instant,
}

impl PageTelemetryBuilder {
    pub fn started_now() -> Self {
        Self {
            telemetry: PageTelemetry::default(),
            started_at: Instant::now(),
        }
    }

    pub fn into_telemetry(self) -> PageTelemetry {
        self.telemetry
    }

    pub fn telemetry(&self) -> &PageTelemetry {
        &self.telemetry
    }

    pub fn telemetry_mut(&mut self) -> &mut PageTelemetry {
        &mut self.telemetry
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn mark_navigation_elapsed_now(&mut self) {
        self.telemetry.navigation.elapsed = Some(self.elapsed());
    }

    pub fn push_event(&mut self, kind: PageTelemetryEventKind, message: impl Into<String>) {
        let elapsed_since_start = Some(self.elapsed());

        self.telemetry.events.push(PageTelemetryEvent {
            kind,
            message: message.into(),
            elapsed_since_start,
        });
    }

    pub fn mark_navigation_started(&mut self) {
        self.push_event(
            PageTelemetryEventKind::NavigationStarted,
            "navigation started",
        );
    }

    pub fn mark_navigation_command_succeeded(&mut self) {
        self.telemetry.navigation.navigation_command_succeeded = Some(true);

        self.push_event(
            PageTelemetryEventKind::NavigationCommandSucceeded,
            "navigation command succeeded",
        );
    }

    pub fn mark_navigation_command_failed(&mut self, message: impl Into<String>) {
        self.telemetry.navigation.navigation_command_succeeded = Some(false);

        self.push_event(
            PageTelemetryEventKind::NavigationCommandFailed,
            message,
        );
    }

    pub fn observe_ready_state(&mut self, ready_state: DocumentReadyState) {
        self.telemetry.readiness.final_ready_state = Some(ready_state);

        self.push_event(
            PageTelemetryEventKind::DocumentReadyStateObserved,
            format!("document.readyState observed as {ready_state:?}"),
        );
    }

    pub fn observe_body_present(&mut self, body_present: bool) {
        self.telemetry.readiness.body_present = Some(body_present);

        self.push_event(
            PageTelemetryEventKind::BodyPresenceObserved,
            format!("document.body present: {body_present}"),
        );
    }

    pub fn mark_wait_started(&mut self) {
        self.push_event(
            PageTelemetryEventKind::WaitStrategyStarted,
            "wait strategy started",
        );
    }

    pub fn mark_wait_satisfied(&mut self, elapsed: Duration) {
        self.telemetry.readiness.wait_strategy_satisfied = Some(true);
        self.telemetry.readiness.wait_elapsed = Some(elapsed);

        self.push_event(
            PageTelemetryEventKind::WaitStrategySatisfied,
            format!("wait strategy satisfied after {elapsed:?}"),
        );
    }

    pub fn mark_wait_timed_out(&mut self, elapsed: Duration) {
        self.telemetry.readiness.wait_strategy_satisfied = Some(false);
        self.telemetry.readiness.wait_elapsed = Some(elapsed);

        self.push_event(
            PageTelemetryEventKind::WaitStrategyTimedOut,
            format!("wait strategy timed out after {elapsed:?}"),
        );
    }

    pub fn mark_grace_delay(&mut self, delay: Duration) {
        self.telemetry.readiness.grace_delay = Some(delay);

        self.push_event(
            PageTelemetryEventKind::GraceDelayApplied,
            format!("grace delay applied: {delay:?}"),
        );
    }

    pub fn observe_network_request_started(&mut self, message: impl Into<String>) {
        self.telemetry.network.request_count += 1;
        self.telemetry.network.inflight_count += 1;

        self.push_event(PageTelemetryEventKind::NetworkRequestStarted, message);
    }

    pub fn observe_network_request_finished(&mut self, message: impl Into<String>) {
        self.telemetry.network.finished_count += 1;
        self.telemetry.network.inflight_count =
            self.telemetry.network.inflight_count.saturating_sub(1);

        self.push_event(PageTelemetryEventKind::NetworkRequestFinished, message);
    }

    pub fn observe_network_request_failed(&mut self, message: impl Into<String>) {
        self.telemetry.network.failed_count += 1;
        self.telemetry.network.inflight_count =
            self.telemetry.network.inflight_count.saturating_sub(1);

        self.push_event(PageTelemetryEventKind::NetworkRequestFailed, message);
    }

    pub fn observe_cdp_error(&mut self, message: impl Into<String>) {
        self.telemetry.browser_health.cdp_error_count += 1;

        self.push_event(PageTelemetryEventKind::CdpErrorObserved, message);
    }

    pub fn observe_cdp_disconnected(&mut self, message: impl Into<String>) {
        self.telemetry.browser_health.disconnect_count += 1;
        self.telemetry.browser_health.cdp_connected_at_end = Some(false);

        self.push_event(PageTelemetryEventKind::CdpDisconnected, message);
    }

    pub fn observe_browser_process_exited(&mut self, message: impl Into<String>) {
        self.telemetry
            .browser_health
            .browser_process_alive_at_end = Some(false);

        self.push_event(PageTelemetryEventKind::BrowserProcessExited, message);
    }
}

/// Navigation/open timing and outcome facts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct NavigationTelemetry {
    /// Wall-clock time spent in the open/navigate operation.
    pub elapsed: Option<Duration>,

    /// Whether the browser navigation command returned successfully.
    pub navigation_command_succeeded: Option<bool>,

    /// HTTP-ish status for the main document when known.
    pub main_resource_status_code: Option<u16>,

    /// Number of redirects observed by the driver.
    pub observed_redirect_count: usize,
}

/// Browser document readiness observations.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ReadinessTelemetry {
    /// Last observed `document.readyState`.
    pub final_ready_state: Option<DocumentReadyState>,

    /// Whether `document.body` existed when checked.
    pub body_present: Option<bool>,

    /// Whether the selected wait strategy completed.
    ///
    /// `None` means no wait strategy was applied or the operation failed before
    /// reaching the wait phase.
    pub wait_strategy_satisfied: Option<bool>,

    /// How long the selected wait strategy ran.
    pub wait_elapsed: Option<Duration>,

    /// If a final grace delay was used, record it.
    pub grace_delay: Option<Duration>,
}

/// Browser document ready state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentReadyState {
    Loading,
    Interactive,
    Complete,
    Unknown,
}

impl DocumentReadyState {
    pub fn from_browser_value(value: &str) -> Self {
        match value {
            "loading" => Self::Loading,
            "interactive" => Self::Interactive,
            "complete" => Self::Complete,
            _ => Self::Unknown,
        }
    }
}

/// Network activity observations.
///
/// These fields are deliberately primitive. They can be filled by CDP Network
/// events, browser Performance API measurements, or both.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct NetworkTelemetry {
    /// Number of requests observed.
    pub request_count: usize,

    /// Number of requests that completed successfully enough for the browser/CDP
    /// layer to report completion.
    pub finished_count: usize,

    /// Number of requests that failed.
    pub failed_count: usize,

    /// Number of requests still in flight when observation ended.
    pub inflight_count: usize,

    /// Most recent observed network activity before observation ended.
    pub last_activity_age: Option<Duration>,

    /// Approximate quiet window observed before snapshot.
    pub observed_idle_for: Option<Duration>,

    /// Resource timing count from `performance.getEntriesByType("resource")`.
    pub performance_resource_count: Option<usize>,

    /// Latest resource end time from the browser Performance API.
    pub latest_performance_resource_end_ms: Option<f64>,
}

/// Browser/CDP/session health observations.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct BrowserHealthTelemetry {
    /// Whether the CDP connection appeared alive at the end of the operation.
    pub cdp_connected_at_end: Option<bool>,

    /// Whether the browser process appeared alive at the end of the operation.
    pub browser_process_alive_at_end: Option<bool>,

    /// Number of CDP/protocol errors observed during the operation.
    pub cdp_error_count: usize,

    /// Number of session/page disconnect events observed.
    pub disconnect_count: usize,
}

/// One low-level lifecycle/event observation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct PageTelemetryEvent {
    pub kind: PageTelemetryEventKind,
    pub message: String,
    pub elapsed_since_start: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageTelemetryEventKind {
    NavigationStarted,
    NavigationCommandSucceeded,
    NavigationCommandFailed,
    RedirectObserved,

    DocumentReadyStateObserved,
    BodyPresenceObserved,

    WaitStrategyStarted,
    WaitStrategySatisfied,
    WaitStrategyTimedOut,
    GraceDelayApplied,

    NetworkRequestStarted,
    NetworkRequestFinished,
    NetworkRequestFailed,
    NetworkIdleObserved,

    CdpErrorObserved,
    CdpDisconnected,
    BrowserProcessExited,

    SnapshotAboutToBeCaptured,

    Other,
}
