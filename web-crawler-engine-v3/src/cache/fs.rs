//! Filesystem cache store.
//!
//! This is the default cache implementation. It stores one self-contained
//! binary artifact per cache key under a user-provided root directory.
//!
//! The filesystem layout is hash-sharded. URLs are not used directly as file
//! paths because URL-derived paths become fragile under long paths, Unicode,
//! query strings, case sensitivity, filesystem limits, and future key changes.
//!
//! The store is deliberately simple:
//!
//! - compute stable key digest,
//! - map digest to a sharded path,
//! - load/decode artifact,
//! - encode/write artifact atomically.
//!
//! It does not decide whether artifacts are healthy. That belongs to
//! `CachePolicy`.

use std::path::{
    Path,
    PathBuf,
};

use async_trait::async_trait;
use tokio::fs;

use crate::cache::{
    sha256_hex,
    BincodeCacheCodec,
    CacheCodec,
    CacheKey,
    CachedPageArtifact,
    CrawlCacheError,
    CrawlCacheStore,
};

#[derive(Debug, Clone)]
pub struct FsCrawlCacheStore {
    root: PathBuf,
}

impl FsCrawlCacheStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_for_key(&self, key: &CacheKey) -> Result<PathBuf, CrawlCacheError> {
        let key_bytes = serde_json::to_vec(key)
            .map_err(|e| CrawlCacheError::KeySerialization(e.to_string()))?;

        let digest = sha256_hex(&key_bytes);
        let shard = &digest[..2];

        Ok(self
            .root
            .join("pages")
            .join(shard)
            .join(format!("{digest}.pagebin")))
    }

    fn tmp_path_for_final_path(&self, final_path: &Path) -> PathBuf {
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact.pagebin");

        self.root
            .join("tmp")
            .join(format!("{file_name}.tmp"))
    }
}

#[async_trait]
impl CrawlCacheStore for FsCrawlCacheStore {
    async fn load(
        &self,
        key: &CacheKey,
    ) -> Result<Option<CachedPageArtifact>, CrawlCacheError> {
        let path = self.path_for_key(key)?;

        let bytes = match fs::read(&path).await {
            Ok(bytes) => bytes,

            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }

            Err(err) => {
                return Err(CrawlCacheError::Io(err));
            }
        };

        let artifact = BincodeCacheCodec::decode(&bytes)?;

        Ok(Some(artifact))
    }

    async fn save(
        &self,
        key: &CacheKey,
        artifact: &CachedPageArtifact,
    ) -> Result<(), CrawlCacheError> {
        let final_path = self.path_for_key(key)?;

        let parent = final_path.parent().ok_or_else(|| {
            CrawlCacheError::Internal(format!(
                "cache path has no parent: {}",
                final_path.display()
            ))
        })?;

        fs::create_dir_all(parent).await?;
        fs::create_dir_all(self.root.join("tmp")).await?;

        let tmp_path = self.tmp_path_for_final_path(&final_path);
        let bytes = BincodeCacheCodec::encode(artifact)?;

        fs::write(&tmp_path, bytes).await?;
        fs::rename(&tmp_path, &final_path).await?;

        Ok(())
    }
}
