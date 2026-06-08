//! Time utilities for the cache.
//!
//! Provides `now_unix_ms()`, a simple helper that returns the current time
//! as milliseconds since the Unix epoch. This is used for `stored_at_unix_ms`
//! timestamps and any future age-based logic.
//!
//! The function is isolated in its own module so it can be easily replaced
//! with a deterministic version during testing if needed.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
