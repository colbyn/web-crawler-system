//! Browser automation boundary for crawler systems.
//!
//! This crate is intentionally *not* a crawler.
//!
//! It owns:
//!
//! - launching and shutting down browser processes,
//! - managing browser profile directories,
//! - opening pages,
//! - waiting for pages to become scrape-ready,
//! - extracting browser-observed page data,
//! - preserving URL resolution facts such as redirects and final URLs.
//!
//! It does **not** own:
//!
//! - crawl frontier scheduling,
//! - seed groups,
//! - business entity association,
//! - max page limits,
//! - deduplication policy,
//! - persistent crawl databases,
//! - app-specific metadata.
//!
//! The crawler engine should treat this crate as a browser witness:
//!
//! > "Given this URL and this browser profile, what did the browser observe?"
//!
//! A critical design requirement is that runtime URL resolution must not erase
//! upstream provenance. For example, a caller may request:
//!
//! ```text
//! http://example.com
//! ```
//!
//! and the browser may resolve to:
//!
//! ```text
//! https://www.example.com/
//! ```
//!
//! This crate should report both. It should never decide that the final URL
//! replaces the caller's original URL. That mapping belongs to the crawler/app
//! layer, which may associate many seeds or business entities with the same
//! resolved document.

pub mod config;
pub mod driver;
pub mod error;
pub mod extract;
pub mod health;
pub mod open;
pub mod page;
pub mod profile;
pub mod resolution;
pub mod session;
pub mod telemetry;
pub mod wait;

pub use config::{
    BrowserDriverConfig,
    BrowserLaunchConfig,
    HeadlessMode,
};

pub use driver::{
    BrowserDriver,
};

pub use error::{
    BrowserDriverError,
    BrowserDriverResult,
    NonCriticalBrowserError,
    NonCriticalBrowserErrorKind,
};

pub use extract::{
    AnchorExtractor,
    ExtractedAnchor,
    Heading,
    PageInfo,
    PageInfoExtractor,
};

pub use health::{
    BrowserSessionHealth,
};

pub use open::{
    OpenPageOptions,
    OpenedPage,
};

pub use page::{
    BrowserPage,
};

pub use profile::{
    BrowserProfile,
    BrowserProfileKind,
    BrowserProfileKey,
};

pub use resolution::{
    RedirectHop,
    UrlResolution,
};

pub use session::{
    BrowserSession,
};

pub use telemetry::{
    BrowserHealthTelemetry,
    DocumentReadyState,
    NavigationTelemetry,
    NetworkTelemetry,
    PageTelemetry,
    PageTelemetryBuilder,
    PageTelemetryEvent,
    PageTelemetryEventKind,
    ReadinessTelemetry,
};

pub use wait::{
    LoadStrategy,
    WaitCondition,
    WaitOptions,
};


