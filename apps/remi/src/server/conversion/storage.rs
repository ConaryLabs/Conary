// apps/remi/src/server/conversion/storage.rs
//! CAS and optional R2 write-through storage for converted blobs.

use super::ConversionService;
use anyhow::{Context, Result};
use conary_core::ccs::convert::ConversionResult;
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::debug;

pub(super) struct StoredChunks {
    pub(super) chunk_hashes: Vec<String>,
    pub(super) cas_duration: Duration,
    pub(super) r2_duration: Option<Duration>,
}

impl ConversionService {
    /// Store blobs from conversion result in CAS
    #[cfg(test)]
    async fn store_chunks(&self, result: &ConversionResult) -> Result<Vec<String>> {
        Ok(self.store_chunks_with_timing(result).await?.chunk_hashes)
    }

    pub(super) async fn store_chunks_with_timing(
        &self,
        result: &ConversionResult,
    ) -> Result<StoredChunks> {
        let mut chunk_hashes = Vec::new();
        let mut cas_duration = Duration::default();
        let mut r2_duration = self.r2_store.as_ref().map(|_| Duration::default());
        let objects_dir = self.chunk_dir.join("objects");

        // Get blobs from the build result (chunks or whole files)
        for (hash, data) in &result.build_result.blobs {
            let cas_started = Instant::now();
            let (prefix, rest) = hash.split_at(2.min(hash.len()));
            let chunk_path = objects_dir.join(prefix).join(rest);

            // Skip if chunk already exists (content-addressed = immutable)
            if chunk_path.exists() {
                cas_duration += cas_started.elapsed();
                debug!("Chunk {} already exists", hash);
                chunk_hashes.push(hash.clone());
                continue;
            }

            // Create parent directory
            if let Some(parent) = chunk_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .context("Failed to create chunk directory")?;
            }

            // Write chunk atomically
            let temp_path = chunk_path.with_extension("tmp");
            tokio::fs::write(&temp_path, data)
                .await
                .context("Failed to write chunk")?;
            tokio::fs::rename(&temp_path, &chunk_path)
                .await
                .context("Failed to rename chunk")?;
            cas_duration += cas_started.elapsed();

            // R2 write-through: upload to Cloudflare R2 in parallel
            if let Some(ref r2) = self.r2_store {
                let r2_started = Instant::now();
                if let Err(e) = r2.put_chunk(hash, data).await {
                    tracing::warn!("R2 write-through failed for chunk {}: {}", hash, e);
                } else {
                    debug!("R2 write-through: uploaded chunk {}", hash);
                }
                if let Some(total) = &mut r2_duration {
                    *total += r2_started.elapsed();
                }
            }

            debug!("Stored chunk: {} ({} bytes)", hash, data.len());
            chunk_hashes.push(hash.clone());
        }

        Ok(StoredChunks {
            chunk_hashes,
            cas_duration,
            r2_duration,
        })
    }

    /// Calculate SHA-256 checksum of a file
    pub(super) fn calculate_checksum(path: &Path) -> Result<String> {
        let mut file = std::fs::File::open(path)?;
        Ok(conary_core::hash::sha256_reader_hex(&mut file)?)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::make_conversion_result;
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_calculate_checksum_valid_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.pkg");
        std::fs::write(&file_path, b"hello world").unwrap();

        let checksum = ConversionService::calculate_checksum(&file_path).unwrap();
        // SHA-256 of "hello world"
        assert_eq!(
            checksum,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_calculate_checksum_empty_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("empty.pkg");
        std::fs::write(&file_path, b"").unwrap();

        let checksum = ConversionService::calculate_checksum(&file_path).unwrap();
        // SHA-256 of empty string
        assert_eq!(
            checksum,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_calculate_checksum_missing_file() {
        let result = ConversionService::calculate_checksum(Path::new("/nonexistent/file.pkg"));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_store_chunks_writes_files() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();

        let service = ConversionService::new(
            chunk_dir.clone(),
            temp_dir.path().to_path_buf(),
            PathBuf::from("/tmp/nonexistent.db"),
            None,
        );

        let mut blobs = std::collections::HashMap::new();
        blobs.insert("abcdef1234567890".to_string(), b"chunk data one".to_vec());
        blobs.insert("1234567890abcdef".to_string(), b"chunk data two".to_vec());

        let result = make_conversion_result(blobs);
        let hashes = service.store_chunks(&result).await.unwrap();
        assert_eq!(hashes.len(), 2);

        // Verify files were written to correct paths
        for hash in &hashes {
            let (prefix, rest) = hash.split_at(2);
            let chunk_path = chunk_dir.join("objects").join(prefix).join(rest);
            assert!(
                chunk_path.exists(),
                "Chunk file should exist at {:?}",
                chunk_path
            );
        }
    }

    #[tokio::test]
    async fn test_store_chunks_idempotent() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();

        let service = ConversionService::new(
            chunk_dir.clone(),
            temp_dir.path().to_path_buf(),
            PathBuf::from("/tmp/nonexistent.db"),
            None,
        );

        let mut blobs = std::collections::HashMap::new();
        blobs.insert("aabbccdd11223344".to_string(), b"some data".to_vec());

        let result = make_conversion_result(blobs.clone());

        // Store twice - should not error
        let hashes1 = service.store_chunks(&result).await.unwrap();

        let result2 = make_conversion_result(blobs);
        let hashes2 = service.store_chunks(&result2).await.unwrap();
        assert_eq!(hashes1, hashes2);
    }

    #[tokio::test]
    async fn test_store_chunks_empty() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();

        let service = ConversionService::new(
            chunk_dir,
            temp_dir.path().to_path_buf(),
            PathBuf::from("/tmp/nonexistent.db"),
            None,
        );

        let result = make_conversion_result(std::collections::HashMap::new());
        let hashes = service.store_chunks(&result).await.unwrap();
        assert!(hashes.is_empty());
    }
}
