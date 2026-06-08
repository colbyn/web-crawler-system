//! Browser session health state.
//!
//! This module tracks low-level browser/CDP/session health observed outside any
//! single page operation. It is intentionally primitive and policy-free.
//!
//! The crawler engine may use this state to terminate sessions and retry crawl
//! work later. This crate only reports whether the current browser session still
//! appears safe to use.
//!
//! Health is shared between the CDP handler task and the public
//! `BrowserSession` API. If the handler observes a protocol error,
//! disconnect, or unexpected termination, it marks the session unhealthy.
//!
//! Browser/page operations should check this state before starting work and
//! again after fallible browser operations. This prevents the driver from
//! silently continuing after the browser transport has become unreliable.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::{BrowserDriverError, BrowserDriverResult};

#[derive(Debug, Clone, Default)]
pub struct BrowserSessionHealth {
    inner: Arc<BrowserSessionHealthInner>,
}

#[derive(Debug, Default)]
struct BrowserSessionHealthInner {
    cdp_disconnected: AtomicBool,
    cdp_error_count: AtomicUsize,
    handler_finished: AtomicBool,
}

impl BrowserSessionHealth {
    /// Records a CDP/protocol-level error observed by the background handler.
    ///
    /// A CDP error does not necessarily mean the connection is gone, but in
    /// practice these errors are strong evidence that the session should be
    /// treated cautiously.
    pub fn observe_cdp_error(&self) {
        self.inner.cdp_error_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Records that the CDP handler observed, or inferred, a disconnect.
    ///
    /// This is intentionally sticky. Once a session is marked disconnected, it
    /// should not become usable again.
    pub fn observe_cdp_disconnected(&self) {
        self.inner.cdp_disconnected.store(true, Ordering::Release);
    }

    /// Records that the CDP handler task ended.
    ///
    /// For a live `BrowserSession`, handler termination means the event pump is
    /// no longer running. That makes the session unsafe to reuse.
    pub fn observe_handler_finished(&self) {
        self.inner.handler_finished.store(true, Ordering::Release);
    }

    pub fn cdp_error_count(&self) -> usize {
        self.inner.cdp_error_count.load(Ordering::Relaxed)
    }

    pub fn is_cdp_disconnected(&self) -> bool {
        self.inner.cdp_disconnected.load(Ordering::Acquire)
    }

    pub fn is_handler_finished(&self) -> bool {
        self.inner.handler_finished.load(Ordering::Acquire)
    }

    pub fn is_usable(&self) -> bool {
        !self.is_cdp_disconnected() && !self.is_handler_finished()
    }

    /// Returns an error if the browser session should no longer be used.
    pub fn check_usable(&self) -> BrowserDriverResult<()> {
        if self.is_cdp_disconnected() {
            return Err(BrowserDriverError::CdpDisconnected(
                "CDP handler reported disconnection".into(),
            ));
        }

        if self.is_handler_finished() {
            return Err(BrowserDriverError::CdpDisconnected(
                "CDP handler finished unexpectedly".into(),
            ));
        }

        Ok(())
    }

    /// Prefer a session-health error over a lower-level operation error.
    ///
    /// This is useful after a Chromiumoxide call fails. If the handler has also
    /// marked the session unhealthy, the higher-level error should communicate
    /// that the whole session is contaminated, not merely that one page action
    /// failed.
    pub fn prefer_health_error(
        &self,
        fallback: BrowserDriverError,
    ) -> BrowserDriverError {
        match self.check_usable() {
            Ok(()) => fallback,
            Err(health_error) => health_error,
        }
    }
}

