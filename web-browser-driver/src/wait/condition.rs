//! Composable wait conditions.
//!
//! Keep these generic and browser/page oriented. Do not encode crawl-specific
//! assumptions here.
//!
//! For example, "body exists" is a browser/page readiness signal.
//! "career link exists" is an app/crawler extraction signal and belongs
//! elsewhere.
//! 
//! Composable wait conditions.
//!
//! These are browser/page-oriented readiness signals.

use std::time::Duration;

use async_trait::async_trait;

use crate::{BrowserDriverResult, BrowserPage};

#[async_trait]
pub trait WaitCondition: Send + Sync {
    async fn is_satisfied(&self, page: &BrowserPage) -> BrowserDriverResult<bool>;

    async fn cleanup(&self, _page: &BrowserPage) -> BrowserDriverResult<()> {
        Ok(())
    }
}

pub struct All(pub Vec<Box<dyn WaitCondition>>);
pub struct Any(pub Vec<Box<dyn WaitCondition>>);

pub struct DomInteractive;
pub struct DomComplete;
pub struct BodyExists;

pub struct ResourceTimingIdle {
    pub idle_for: Duration,
}

impl ResourceTimingIdle {
    pub fn new(idle_for: Duration) -> Self {
        Self { idle_for }
    }
}

#[async_trait]
impl WaitCondition for All {
    async fn is_satisfied(&self, page: &BrowserPage) -> BrowserDriverResult<bool> {
        for cond in &self.0 {
            if !cond.is_satisfied(page).await? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn cleanup(&self, page: &BrowserPage) -> BrowserDriverResult<()> {
        for cond in &self.0 {
            let _ = cond.cleanup(page).await;
        }
        Ok(())
    }
}

#[async_trait]
impl WaitCondition for Any {
    async fn is_satisfied(&self, page: &BrowserPage) -> BrowserDriverResult<bool> {
        for cond in &self.0 {
            if cond.is_satisfied(page).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn cleanup(&self, page: &BrowserPage) -> BrowserDriverResult<()> {
        for cond in &self.0 {
            let _ = cond.cleanup(page).await;
        }
        Ok(())
    }
}

#[async_trait]
impl WaitCondition for DomInteractive {
    async fn is_satisfied(&self, page: &BrowserPage) -> BrowserDriverResult<bool> {
        let value = page
            .eval_json(
                r#"(() => {
                    const rs = document.readyState;
                    return rs === "interactive" || rs === "complete";
                })()"#,
            )
            .await?;
        Ok(value.as_bool().unwrap_or(false))
    }
}

#[async_trait]
impl WaitCondition for DomComplete {
    async fn is_satisfied(&self, page: &BrowserPage) -> BrowserDriverResult<bool> {
        let value = page
            .eval_json(r#"(() => document.readyState === "complete")()"#)
            .await?;
        Ok(value.as_bool().unwrap_or(false))
    }
}

#[async_trait]
impl WaitCondition for BodyExists {
    async fn is_satisfied(&self, page: &BrowserPage) -> BrowserDriverResult<bool> {
        let value = page.eval_json(r#"(() => !!document.body)()"#).await?;
        Ok(value.as_bool().unwrap_or(false))
    }
}

#[async_trait]
impl WaitCondition for ResourceTimingIdle {
    async fn is_satisfied(&self, page: &BrowserPage) -> BrowserDriverResult<bool> {
        let idle_ms = self.idle_for.as_millis();
        let script = format!(
            r#"
            (() => {{
                const entries = performance.getEntriesByType("resource");
                if (!entries || entries.length === 0) return true;

                const now = performance.now();
                const latest = entries.reduce((max, e) => {{
                    const end = Number(e.responseEnd || 0);
                    const start = Number(e.startTime || 0);
                    const dur = Number(e.duration || 0);
                    return Math.max(max, end, start + dur);
                }}, 0);

                return (now - latest) >= {idle_ms};
            }})()
            "#
        );
        let value = page.eval_json(&script).await?;
        Ok(value.as_bool().unwrap_or(false))
    }
}