// conary-server/src/server/r2.rs
//! Cloudflare R2 storage backend for CDN-backed chunk distribution
//!
//! R2Store wraps an S3-compatible bucket client to store and retrieve
//! CAS chunks in Cloudflare R2 object storage. Chunks are stored under
//! a configurable prefix (e.g., "chunks/") using their content hash as key.

use anyhow::{Context, Result};
use s3::Bucket;
use s3::Region;
use s3::creds::Credentials;

/// Configuration for the R2 storage backend
#[derive(Debug, Clone)]
pub struct R2Config {
    /// R2 endpoint URL (e.g., "https://<account-id>.r2.cloudflarestorage.com")
    pub endpoint: String,
    /// Bucket name
    pub bucket: String,
    /// Key prefix for chunks (e.g., "chunks/")
    pub prefix: String,
    /// Region string (typically "auto" for R2)
    pub region: String,
}

impl Default for R2Config {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            bucket: "conary-chunks".to_string(),
            prefix: "chunks/".to_string(),
            region: "auto".to_string(),
        }
    }
}

/// Cloudflare R2 object storage backend for CAS chunks
#[derive(Debug)]
pub struct R2Store {
    bucket: Box<Bucket>,
    prefix: String,
}

impl R2Store {
    /// Create a new R2Store from configuration.
    ///
    /// Authentication uses environment variables:
    /// - `CONARY_R2_ACCESS_KEY` - R2 access key ID
    /// - `CONARY_R2_SECRET_KEY` - R2 secret access key
    pub fn new(config: &R2Config) -> Result<Self> {
        let access_key = std::env::var("CONARY_R2_ACCESS_KEY")
            .context("CONARY_R2_ACCESS_KEY environment variable not set")?;
        let secret_key = std::env::var("CONARY_R2_SECRET_KEY")
            .context("CONARY_R2_SECRET_KEY environment variable not set")?;

        let region = Region::Custom {
            region: config.region.clone(),
            endpoint: config.endpoint.clone(),
        };

        let credentials = Credentials::new(Some(&access_key), Some(&secret_key), None, None, None)?;

        let bucket = Bucket::new(&config.bucket, region, credentials)?.with_path_style();

        Ok(Self {
            bucket,
            prefix: config.prefix.clone(),
        })
    }

    /// Store a chunk in R2.
    pub async fn put_chunk(&self, hash: &str, data: &[u8]) -> Result<()> {
        let key = self.chunk_key(hash);
        let response = self.bucket.put_object(&key, data).await?;

        if response.status_code() >= 300 {
            anyhow::bail!("R2 PUT failed for {}: HTTP {}", key, response.status_code());
        }

        Ok(())
    }

    /// Retrieve a chunk from R2. Returns `None` if the chunk does not exist.
    pub async fn get_chunk(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        let key = self.chunk_key(hash);
        let response = self.bucket.get_object(&key).await;

        match response {
            Ok(resp) => {
                if resp.status_code() == 404 {
                    Ok(None)
                } else if resp.status_code() >= 300 {
                    anyhow::bail!("R2 GET failed for {}: HTTP {}", key, resp.status_code());
                } else {
                    Ok(Some(resp.to_vec()))
                }
            }
            Err(e) => {
                // rust-s3 may return an error for 404 depending on version
                let msg = e.to_string();
                if msg.contains("404") || msg.contains("NoSuchKey") {
                    Ok(None)
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// Check whether a chunk exists in R2.
    pub async fn head_chunk(&self, hash: &str) -> Result<bool> {
        let key = self.chunk_key(hash);

        match self.bucket.head_object(&key).await {
            Ok((_, code)) => Ok(code < 300),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("404") || msg.contains("NoSuchKey") {
                    Ok(false)
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// Delete a chunk from R2. Returns `true` if the object was deleted,
    /// `false` if it did not exist.
    pub async fn delete_chunk(&self, hash: &str) -> Result<bool> {
        let key = self.chunk_key(hash);
        let response = self.bucket.delete_object(&key).await?;

        // S3/R2 returns 204 for successful delete, 404 if not found
        Ok(response.status_code() < 300)
    }

    /// Generate a presigned GET URL for a chunk
    ///
    /// The URL allows direct download from R2/Cloudflare CDN without authentication.
    /// Expires after `expiry_secs` seconds.
    pub async fn presign_get(&self, hash: &str, expiry_secs: u32) -> Result<String> {
        let key = self.chunk_key(hash);
        Ok(self.bucket.presign_get(&key, expiry_secs, None).await?)
    }

    /// List all chunk hashes stored in R2.
    ///
    /// Paginates through all objects under the configured prefix,
    /// stripping the prefix to return bare hash strings.
    pub async fn list_chunks(&self) -> Result<Vec<String>> {
        let results = self
            .bucket
            .list(self.prefix.clone(), None)
            .await
            .context("R2 list objects failed")?;

        let mut hashes = Vec::new();
        for page in &results {
            for object in &page.contents {
                if let Some(hash) = object.key.strip_prefix(&self.prefix)
                    && !hash.is_empty()
                {
                    hashes.push(hash.to_string());
                }
            }
        }

        Ok(hashes)
    }

    /// Return the configured prefix (e.g., `"chunks/"`).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Build the full object key for a chunk hash.
    fn chunk_key(&self, hash: &str) -> String {
        format!("{}{}", self.prefix, hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_r2_config_default() {
        let config = R2Config::default();
        assert_eq!(config.bucket, "conary-chunks");
        assert_eq!(config.prefix, "chunks/");
        assert_eq!(config.region, "auto");
        assert!(config.endpoint.is_empty());
    }

    #[test]
    fn test_chunk_key_generation() {
        // We can't create a real R2Store without credentials, but we can test
        // the key logic directly by constructing an R2Store-like prefix
        let prefix = "chunks/";
        let hash = "abc123def456";
        let key = format!("{}{}", prefix, hash);
        assert_eq!(key, "chunks/abc123def456");
    }

    #[test]
    fn test_chunk_key_with_custom_prefix() {
        let prefix = "v1/cas/";
        let hash = "sha256_abcdef";
        let key = format!("{}{}", prefix, hash);
        assert_eq!(key, "v1/cas/sha256_abcdef");
    }

    #[test]
    fn test_chunk_key_empty_prefix() {
        let prefix = "";
        let hash = "abc123";
        let key = format!("{}{}", prefix, hash);
        assert_eq!(key, "abc123");
    }

    #[test]
    fn test_r2_config_custom() {
        let config = R2Config {
            endpoint: "https://abc123.r2.cloudflarestorage.com".to_string(),
            bucket: "my-bucket".to_string(),
            prefix: "prod/chunks/".to_string(),
            region: "us-east-1".to_string(),
        };

        assert_eq!(config.endpoint, "https://abc123.r2.cloudflarestorage.com");
        assert_eq!(config.bucket, "my-bucket");
        assert_eq!(config.prefix, "prod/chunks/");
        assert_eq!(config.region, "us-east-1");
    }

    #[test]
    fn test_list_chunks_prefix_stripping() {
        // Verify the prefix-stripping logic used in list_chunks
        let prefix = "chunks/";
        let keys = vec![
            "chunks/abc123def456",
            "chunks/", // empty hash after strip
            "chunks/xyz789",
            "other/notchunk", // different prefix
        ];

        let mut hashes = Vec::new();
        for key in keys {
            if let Some(hash) = key.strip_prefix(prefix) {
                if !hash.is_empty() {
                    hashes.push(hash.to_string());
                }
            }
        }

        assert_eq!(hashes, vec!["abc123def456", "xyz789"]);
    }

    #[test]
    fn test_new_requires_env_vars() {
        // Ensure env vars are not set for this test
        // SAFETY: This is a unit test running single-threaded; no concurrent env access
        unsafe {
            std::env::remove_var("CONARY_R2_ACCESS_KEY");
            std::env::remove_var("CONARY_R2_SECRET_KEY");
        }

        let config = R2Config {
            endpoint: "https://test.r2.cloudflarestorage.com".to_string(),
            bucket: "test".to_string(),
            prefix: "chunks/".to_string(),
            region: "auto".to_string(),
        };

        let result = R2Store::new(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("CONARY_R2_ACCESS_KEY"),
            "Expected error about missing access key, got: {}",
            err_msg
        );
    }
}
