//! Cache artifact binary codec.
//!
//! The cache format is intentionally hidden behind this module. The engine and
//! CLI should not depend directly on a particular binary serialization crate.
//!
//! This makes it possible to start with a simple Rust-native codec and later
//! switch to another format without contaminating crawler code.
//!
//! The first implementation uses `bincode` because the cache is internal,
//! compact, and fast. If cross-language inspection becomes important later,
//! this module can grow alternate codecs such as MessagePack or CBOR.

use crate::cache::{
    CachedPageArtifact,
    CrawlCacheError,
};

pub trait CacheCodec {
    fn encode(artifact: &CachedPageArtifact) -> Result<Vec<u8>, CrawlCacheError>;

    fn decode(bytes: &[u8]) -> Result<CachedPageArtifact, CrawlCacheError>;
}

#[derive(Debug, Clone, Copy)]
pub struct BincodeCacheCodec;

impl CacheCodec for BincodeCacheCodec {
    fn encode(artifact: &CachedPageArtifact) -> Result<Vec<u8>, CrawlCacheError> {
        bincode::serde::encode_to_vec(
            artifact,
            bincode::config::standard(),
        )
        .map_err(|e| CrawlCacheError::Encode(e.to_string()))
    }

    fn decode(bytes: &[u8]) -> Result<CachedPageArtifact, CrawlCacheError> {
        let (artifact, _bytes_read): (CachedPageArtifact, usize) =
            bincode::serde::decode_from_slice(
                bytes,
                bincode::config::standard(),
            )
            .map_err(|e| CrawlCacheError::Decode(e.to_string()))?;

        Ok(artifact)
    }
}

