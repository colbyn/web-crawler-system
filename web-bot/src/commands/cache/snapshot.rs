//! Snapshot export.
//!
//! This module prints or saves the primary cached payload body for one URL.
//! It currently only supports uncompressed payloads because the existing cache
//! model stores compression state but does not expose a decompression helper at
//! the CLI boundary.

use std::path::PathBuf;

use web_crawler_db::CachePayloadCompression;

use super::common::{
    key_for_url,
    primary_payload,
    CacheHandle,
};

pub(crate) async fn get_snapshot(
    cache: &CacheHandle,
    url: &str,
    namespace: Option<String>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;

    let entry = cache
        .get(&cache_key)
        .await
        .ok_or_else(|| anyhow::anyhow!("URL not found in cache: {}", url))?;

    let payload = primary_payload(&entry);

    if payload.descriptor.compression != CachePayloadCompression::None {
        anyhow::bail!("unsupported compression: {:?}", payload.descriptor.compression);
    }

    let html = String::from_utf8_lossy(&payload.body).to_string();

    if let Some(path) = output {
        std::fs::write(&path, &html)?;
        eprintln!("Snapshot written to {}", path.display());
    } else {
        println!("{}", html);
    }

    Ok(())
}

