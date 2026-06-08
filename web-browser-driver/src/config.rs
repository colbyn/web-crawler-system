//! Browser launch and driver configuration.
//! Focused on browser mechanics only.
//!
//! This module should remain focused on browser mechanics. Higher-level crawl
//! concepts such as seed groups, max pages, domain scope, or app metadata belong
//! in `web-crawler-engine-v3`.

use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct BrowserDriverConfig {
    pub executable_path: Option<PathBuf>,
    pub launch: BrowserLaunchConfig,
}

#[derive(Debug, Clone)]
pub struct BrowserLaunchConfig {
    pub headless: HeadlessMode,
    pub args: Vec<String>,
    pub startup_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessMode {
    False,   // visible window
    True,    // classic headless
    New,     // new Chromium headless mode
}

impl Default for HeadlessMode {
    fn default() -> Self {
        Self::New
    }
}

impl Default for BrowserDriverConfig {
    fn default() -> Self {
        Self {
            executable_path: None,
            launch: BrowserLaunchConfig::default(),
        }
    }
}

impl Default for BrowserLaunchConfig {
    fn default() -> Self {
        Self {
            headless: HeadlessMode::default(),
            args: Vec::new(),
            startup_timeout: Duration::from_secs(20),
        }
    }
}
