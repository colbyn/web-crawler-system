//! Browser-based page extraction helpers.
//!
//! Extraction helpers should return browser-observed page facts. They should
//! not decide crawl policy.
//!
//! Good extraction output:
//!
//! - page title,
//! - meta description,
//! - canonical URL,
//! - headings,
//! - anchors,
//! - forms,
//! - JSON-LD,
//! - visible text.
//!
//! Bad extraction responsibility:
//!
//! - deciding whether a URL is in scope,
//! - associating a page with a business entity,
//! - deciding whether a link should be crawled,
//! - writing app-specific records.

pub mod anchors;
pub mod page_info;

pub use anchors::{
    AnchorExtractor,
    ExtractedAnchor,
};

pub use page_info::{
    PageInfo,
    PageInfoExtractor,
    Heading,
};

