//! Single-entry cache lookup.
//!
//! This module handles metadata inspection for one cached URL. It is allowed to
//! load the full cache entry because the operator explicitly requested one URL.
//! Bulk list commands should not live here because they should avoid loading
//! payload bodies.

use web_crawler_db::CachePayloadCompression;

use super::common::{
    format_bytes,
    format_status,
    key_for_url,
    primary_payload,
    CacheHandle,
};

pub(crate) async fn lookup_metadata(
    cache: &CacheHandle,
    url: &str,
    namespace: Option<String>,
    json: bool,
    full: bool,
) -> anyhow::Result<()> {
    let cache_key = key_for_url(url, namespace)?;

    match cache.get(&cache_key).await {
        Some(entry) => {
            let primary = primary_payload(&entry);

            let payload_json = serde_json::json!({
                "descriptor": primary.descriptor,
                "body": if full && primary.descriptor.compression == CachePayloadCompression::None {
                    Some(String::from_utf8_lossy(&primary.body).to_string())
                } else {
                    None::<String>
                }
            });

            let value = serde_json::json!({
                "metadata": entry.metadata,
                "payload": payload_json,
                "tags": entry.tags.iter().map(|tag| {
                    serde_json::json!({
                        "kind": tag.kind(),
                        "key": tag.key(),
                        "tag": tag.as_compound(),
                    })
                }).collect::<Vec<_>>(),
            });

            if json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                eprintln!("Cache hit");
                eprintln!("  Requested URL:  {}", entry.metadata.request.requested_url);

                if let Some(final_url) = &entry.metadata.response.final_url {
                    eprintln!("  Final URL:      {}", final_url);
                }

                eprintln!("  Stored at ms:   {}", entry.metadata.stored_at_unix_ms);
                eprintln!("  Status:         {}", format_status(entry.metadata.response.status_code));
                eprintln!(
                    "  Content type:   {}",
                    entry
                        .metadata
                        .response
                        .content_type
                        .as_deref()
                        .unwrap_or("-")
                );
                eprintln!("  Snapshot size:  {}", format_bytes(primary.descriptor.byte_len as i64));
                eprintln!("  SHA-256:        {}", primary.descriptor.sha256_hex);
                eprintln!("  Anchors:        {}", entry.metadata.anchors.len());

                if let Some(title) = entry
                    .metadata
                    .page_info
                    .as_ref()
                    .and_then(|page_info| page_info.title.as_deref())
                {
                    eprintln!("  Title:          {}", title);
                }

                if !entry.tags.is_empty() {
                    eprintln!(
                        "  Tags:           {}",
                        entry
                            .tags
                            .iter()
                            .map(|tag| tag.as_compound())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }
        None => {
            if json {
                println!("null");
            } else {
                eprintln!("Not found in cache: {}", url);
            }
        }
    }

    Ok(())
}

