// apps/remi/src/server/conversion/storage.rs
//! Bounded-memory CAS and optional R2 storage for emitted CCS artifacts.

use super::ConversionService;
use anyhow::{Context, Result};
use conary_core::ccs::chunking::Chunker;
use conary_core::ccs::convert::ConversionResult;
use conary_core::db::models::ChunkAccess;
use conary_core::filesystem::{CasStore, object_path};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::debug;

pub(super) struct StoredChunks {
    pub(super) chunk_hashes: Vec<String>,
    pub(super) chunking_duration: Duration,
    pub(super) cas_duration: Duration,
    pub(super) r2_duration: Option<Duration>,
}

struct LocalStoredChunks {
    ordered_chunks: Vec<(String, i64)>,
    chunking_duration: Duration,
    cas_duration: Duration,
}

fn chunk_and_store_ccs_artifact(
    package_path: &Path,
    objects_dir: &Path,
) -> Result<LocalStoredChunks> {
    let file = File::open(package_path)
        .with_context(|| format!("open emitted CCS artifact {}", package_path.display()))?;
    let expected_size = file
        .metadata()
        .with_context(|| format!("stat emitted CCS artifact {}", package_path.display()))?
        .len();
    if expected_size == 0 {
        anyhow::bail!("emitted CCS artifact is empty");
    }

    let cas = CasStore::new(objects_dir).context("initialize converted-artifact CAS")?;
    let mut ordered_chunks = Vec::new();
    let mut cas_duration = Duration::default();
    let chunking_started = Instant::now();
    let processed = Chunker::new().visit_reader_chunks(file, |chunk| {
        let expected_hash = chunk.hash_hex();
        let cas_started = Instant::now();
        let stored_hash = cas
            .store(&chunk.data)
            .context("store emitted CCS artifact chunk")?;
        cas_duration += cas_started.elapsed();
        if stored_hash != expected_hash {
            anyhow::bail!(
                "CAS hash disagrees with streamed chunk authority: {stored_hash} != {expected_hash}"
            );
        }
        ordered_chunks.push((expected_hash, i64::from(chunk.length)));
        Ok(())
    })?;
    let chunking_duration = chunking_started.elapsed().saturating_sub(cas_duration);

    if processed != expected_size {
        anyhow::bail!(
            "streamed CCS artifact size disagrees with file metadata: {processed} != {expected_size}"
        );
    }
    if ordered_chunks.is_empty() {
        anyhow::bail!("emitted CCS artifact produced no chunks");
    }

    Ok(LocalStoredChunks {
        ordered_chunks,
        chunking_duration,
        cas_duration,
    })
}

impl ConversionService {
    /// Store the emitted CCS artifact as ordered content-defined chunks.
    #[cfg(test)]
    async fn store_chunks(&self, result: &ConversionResult) -> Result<Vec<String>> {
        Ok(self.store_chunks_with_timing(result).await?.chunk_hashes)
    }

    pub(super) async fn store_chunks_with_timing(
        &self,
        result: &ConversionResult,
    ) -> Result<StoredChunks> {
        let package_path = result
            .package_path
            .clone()
            .context("conversion did not emit a CCS artifact")?;
        let objects_dir = self.chunk_dir.join("objects");
        let local_objects_dir = objects_dir.clone();
        let local = tokio::task::spawn_blocking(move || {
            chunk_and_store_ccs_artifact(&package_path, &local_objects_dir)
        })
        .await
        .context("join emitted CCS artifact chunking task")??;

        let mut exact_sizes = BTreeMap::new();
        for (hash, size) in &local.ordered_chunks {
            if let Some(previous) = exact_sizes.insert(hash.clone(), *size)
                && previous != *size
            {
                anyhow::bail!(
                    "identical chunk hash has conflicting exact sizes: {previous} != {size}"
                );
            }
        }

        let r2_duration = if let Some(ref r2) = self.r2_store {
            let mut duration = Duration::default();
            let unique_hashes: BTreeSet<_> = local
                .ordered_chunks
                .iter()
                .map(|(hash, _)| hash.clone())
                .collect();
            for hash in unique_hashes {
                let r2_started = Instant::now();
                let chunk_path = object_path(&objects_dir, &hash)?;
                let data = tokio::fs::read(&chunk_path)
                    .await
                    .with_context(|| format!("read local CAS chunk {hash} for R2 write-through"))?;
                let actual_hash = conary_core::hash::sha256(&data);
                if actual_hash != hash {
                    anyhow::bail!(
                        "local CAS chunk failed R2 integrity check: {actual_hash} != {hash}"
                    );
                }
                if let Err(e) = r2.put_chunk(&hash, &data).await {
                    tracing::warn!("R2 write-through failed for chunk {}: {}", hash, e);
                } else {
                    debug!("R2 write-through: uploaded chunk {}", hash);
                }
                duration += r2_started.elapsed();
            }
            Some(duration)
        } else {
            None
        };

        if !exact_sizes.is_empty() {
            let db_path = self.db_path.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                let mut conn = crate::server::open_runtime_db(&db_path)?;
                let tx = conn.transaction()?;
                for (hash, size) in exact_sizes {
                    if let Some(existing) = ChunkAccess::find_by_hash(&tx, &hash)?
                        && existing.size_bytes != size
                    {
                        anyhow::bail!(
                            "chunk {hash} size disagrees with persisted CAS authority: \
                             {} != {size}",
                            existing.size_bytes
                        );
                    }
                    ChunkAccess::new(hash, size).upsert(&tx)?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
            .context("join exact chunk-size persistence task")??;
        }

        let chunk_hashes = local
            .ordered_chunks
            .into_iter()
            .map(|(hash, _)| hash)
            .collect();
        Ok(StoredChunks {
            chunk_hashes,
            chunking_duration: local.chunking_duration,
            cas_duration: local.cas_duration,
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

    fn initialized_db(temp_dir: &tempfile::TempDir) -> PathBuf {
        let db_path = temp_dir.path().join("remi.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        db_path
    }

    fn artifact_data(seed: u64, len: usize) -> Vec<u8> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                (state >> 32) as u8
            })
            .collect()
    }

    fn conversion_result_for(path: PathBuf) -> ConversionResult {
        make_conversion_result(Some(path))
    }

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
    async fn test_store_chunks_reassembles_emitted_ccs_artifact() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();
        let artifact_path = temp_dir.path().join("test.ccs");
        let artifact = artifact_data(91, 1_250_000);
        std::fs::write(&artifact_path, &artifact).unwrap();

        let service = ConversionService::new(
            chunk_dir.clone(),
            temp_dir.path().to_path_buf(),
            initialized_db(&temp_dir),
            None,
        );

        let result = conversion_result_for(artifact_path);
        let hashes = service.store_chunks(&result).await.unwrap();
        assert!(hashes.len() > 1);

        let conn = rusqlite::Connection::open(temp_dir.path().join("remi.db")).unwrap();
        let mut reassembled = Vec::new();
        let mut exact_total = 0_i64;
        for hash in &hashes {
            assert_eq!(hash.len(), 64);
            let chunk_path = object_path(&chunk_dir.join("objects"), hash).unwrap();
            let chunk = std::fs::read(chunk_path).unwrap();
            assert_eq!(conary_core::hash::sha256(&chunk), *hash);
            reassembled.extend_from_slice(&chunk);
            exact_total += ChunkAccess::find_by_hash(&conn, hash)
                .unwrap()
                .unwrap()
                .size_bytes;
        }
        assert_eq!(reassembled, artifact);
        assert_eq!(exact_total, artifact.len() as i64);
    }

    #[tokio::test]
    async fn test_store_chunks_idempotent() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();
        let artifact_path = temp_dir.path().join("repeat.ccs");
        std::fs::write(&artifact_path, artifact_data(32, 500_000)).unwrap();

        let service = ConversionService::new(
            chunk_dir.clone(),
            temp_dir.path().to_path_buf(),
            initialized_db(&temp_dir),
            None,
        );

        let result = conversion_result_for(artifact_path.clone());
        let hashes1 = service.store_chunks(&result).await.unwrap();
        let result2 = conversion_result_for(artifact_path);
        let hashes2 = service.store_chunks(&result2).await.unwrap();
        assert_eq!(hashes1, hashes2);
    }

    #[tokio::test]
    async fn test_store_chunks_rejects_empty_artifact() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();
        let artifact_path = temp_dir.path().join("empty.ccs");
        std::fs::write(&artifact_path, b"").unwrap();

        let service = ConversionService::new(
            chunk_dir,
            temp_dir.path().to_path_buf(),
            initialized_db(&temp_dir),
            None,
        );

        let result = conversion_result_for(artifact_path);
        let error = service.store_chunks(&result).await.unwrap_err();
        assert!(error.to_string().contains("empty"));
    }
}
