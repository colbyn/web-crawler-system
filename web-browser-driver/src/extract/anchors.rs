//! Anchor extraction.
//!
//! This extractor reports what the browser sees in the DOM. It should preserve
//! both raw href facts and resolved href facts where possible.
//!
//! The crawler engine can later decide:
//!
//! - whether to visit the link,
//! - how to normalize it,
//! - whether it is in scope,
//! - how to score it,
//! - which seed lineage discovered it.
//!
//! Extracted anchors are generic browser-observed facts and are intended to be
//! serializable for cache artifacts and downstream inspection tools.

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;

use crate::{
    BrowserDriverResult,
    BrowserPage,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExtractedAnchor {
    /// Position in document order.
    pub index: usize,

    /// Raw `href` attribute as written in the DOM.
    pub raw_href: Option<String>,

    /// Browser-resolved absolute href from `HTMLAnchorElement.href`.
    pub href: Option<Url>,

    /// Visible anchor text.
    pub text: Option<String>,

    /// ARIA label, title, or other small label hints.
    pub label: Option<String>,

    /// Nearby context, when cheaply available.
    pub nearby_text: Option<String>,
}

pub struct AnchorExtractor;

impl AnchorExtractor {
    pub async fn extract(page: &BrowserPage) -> BrowserDriverResult<Vec<ExtractedAnchor>> {
        let value = page
            .eval_json(
                r#"
                (() => {
                    const clean = (s) => {
                        if (!s) return null;
                        const out = String(s).trim().replace(/\s+/g, " ");
                        return out.length ? out : null;
                    };

                    const nearbyText = (a) => {
                        let node = a;
                        for (let i = 0; i < 3 && node; i++) {
                            node = node.parentElement;
                            if (!node) break;

                            const txt = clean(node.innerText || node.textContent);
                            if (txt && txt.length <= 500) return txt;
                        }
                        return null;
                    };

                    return Array.from(document.querySelectorAll("a")).map((a, index) => ({
                        index,
                        rawHref: a.getAttribute("href"),
                        href: a.href || null,
                        text: clean(a.innerText || a.textContent),
                        label: clean(
                            a.getAttribute("aria-label")
                            || a.getAttribute("title")
                            || a.getAttribute("alt")
                        ),
                        nearbyText: nearbyText(a)
                    }));
                })()
                "#,
            )
            .await?;

        let mut out = Vec::new();

        let Some(items) = value.as_array() else {
            return Ok(out);
        };

        for item in items {
            let index = item
                .get("index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            let raw_href = item
                .get("rawHref")
                .and_then(|v| v.as_str())
                .map(str::to_owned);

            let href = item
                .get("href")
                .and_then(|v| v.as_str())
                .and_then(|s| Url::parse(s).ok());

            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .map(str::to_owned);

            let label = item
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::to_owned);

            let nearby_text = item
                .get("nearbyText")
                .and_then(|v| v.as_str())
                .map(str::to_owned);

            out.push(ExtractedAnchor {
                index,
                raw_href,
                href,
                text,
                label,
                nearby_text,
            });
        }

        Ok(out)
    }
}

