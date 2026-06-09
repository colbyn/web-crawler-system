//! Browser session pool with health-aware rotation and page-slot leasing.
//!
//! This module manages browser session lifecycle for the crawler engine.
//!
//! A browser session is a running browser process/client associated with one
//! explicit browser profile. Sessions are expensive enough that the crawler
//! should reuse them, but fragile enough that the crawler must rotate or retire
//! them when health degrades.
//!
//! ## Throughput model
//!
//! The pool supports hybrid concurrency:
//!
//! ```text
//! N browser sessions × M concurrent pages per session
//! ```
//!
//! This is preferable to launching one browser per page job. Chromium launch and
//! profile warmup are expensive, while browser tabs/pages are much cheaper.
//!
//! ## Important safety default
//!
//! Start with `max_concurrent_pages_per_session = 1` if unsure. Once the engine
//! compiles and runs cleanly, increase to 2 or 4 and watch telemetry.
//!
//! ## Rotation policy
//!
//! Sessions are rotated when:
//!
//! - the session becomes unhealthy,
//! - the session has started more than `max_pages_per_session` page jobs,
//! - the session is explicitly terminated.
//!
//! A retired session is removed from the active map. Existing page leases may
//! finish using it. The pool will not assign new page jobs to retired sessions.

use std::{
    collections::HashMap,
    future::Future,
    sync::{
        atomic::{
            AtomicBool,
            AtomicUsize,
            Ordering,
        },
        Arc,
    },
};

use tokio::sync::{
    Mutex,
    OwnedSemaphorePermit,
    Semaphore,
};
use web_browser_driver::{
    BrowserDriver,
    BrowserDriverError,
    BrowserDriverResult,
    BrowserSession,
};

use crate::{
    input::CrawlRequest,
    scheduler::{
        BrowserProfileStrategy,
        SessionScheduler,
    },
};

/// Reason a session was rotated or terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationReason {
    /// Session health check failed.
    Unhealthy,

    /// Session reached the configured page limit.
    PageLimitReached,

    /// Explicitly terminated via `terminate_for_request`.
    ExplicitTermination,
}

type RotationCallback = Arc<dyn Fn(&str, RotationReason) + Send + Sync>;

struct ManagedSession {
    key: String,
    session: Arc<BrowserSession>,

    /// Number of page jobs started on this session.
    pages_started: AtomicUsize,

    /// Prevents this retired session from being selected for new work.
    retired: AtomicBool,

    /// Holds one global live-session permit for this session's lifetime.
    _session_permit: OwnedSemaphorePermit,

    /// Per-session page capacity.
    page_permits: Arc<Semaphore>,
}

impl ManagedSession {
    fn is_usable_for_new_work(&self, max_pages_per_session: usize) -> bool {
        if self.retired.load(Ordering::Acquire) {
            return false;
        }

        if !self.session.health().is_usable() {
            return false;
        }

        self.pages_started.load(Ordering::Acquire) < max_pages_per_session
    }

    fn mark_retired(&self) {
        self.retired.store(true, Ordering::Release);
    }
}

/// A leased page slot on a browser session.
///
/// Holding this value means the caller has permission to start/use one page job
/// on the associated browser session.
pub struct SessionPageLease {
    managed: Arc<ManagedSession>,
    _page_permit: OwnedSemaphorePermit,
}

impl SessionPageLease {
    pub fn session(&self) -> Arc<BrowserSession> {
        self.managed.session.clone()
    }

    pub fn profile_key(&self) -> &str {
        &self.managed.key
    }
}

struct SessionPoolInner {
    driver: BrowserDriver,
    scheduler: SessionScheduler,
    profile_root: std::path::PathBuf,

    active: Mutex<HashMap<String, Arc<ManagedSession>>>,

    session_permits: Arc<Semaphore>,

    max_sessions: usize,
    max_pages_per_session: usize,
    max_concurrent_pages_per_session: usize,

    on_rotated: Option<RotationCallback>,
}

#[derive(Clone)]
pub struct SessionPool {
    inner: Arc<SessionPoolInner>,
}

impl SessionPool {
    pub fn new(
        driver: BrowserDriver,
        strategy: BrowserProfileStrategy,
        profile_root: std::path::PathBuf,
        max_sessions: usize,
        max_pages_per_session: usize,
        max_concurrent_pages_per_session: usize,
    ) -> Self {
        let max_sessions = max_sessions.max(1);
        let max_concurrent_pages_per_session =
            max_concurrent_pages_per_session.max(1);

        Self {
            inner: Arc::new(SessionPoolInner {
                driver,
                scheduler: SessionScheduler::new(strategy),
                profile_root,
                active: Mutex::new(HashMap::new()),
                session_permits: Arc::new(Semaphore::new(max_sessions)),
                max_sessions,
                max_pages_per_session,
                max_concurrent_pages_per_session,
                on_rotated: None,
            }),
        }
    }

    pub fn with_rotation_callback<F>(self, callback: F) -> Self
    where
        F: Fn(&str, RotationReason) + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(SessionPoolInner {
                driver: self.inner.driver.clone(),
                scheduler: self.inner.scheduler.clone(),
                profile_root: self.inner.profile_root.clone(),
                active: Mutex::new(HashMap::new()),
                session_permits: self.inner.session_permits.clone(),
                max_sessions: self.inner.max_sessions,
                max_pages_per_session: self.inner.max_pages_per_session,
                max_concurrent_pages_per_session: self
                    .inner
                    .max_concurrent_pages_per_session,
                on_rotated: Some(Arc::new(callback)),
            }),
        }
    }

    /// Run work with a leased browser session page slot.
    ///
    /// The closure receives an `Arc<BrowserSession>`, not `&mut BrowserSession`.
    /// This is what allows multiple page jobs to share one browser session.
    pub async fn with_session<P, F, Fut, T>(
        &self,
        request: &CrawlRequest<P>,
        f: F,
    ) -> BrowserDriverResult<T>
    where
        F: FnOnce(Arc<BrowserSession>) -> Fut,
        Fut: Future<Output = BrowserDriverResult<T>>,
    {
        let lease = self.lease_page_slot(request).await?;
        let session = lease.session();

        let result = f(session).await;

        if result
            .as_ref()
            .err()
            .is_some_and(|err| err.should_terminate_session())
        {
            lease.managed.mark_retired();

            if let Some(callback) = &self.inner.on_rotated {
                callback(lease.profile_key(), RotationReason::ExplicitTermination);
            }
        }

        result
    }

    pub async fn lease_page_slot<P>(
        &self,
        request: &CrawlRequest<P>,
    ) -> BrowserDriverResult<SessionPageLease> {
        let managed = self.get_or_start(request).await?;

        let permit = managed
            .page_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| {
                BrowserDriverError::BrowserUnhealthy(
                    "session page semaphore closed".into(),
                )
            })?;

        managed.pages_started.fetch_add(1, Ordering::AcqRel);

        Ok(SessionPageLease {
            managed,
            _page_permit: permit,
        })
    }

    pub async fn terminate_for_request<P>(&self, request: &CrawlRequest<P>) {
        let assignment = self.inner.scheduler.assign_profile(request);
        let key = assignment.key.as_str().to_string();

        let removed = {
            let mut active = self.inner.active.lock().await;
            active.remove(&key)
        };

        if let Some(managed) = removed {
            managed.mark_retired();

            if let Some(callback) = &self.inner.on_rotated {
                callback(&key, RotationReason::ExplicitTermination);
            }

            self.best_effort_shutdown(managed).await;
        }
    }

    pub async fn active_count(&self) -> usize {
        let active = self.inner.active.lock().await;
        active.len()
    }

    pub async fn shutdown_all(&self) {
        let sessions = {
            let mut active = self.inner.active.lock().await;
            active.drain().map(|(_, session)| session).collect::<Vec<_>>()
        };

        for managed in sessions {
            managed.mark_retired();
            self.best_effort_shutdown(managed).await;
        }
    }

    async fn get_or_start<P>(
        &self,
        request: &CrawlRequest<P>,
    ) -> BrowserDriverResult<Arc<ManagedSession>> {
        let assignment = self.inner.scheduler.assign_profile(request);
        let key = assignment.key.as_str().to_string();

        let mut retired: Option<(Arc<ManagedSession>, RotationReason)> = None;

        {
            let mut active = self.inner.active.lock().await;

            if let Some(existing) = active.get(&key) {
                if existing.is_usable_for_new_work(self.inner.max_pages_per_session) {
                    return Ok(existing.clone());
                }

                let reason = if !existing.session.health().is_usable() {
                    RotationReason::Unhealthy
                } else {
                    RotationReason::PageLimitReached
                };

                let old = active.remove(&key).expect("existing session disappeared");
                old.mark_retired();
                retired = Some((old, reason));
            }
        }

        if let Some((old, reason)) = retired {
            if let Some(callback) = &self.inner.on_rotated {
                callback(&key, reason);
            }

            // Do not let cleanup block new session creation forever. This
            // function is best-effort and quick if no other leases exist.
            self.best_effort_shutdown(old).await;
        }

        // Acquire global live-session capacity before launching Chromium.
        let session_permit = self
            .inner
            .session_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| {
                BrowserDriverError::BrowserUnhealthy(
                    "global session semaphore closed".into(),
                )
            })?;

        let profile = self
            .inner
            .scheduler
            .materialize_profile(&assignment, &self.inner.profile_root);

        let session = self.inner.driver.start_session(profile).await?;

        let managed = Arc::new(ManagedSession {
            key: key.clone(),
            session: Arc::new(session),
            pages_started: AtomicUsize::new(0),
            retired: AtomicBool::new(false),
            _session_permit: session_permit,
            page_permits: Arc::new(Semaphore::new(
                self.inner.max_concurrent_pages_per_session,
            )),
        });

        let mut active = self.inner.active.lock().await;

        // Another task may have inserted while this one was launching. Prefer
        // the existing usable session and retire this just-launched duplicate.
        if let Some(existing) = active.get(&key).cloned() {
            if existing.is_usable_for_new_work(self.inner.max_pages_per_session) {
                managed.mark_retired();
                drop(active);

                self.best_effort_shutdown(managed).await;

                return Ok(existing);
            }
        }

        active.insert(key, managed.clone());

        Ok(managed)
    }

    async fn best_effort_shutdown(&self, managed: Arc<ManagedSession>) {
        // If other page leases still exist, this will fail and the session will
        // be dropped later. That is acceptable during migration. Once the engine
        // is stable, we can add a retired-session reaper that waits for all page
        // permits and shuts down deterministically.
        let Ok(managed) = Arc::try_unwrap(managed) else {
            return;
        };

        let Ok(session) = Arc::try_unwrap(managed.session) else {
            return;
        };

        let _ = session.shutdown().await;
    }
}

