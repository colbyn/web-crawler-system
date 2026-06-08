//! Browser page abstraction.
//!
//! `BrowserPage` wraps a concrete page/tab handle from the underlying browser
//! automation library.
//!
//! This type should offer scrape-oriented primitives:
//!
//! - evaluate JavaScript,
//! - retrieve HTML/text,
//! - inspect current URL,
//! - run extraction helpers,
//! - close the page.
//!
//! It should not know about seed groups, crawl depth, business entities, or
//! frontier scheduling.
//!
//! `BrowserPage` wraps a `chromiumoxide::Page` and provides a clean,
//! scrape-oriented API:
//!
//! - Navigation state inspection (`current_url`)
//! - Content retrieval (`html`, `text`)
//! - JavaScript evaluation (`eval_json`)
//! - Composable waiting (`wait_until`)
//! - Clean shutdown (`close`)
//!
//! It deliberately does **not** know about crawl policy, seeds, or business
//! entities. Those concerns live in the crawler engine and extractors.

use crate::{
    wait::{WaitCondition, WaitOptions},
    BrowserDriverResult,
};

pub struct BrowserPage {
    inner: Option<chromiumoxide::Page>,
}

impl BrowserPage {
    // pub(crate) fn new() -> Self {
    //     Self { inner: None }
    // }

    pub(crate) fn from_chromiumoxide(page: chromiumoxide::Page) -> Self {
        Self { inner: Some(page) }
    }

    pub async fn current_url(&self) -> BrowserDriverResult<String> {
        let page = self.inner.as_ref().ok_or_else(|| {
            crate::BrowserDriverError::Internal("BrowserPage not initialized".into())
        })?;

        // FIX: url() returns Option<String>
        let url = page
            .url()
            .await
            .map_err(|e| crate::BrowserDriverError::Page(e.to_string()))?
            .unwrap_or_default();

        Ok(url)
    }

    pub async fn html(&self) -> BrowserDriverResult<String> {
        let page = self.inner.as_ref().ok_or_else(|| {
            crate::BrowserDriverError::Internal("BrowserPage not initialized".into())
        })?;
        page.content()
            .await
            .map_err(|e| crate::BrowserDriverError::Page(e.to_string()))
    }

    pub async fn text(&self) -> BrowserDriverResult<String> {
        let value = self
            .eval_json(r#"document.body ? document.body.innerText || "" : "" "#)
            .await?;
        Ok(value.as_str().unwrap_or("").to_string())
    }

    pub async fn eval_json(&self, script: &str) -> BrowserDriverResult<serde_json::Value> {
        let page = self.inner.as_ref().ok_or_else(|| {
            crate::BrowserDriverError::Internal("BrowserPage not initialized".into())
        })?;

        // FIX: correct chromiumoxide evaluate API
        let result = page
            .evaluate(script)
            .await
            .map_err(|e| crate::BrowserDriverError::JavaScriptEvaluation(e.to_string()))?;

        // Convert EvaluationResult → serde_json::Value
        let value: serde_json::Value = result
            .into_value()
            .map_err(|e| crate::BrowserDriverError::JavaScriptEvaluation(e.to_string()))?;

        Ok(value)
    }

    pub async fn wait_until<C>(
        &self,
        condition: C,
        options: WaitOptions,
    ) -> BrowserDriverResult<()>
    where
        C: WaitCondition,
    {
        crate::wait::wait_until(self, condition, options).await
    }

    pub async fn close(self) -> BrowserDriverResult<()> {
        if let Some(page) = self.inner {
            let _ = page.close().await;
        }
        Ok(())
    }
}