// apps/remi/src/server/r2.rs
//! Cloudflare R2 storage backend for CDN-backed chunk distribution
//!
//! R2Store wraps an S3-compatible bucket client to store and retrieve
//! CAS chunks in Cloudflare R2 object storage. Chunks are stored under
//! a configurable prefix (e.g., "chunks/") using their content hash as key.

use anyhow::{Context, Result};
use s3::Bucket;
use s3::Region;
use s3::creds::Credentials;
use s3::serde_types::{DeleteObjectsResult, ObjectIdentifier};
use std::collections::BTreeSet;

/// Maximum keys S3/R2 accepts in one `DeleteObjects` request.
const R2_DELETE_BATCH_SIZE: usize = 1000;

/// Maximum (hash, error) pairs retained from a bulk delete for diagnostics.
const R2_DELETE_FAILURE_SAMPLES: usize = 10;

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

/// Aggregate outcome of a bulk chunk delete.
///
/// A per-key rejection is recorded rather than propagated so one bad key
/// cannot abandon the rest of a garbage-collection run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct R2BatchDeleteOutcome {
    /// Keys submitted for deletion
    pub attempted: usize,
    /// Keys R2 removed (or that were already absent)
    pub deleted: usize,
    /// Keys R2 refused, plus keys in a batch whose request failed outright.
    ///
    /// Held in full rather than counted: the caller subtracts these from what it
    /// submitted to decide which chunks are actually gone, and a count cannot
    /// answer that. Size is bounded by the submitted key set the caller already
    /// holds.
    pub failed_hashes: BTreeSet<String>,
    /// Up to `R2_DELETE_FAILURE_SAMPLES` (hash, error) pairs for diagnostics
    pub failure_samples: Vec<(String, String)>,
}

/// Exact object metadata returned by an R2 prefix inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct R2ChunkObject {
    pub hash: String,
    pub size_bytes: u64,
}

impl R2BatchDeleteOutcome {
    /// Number of keys that were not removed.
    pub fn failed(&self) -> usize {
        self.failed_hashes.len()
    }

    fn record_failure(&mut self, hash: &str, error: &str) {
        if !self.failed_hashes.insert(hash.to_string()) {
            return;
        }
        if self.failure_samples.len() < R2_DELETE_FAILURE_SAMPLES {
            self.failure_samples
                .push((hash.to_string(), error.to_string()));
        }
    }

    /// Fold a batch whose request succeeded.
    ///
    /// S3 reports only the keys it refused; deletion is idempotent, so every
    /// other key in the batch is gone whether or not it existed.
    fn fold_batch(&mut self, batch: &[String], failures: &[(String, String)]) {
        for (hash, error) in failures {
            self.record_failure(hash, error);
        }
        self.deleted += batch.len().saturating_sub(failures.len());
    }

    /// Fold a batch whose request itself failed: none of its keys were removed.
    fn fold_batch_request_failure(&mut self, batch: &[String], error: &str) {
        for hash in batch {
            self.record_failure(hash, error);
        }
    }
}

/// Split chunk hashes into request-sized batches.
fn delete_batches(hashes: &[String]) -> std::slice::Chunks<'_, String> {
    hashes.chunks(R2_DELETE_BATCH_SIZE)
}

/// Convert a bulk-delete response's per-key errors into (hash, error) pairs.
fn delete_failures(prefix: &str, response: &DeleteObjectsResult) -> Vec<(String, String)> {
    response
        .errors
        .iter()
        .map(|err| {
            let hash = err.key.strip_prefix(prefix).unwrap_or(&err.key).to_string();
            (hash, format!("{}: {}", err.code, err.message))
        })
        .collect()
}

fn classify_head_status(key: &str, status: u16) -> Result<bool> {
    match status {
        200..=299 => Ok(true),
        404 => Ok(false),
        _ => anyhow::bail!("R2 HEAD failed for {key}: HTTP {status}"),
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
            Ok((_, code)) => classify_head_status(&key, code),
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

    /// Delete many chunks from R2 using the S3 multi-object delete API.
    ///
    /// Keys are sent in batches of at most `R2_DELETE_BATCH_SIZE`. Neither a
    /// per-key rejection nor a failed batch request aborts the remaining
    /// batches; both are reported through the returned outcome.
    pub async fn delete_chunks(&self, hashes: &[String]) -> Result<R2BatchDeleteOutcome> {
        let mut outcome = R2BatchDeleteOutcome {
            attempted: hashes.len(),
            ..Default::default()
        };

        for batch in delete_batches(hashes) {
            let objects: Vec<ObjectIdentifier> = batch
                .iter()
                .map(|hash| ObjectIdentifier::new(self.chunk_key(hash)))
                .collect();

            match self.bucket.delete_objects(objects).await {
                Ok(response) => {
                    let failures = delete_failures(&self.prefix, &response);
                    outcome.fold_batch(batch, &failures);
                }
                Err(e) => {
                    outcome.fold_batch_request_failure(batch, &e.to_string());
                }
            }
        }

        Ok(outcome)
    }

    /// Generate a presigned GET URL for a chunk
    ///
    /// The URL allows direct download from R2/Cloudflare CDN without authentication.
    /// Expires after `expiry_secs` seconds.
    pub async fn presign_get(&self, hash: &str, expiry_secs: u32) -> Result<String> {
        let key = self.chunk_key(hash);
        Ok(self.bucket.presign_get(&key, expiry_secs, None).await?)
    }

    /// List all chunk objects stored in R2 with their exact stored sizes.
    ///
    /// Paginates through all objects under the configured prefix,
    /// stripping the prefix to return bare hash identities.
    pub async fn list_chunk_objects(&self) -> Result<Vec<R2ChunkObject>> {
        let results = self
            .bucket
            .list(self.prefix.clone(), None)
            .await
            .context("R2 list objects failed")?;

        let mut objects = Vec::new();
        for page in &results {
            for object in &page.contents {
                if let Some(hash) = object.key.strip_prefix(&self.prefix)
                    && !hash.is_empty()
                {
                    objects.push(R2ChunkObject {
                        hash: hash.to_string(),
                        size_bytes: object.size,
                    });
                }
            }
        }

        objects.sort_by(|left, right| left.hash.cmp(&right.hash));
        Ok(objects)
    }

    /// List all chunk hashes stored in R2.
    pub async fn list_chunks(&self) -> Result<Vec<String>> {
        Ok(self
            .list_chunk_objects()
            .await?
            .into_iter()
            .map(|object| object.hash)
            .collect())
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
    fn head_status_distinguishes_absence_from_authority_failure() {
        assert!(classify_head_status("chunks/hash", 200).unwrap());
        assert!(!classify_head_status("chunks/hash", 404).unwrap());
        for status in [401, 403, 429, 500, 503] {
            let error = classify_head_status("chunks/hash", status)
                .unwrap_err()
                .to_string();
            assert!(error.contains(&status.to_string()), "{error}");
        }
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
            if let Some(hash) = key.strip_prefix(prefix)
                && !hash.is_empty()
            {
                hashes.push(hash.to_string());
            }
        }

        assert_eq!(hashes, vec!["abc123def456", "xyz789"]);
    }

    fn hashes(range: std::ops::Range<usize>) -> Vec<String> {
        range.map(|i| format!("hash-{i:06}")).collect()
    }

    #[test]
    fn delete_batches_never_exceed_the_s3_request_limit() {
        let keys = hashes(0..2500);
        let batches: Vec<&[String]> = delete_batches(&keys).collect();

        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), R2_DELETE_BATCH_SIZE);
        assert_eq!(batches[1].len(), R2_DELETE_BATCH_SIZE);
        // Final partial batch carries the remainder.
        assert_eq!(batches[2].len(), 500);
        assert_eq!(batches[0][0], "hash-000000");
        assert_eq!(batches[2][499], "hash-002499");

        let total: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total, keys.len());
    }

    #[test]
    fn delete_batches_are_empty_for_no_keys() {
        assert_eq!(delete_batches(&[]).count(), 0);
    }

    #[test]
    fn delete_batches_send_an_exact_multiple_without_a_trailing_batch() {
        let keys = hashes(0..R2_DELETE_BATCH_SIZE * 2);
        let batches: Vec<&[String]> = delete_batches(&keys).collect();
        assert_eq!(batches.len(), 2);
        assert!(batches.iter().all(|b| b.len() == R2_DELETE_BATCH_SIZE));
    }

    fn delete_error(key: &str, code: &str) -> s3::serde_types::DeleteError {
        s3::serde_types::DeleteError {
            key: key.to_string(),
            code: code.to_string(),
            message: "denied".to_string(),
            version_id: None,
        }
    }

    #[test]
    fn delete_failures_strip_the_chunk_prefix() {
        let response = DeleteObjectsResult {
            deleted: Vec::new(),
            errors: vec![
                delete_error("chunks/abc123", "AccessDenied"),
                // A key outside the prefix is reported verbatim.
                delete_error("other/def456", "InternalError"),
            ],
        };

        let failures = delete_failures("chunks/", &response);
        assert_eq!(
            failures,
            vec![
                ("abc123".to_string(), "AccessDenied: denied".to_string()),
                (
                    "other/def456".to_string(),
                    "InternalError: denied".to_string()
                ),
            ]
        );
    }

    #[test]
    fn outcome_counts_unreported_keys_in_a_batch_as_deleted() {
        let batch = hashes(0..5);
        let mut outcome = R2BatchDeleteOutcome {
            attempted: batch.len(),
            ..Default::default()
        };

        outcome.fold_batch(
            &batch,
            &[(
                "hash-000002".to_string(),
                "AccessDenied: denied".to_string(),
            )],
        );

        assert_eq!(outcome.attempted, 5);
        assert_eq!(outcome.deleted, 4);
        assert_eq!(outcome.failed(), 1);
        assert_eq!(
            outcome.failure_samples,
            vec![(
                "hash-000002".to_string(),
                "AccessDenied: denied".to_string()
            )]
        );
    }

    #[test]
    fn outcome_accumulates_across_batches() {
        let first = hashes(0..3);
        let second = hashes(3..6);
        let mut outcome = R2BatchDeleteOutcome {
            attempted: 6,
            ..Default::default()
        };

        outcome.fold_batch(&first, &[]);
        // A failed request loses its whole batch but not the earlier one.
        outcome.fold_batch_request_failure(&second, "connection reset");

        assert_eq!(outcome.deleted, 3);
        assert_eq!(outcome.failed(), 3);
        assert_eq!(outcome.failure_samples.len(), 3);
        assert!(
            outcome
                .failure_samples
                .iter()
                .all(|(_, error)| error == "connection reset")
        );
    }

    #[test]
    fn outcome_caps_failure_samples_but_keeps_counting() {
        let batch = hashes(0..250);
        let mut outcome = R2BatchDeleteOutcome {
            attempted: batch.len(),
            ..Default::default()
        };

        outcome.fold_batch_request_failure(&batch, "timeout");

        assert_eq!(outcome.deleted, 0);
        assert_eq!(outcome.failed(), 250);
        assert_eq!(outcome.failure_samples.len(), R2_DELETE_FAILURE_SAMPLES);
        // Samples are the first failures seen, not a random subset.
        assert_eq!(outcome.failure_samples[0].0, "hash-000000");
        assert_eq!(outcome.failure_samples[9].0, "hash-000009");
    }

    #[test]
    fn outcome_does_not_underflow_when_a_server_over_reports_errors() {
        let batch = hashes(0..2);
        let mut outcome = R2BatchDeleteOutcome {
            attempted: batch.len(),
            ..Default::default()
        };

        outcome.fold_batch(
            &batch,
            &[
                ("hash-000000".to_string(), "e".to_string()),
                ("hash-000001".to_string(), "e".to_string()),
                ("hash-000099".to_string(), "e".to_string()),
            ],
        );

        assert_eq!(outcome.deleted, 0);
        assert_eq!(outcome.failed(), 3);
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
