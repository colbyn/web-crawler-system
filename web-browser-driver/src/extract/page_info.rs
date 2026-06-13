//! Page-level metadata extraction.
//!
//! This is intentionally generic. The caller can interpret the facts however it
//! wants.
//!
//! Page canonical URL is included here because it is content-declared metadata,
//! not necessarily part of transport-level redirect resolution.
//!
//! Page info is generic browser-observed evidence and is intended to be
//! serializable for cache artifacts and downstream inspection tools.

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;
use schemars::JsonSchema;

use crate::{
    BrowserDriverResult,
    BrowserPage,
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct PageInfo {
    pub title: Option<String>,
    pub description: Option<String>,
    pub canonical_url: Option<Url>,
    pub lang: Option<String>,
    pub headings: Vec<Heading>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct Heading {
    pub level: u8,
    pub text: String,
}

pub struct PageInfoExtractor;

impl PageInfoExtractor {
    pub async fn extract(page: &BrowserPage) -> BrowserDriverResult<PageInfo> {
        let value = page
            .eval_json(
                r#"
                (() => {
                    const text = (node) => node && node.textContent
                        ? node.textContent.trim().replace(/\s+/g, " ")
                        : null;

                    const attr = (selector, name) => {
                        const el = document.querySelector(selector);
                        return el ? el.getAttribute(name) : null;
                    };

                    const headings = Array.from(
                        document.querySelectorAll("h1,h2,h3,h4,h5,h6")
                    ).map((el) => ({
                        level: Number(el.tagName.slice(1)),
                        text: text(el)
                    })).filter((h) => h.text);

                    return {
                        title: document.title || null,
                        description: attr('meta[name="description"]', "content"),
                        canonicalUrl: attr('link[rel="canonical"]', "href"),
                        lang: document.documentElement
                            ? document.documentElement.getAttribute("lang")
                            : null,
                        headings
                    };
                })()
                "#,
            )
            .await?;

        let title = value
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::to_owned);

        let description = value
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::to_owned);

        let canonical_url = value
            .get("canonicalUrl")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok());

        let lang = value
            .get("lang")
            .and_then(|v| v.as_str())
            .map(str::to_owned);

        let headings = value
            .get("headings")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let level = item.get("level")?.as_u64()? as u8;
                        let text = item.get("text")?.as_str()?.to_owned();

                        Some(Heading { level, text })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(PageInfo {
            title,
            description,
            canonical_url,
            lang,
            headings,
        })
    }
}

