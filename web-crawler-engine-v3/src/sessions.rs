//! Browser session pool with health-aware rotation.
//!
//! This module manages the lifecycle of browser sessions used by the crawler.
//! It is a critical component for long-running batch processes.
//!
//! # Responsibilities
//!
//! - Reuse browser sessions per profile key for performance and cache warmth
//! - Proactively rotate sessions based on page count and health signals
//! - Provide observable hooks for session rotation events
//!
//! # Rotation Policy
//!
//! Sessions are rotated when any of the following conditions are met:
//!
//! - The session becomes unhealthy (`BrowserSessionHealth::is_usable()` returns `false`)
//! - The session has served more pages than the configured `max_pages_per_session` limit
//! - The session is explicitly terminated via `terminate_for_request()`
//!
//! # Observability
//!
//! Use [`SessionPool::with_rotation_callback`] to register a handler that is
//! invoked whenever a session is rotated. This is useful for logging and metrics.
//!
//! Rotation reasons are described by [`RotationReason`].

use std::collections::HashMap;

use web_browser_driver::{BrowserDriver, BrowserDriverResult, BrowserSession};

use crate::{
    input::CrawlRequest,
    scheduler::{BrowserProfileStrategy, SessionScheduler},
};

/// Reason a session was rotated or terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationReason {
    /// Session health check failed (`is_usable()` returned false).
    Unhealthy,
    /// Session reached the configured page limit.
    PageLimitReached,
    /// Explicitly terminated via `terminate_for_request`.
    ExplicitTermination,
}

/// Internal wrapper tracking usage.
struct ManagedSession {
    session: BrowserSession,
    pages_served: usize,
}

pub struct SessionPool<'a> {
    driver: &'a BrowserDriver,
    scheduler: SessionScheduler,
    profile_root: std::path::PathBuf,
    active: HashMap<String, ManagedSession>,
    max_pages_per_session: usize,

    /// Optional callback invoked whenever a session is rotated.
    on_rotated: Option<Box<dyn Fn(&str, RotationReason) + Send + Sync>>,
}

impl<'a> SessionPool<'a> {
    pub fn new(
        driver: &'a BrowserDriver,
        strategy: BrowserProfileStrategy,
        profile_root: std::path::PathBuf,
        max_pages_per_session: usize,
    ) -> Self {
        Self {
            driver,
            scheduler: SessionScheduler::new(strategy),
            profile_root,
            active: HashMap::new(),
            max_pages_per_session,
            on_rotated: None,
        }
    }

    /// Register a callback that will be invoked whenever a session is rotated.
    ///
    /// Example:
    /// ```ignore
    /// pool.with_rotation_callback(|profile_key, reason| {
    ///     tracing::info!(profile_key, ?reason, "session rotated");
    /// });
    /// ```
    pub fn with_rotation_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str, RotationReason) + Send + Sync + 'static,
    {
        self.on_rotated = Some(Box::new(callback));
        self
    }

    pub async fn get_or_start<P>(
        &mut self,
        request: &CrawlRequest<P>,
    ) -> BrowserDriverResult<&mut BrowserSession> {
        let assignment = self.scheduler.assign_profile(request);
        let key = assignment.key.as_str().to_string();

        let mut rotation_reason = None;

        if let Some(managed) = self.active.get(&key) {
            if !managed.session.health().is_usable() {
                rotation_reason = Some(RotationReason::Unhealthy);
            } else if managed.pages_served >= self.max_pages_per_session {
                rotation_reason = Some(RotationReason::PageLimitReached);
            }
        }

        if let Some(reason) = rotation_reason {
            if let Some(old) = self.active.remove(&key) {
                let _ = old.session.shutdown().await;
            }
            if let Some(cb) = &self.on_rotated {
                cb(&key, reason);
            }
        }

        if !self.active.contains_key(&key) {
            let profile = self.scheduler.materialize_profile(&assignment, &self.profile_root);
            let new_session = self.driver.start_session(profile).await?;

            self.active.insert(
                key.clone(),
                ManagedSession {
                    session: new_session,
                    pages_served: 0,
                },
            );
        }

        let managed = self.active.get_mut(&key).unwrap();
        managed.pages_served += 1;

        Ok(&mut managed.session)
    }

    pub async fn terminate_for_request<P>(&mut self, request: &CrawlRequest<P>) {
        let assignment = self.scheduler.assign_profile(request);
        let key = assignment.key.as_str().to_string();

        if let Some(managed) = self.active.remove(&key) {
            let _ = managed.session.shutdown().await;
            if let Some(cb) = &self.on_rotated {
                cb(&key, RotationReason::ExplicitTermination);
            }
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub async fn shutdown_all(mut self) {
        for (_, managed) in self.active.drain() {
            let _ = managed.session.shutdown().await;
        }
    }
}

