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

/// How one write-through pass over the artifact's chunks ended.
enum WriteThroughOutcome {
    /// Every chunk was read, verified, and offered to the uploader.
    Completed(Duration),
    /// A local chunk had been deleted underneath the pass. The caller repairs
    /// the CAS and restarts; the elapsed time so far is still reported.
    ChunkMissing { hash: String, elapsed: Duration },
}

/// Upload every chunk in `hashes` once, reading each back from the local CAS.
///
/// A chunk that is missing ends the pass early when `allow_restart` is set, so
/// the caller can repair the CAS and start over. Without it, a missing chunk is
/// a hard error carrying the same context as any other failed read.
///
/// An upload failure is warned and skipped rather than propagated, matching how
/// R2 write-through has always treated a transient upload error: the chunk is on
/// local disk either way, and the next conversion or serve re-uploads it.
///
/// A chunk that reads back with the wrong hash is always a hard error. That is
/// corruption, not a garbage-collection race, and rewriting over it would
/// destroy the evidence.
async fn write_through_pass<F, Fut>(
    objects_dir: &Path,
    hashes: &BTreeSet<String>,
    allow_restart: bool,
    upload: &F,
) -> Result<WriteThroughOutcome>
where
    F: Fn(String, Vec<u8>) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut elapsed = Duration::default();

    for hash in hashes {
        let started = Instant::now();
        let chunk_path = object_path(objects_dir, hash)?;
        let data = match tokio::fs::read(&chunk_path).await {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && allow_restart => {
                elapsed += started.elapsed();
                return Ok(WriteThroughOutcome::ChunkMissing {
                    hash: hash.clone(),
                    elapsed,
                });
            }
            Err(e) => {
                return Err(anyhow::Error::new(e))
                    .with_context(|| format!("read local CAS chunk {hash} for R2 write-through"));
            }
        };

        let actual_hash = conary_core::hash::sha256(&data);
        if actual_hash != *hash {
            anyhow::bail!("local CAS chunk failed R2 integrity check: {actual_hash} != {hash}");
        }

        if let Err(e) = upload(hash.clone(), data).await {
            tracing::warn!("R2 write-through failed for chunk {}: {}", hash, e);
        } else {
            debug!("R2 write-through: uploaded chunk {}", hash);
        }
        elapsed += started.elapsed();
    }

    Ok(WriteThroughOutcome::Completed(elapsed))
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
        let local_artifact = package_path.clone();
        let local = tokio::task::spawn_blocking(move || {
            chunk_and_store_ccs_artifact(&local_artifact, &local_objects_dir)
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

        // Persist chunk_access before the R2 write-through, not after it. The
        // upsert refreshes last_accessed, and that timestamp is the only thing
        // that puts a chunk this conversion deduplicated against inside the
        // chunk GC's grace window. Running it after the write-through left every
        // chunk of a large artifact unprotected for the whole upload, which is
        // how a concurrent GC deleted one out from under the read below.
        self.persist_chunk_sizes(&exact_sizes).await?;

        let r2_duration = if let Some(r2) = self.r2_store.clone() {
            let unique_hashes: BTreeSet<String> = local
                .ordered_chunks
                .iter()
                .map(|(hash, _)| hash.clone())
                .collect();
            Some(
                self.write_through_to_r2(
                    &objects_dir,
                    &package_path,
                    &unique_hashes,
                    &exact_sizes,
                    move |hash, data| {
                        let r2 = r2.clone();
                        async move { r2.put_chunk(&hash, &data).await }
                    },
                )
                .await?,
            )
        } else {
            None
        };

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

    /// Write every chunk of the emitted artifact through to R2, restarting the
    /// whole pass once if a concurrent chunk GC deletes a local chunk mid-flight.
    ///
    /// Resuming at the missing chunk is not enough. The GC that removed it is
    /// deleting a whole snapshot of orphans, so chunks this pass already
    /// uploaded can be removed from R2 behind it. Restarting from the first hash
    /// re-uploads everything and leaves the published chunk list pointing at
    /// objects that exist. The cost is a duplicate upload of the chunks already
    /// sent in the abandoned pass; that is accepted rather than optimized,
    /// because the restart path is rare (one occurrence across a GC that deleted
    /// 2.1M chunks) and a chunk list pointing at R2 404s is not.
    ///
    /// The CAS repair and the `chunk_access` refresh both run before any further
    /// upload work, so the recreated chunks are back inside the GC's grace window
    /// before the second pass spends time on them.
    ///
    /// Exactly one restart is allowed. The artifact has just been re-chunked at
    /// that point, so a chunk still missing during the second pass is not this
    /// race and fails with the same error a missing chunk has always produced.
    ///
    /// Residual, accepted: a GC bulk delete that lands after the restarted
    /// uploads can still remove a deduplicated object, and no amount of retrying
    /// inside the conversion closes that. Closing it needs the conversion and the
    /// GC to synchronize -- issue #316's deferred option 2. What the
    /// reference-time `chunk_access` touch does buy is a bound: only a conversion
    /// that began before the GC's grace query can be caught, because any later
    /// one is inside the grace window the GC honors.
    async fn write_through_to_r2<F, Fut>(
        &self,
        objects_dir: &Path,
        package_path: &Path,
        hashes: &BTreeSet<String>,
        exact_sizes: &BTreeMap<String, i64>,
        upload: F,
    ) -> Result<Duration>
    where
        F: Fn(String, Vec<u8>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let mut duration = Duration::default();
        let mut restarts_left = 1usize;

        loop {
            match write_through_pass(objects_dir, hashes, restarts_left > 0, &upload).await? {
                WriteThroughOutcome::Completed(elapsed) => {
                    duration += elapsed;
                    return Ok(duration);
                }
                WriteThroughOutcome::ChunkMissing { hash, elapsed } => {
                    duration += elapsed;
                    restarts_left -= 1;
                    tracing::warn!(
                        "local CAS chunk {} disappeared during R2 write-through; \
                         rewriting the emitted artifact's chunks and restarting the pass",
                        hash
                    );

                    let restart_started = Instant::now();
                    let repair_artifact = package_path.to_path_buf();
                    let repair_objects = objects_dir.to_path_buf();
                    tokio::task::spawn_blocking(move || {
                        chunk_and_store_ccs_artifact(&repair_artifact, &repair_objects)
                    })
                    .await
                    .context("join CAS repair task for R2 write-through")??;
                    duration += restart_started.elapsed();

                    // The GC pass that removed the chunk file also removed its
                    // chunk_access row. Put the sizes and the grace timestamp
                    // back before re-uploading anything.
                    self.persist_chunk_sizes(exact_sizes).await?;
                }
            }
        }
    }

    /// Record each chunk's exact size in `chunk_access`.
    ///
    /// The upsert also refreshes `last_accessed`, which is what the chunk GC's
    /// grace period reads, so this doubles as the reference-time touch for
    /// chunks that deduplicated against an already-stored copy.
    async fn persist_chunk_sizes(&self, exact_sizes: &BTreeMap<String, i64>) -> Result<()> {
        if exact_sizes.is_empty() {
            return Ok(());
        }

        let exact_sizes = exact_sizes.clone();
        let db_path = self.db_path.clone();
        let writer = self.database_writer.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            writer.execute(|| {
                let mut conn = crate::server::open_runtime_db(&db_path)?;
                let tx =
                    conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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
        })
        .await
        .context("join exact chunk-size persistence task")??;

        Ok(())
    }

    /// Calculate the typed SHA-256 checksum of a file.
    pub(super) fn calculate_sha256(path: &Path) -> Result<conary_core::hash::Hash> {
        let mut file = std::fs::File::open(path)?;
        Ok(conary_core::hash::hash_reader(
            conary_core::hash::HashAlgorithm::Sha256,
            &mut file,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::make_conversion_result;
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    fn initialized_db(temp_dir: &tempfile::TempDir) -> PathBuf {
        let db_path = temp_dir.path().join("remi.db");
        conary_core::db::init(&db_path).unwrap();
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
    fn calculated_sha256_is_typed_and_serializes_with_algorithm() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.pkg");
        std::fs::write(&file_path, b"hello world").unwrap();

        let checksum = ConversionService::calculate_sha256(&file_path).unwrap();
        // SHA-256 of "hello world"
        assert_eq!(
            checksum.to_prefixed_string(),
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(checksum.algorithm, conary_core::hash::HashAlgorithm::Sha256);
    }

    #[test]
    fn test_calculate_checksum_empty_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("empty.pkg");
        std::fs::write(&file_path, b"").unwrap();

        let checksum = ConversionService::calculate_sha256(&file_path).unwrap();
        // SHA-256 of empty string
        assert_eq!(
            checksum.to_prefixed_string(),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_calculate_checksum_missing_file() {
        let result = ConversionService::calculate_sha256(Path::new("/nonexistent/file.pkg"));
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

    /// A conversion that deduplicates against already-stored chunks must still
    /// refresh `chunk_access.last_accessed`, because that timestamp is the only
    /// thing the chunk GC's grace period consults before deleting an orphan.
    #[tokio::test]
    async fn dedup_reference_refreshes_the_chunk_gc_grace_window() {
        const ANCIENT: &str = "2020-01-01 00:00:00";

        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();
        let artifact_path = temp_dir.path().join("dedup.ccs");
        std::fs::write(&artifact_path, artifact_data(77, 800_000)).unwrap();

        let db_path = initialized_db(&temp_dir);
        let service = ConversionService::new(
            chunk_dir,
            temp_dir.path().to_path_buf(),
            db_path.clone(),
            None,
        );

        let hashes = service
            .store_chunks(&conversion_result_for(artifact_path.clone()))
            .await
            .unwrap();
        assert!(hashes.len() > 1);

        // Age every row past any plausible grace period, then convert the same
        // artifact again: every chunk is already in the CAS, so this second run
        // stores nothing new and only takes references.
        let conn = crate::server::open_runtime_db(&db_path).unwrap();
        conn.execute("UPDATE chunk_access SET last_accessed = ?1", [ANCIENT])
            .unwrap();
        drop(conn);

        service
            .store_chunks(&conversion_result_for(artifact_path))
            .await
            .unwrap();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let still_ancient: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunk_access WHERE last_accessed <= ?1",
                [ANCIENT],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            still_ancient, 0,
            "a deduplicated chunk kept a stale last_accessed, so the GC grace \
             period would not cover it"
        );
    }

    /// Drive the write-through machine over a temp CAS, recording every upload
    /// it performs. The uploader is a parameter precisely so the restart
    /// behavior is observable without a live R2 bucket.
    struct WriteThroughHarness {
        _temp_dir: tempfile::TempDir,
        service: ConversionService,
        objects_dir: PathBuf,
        artifact_path: PathBuf,
        db_path: PathBuf,
        /// Sorted exactly as the write-through pass walks them.
        hashes: BTreeSet<String>,
        exact_sizes: BTreeMap<String, i64>,
    }

    impl WriteThroughHarness {
        fn new(seed: u64, len: usize) -> Self {
            let temp_dir = tempfile::TempDir::new().unwrap();
            let chunk_dir = temp_dir.path().join("chunks");
            std::fs::create_dir_all(&chunk_dir).unwrap();
            let objects_dir = chunk_dir.join("objects");
            let artifact_path = temp_dir.path().join("raced.ccs");
            std::fs::write(&artifact_path, artifact_data(seed, len)).unwrap();

            let stored = chunk_and_store_ccs_artifact(&artifact_path, &objects_dir).unwrap();
            assert!(
                stored.ordered_chunks.len() > 2,
                "the restart tests need an artifact that chunks into several pieces"
            );
            let hashes: BTreeSet<String> = stored
                .ordered_chunks
                .iter()
                .map(|(hash, _)| hash.clone())
                .collect();
            let exact_sizes: BTreeMap<String, i64> =
                stored.ordered_chunks.iter().cloned().collect();

            let db_path = initialized_db(&temp_dir);
            let service = ConversionService::new(
                chunk_dir,
                temp_dir.path().to_path_buf(),
                db_path.clone(),
                None,
            );

            Self {
                _temp_dir: temp_dir,
                service,
                objects_dir,
                artifact_path,
                db_path,
                hashes,
                exact_sizes,
            }
        }

        /// The chunk the pass visits last, so a delete there proves every
        /// earlier chunk was re-uploaded rather than skipped.
        fn last_visited_hash(&self) -> String {
            self.hashes.iter().next_back().unwrap().clone()
        }

        fn delete_chunk(&self, hash: &str) {
            std::fs::remove_file(object_path(&self.objects_dir, hash).unwrap()).unwrap();
        }

        fn chunk_access_rows(&self) -> i64 {
            rusqlite::Connection::open(&self.db_path)
                .unwrap()
                .query_row("SELECT COUNT(*) FROM chunk_access", [], |row| row.get(0))
                .unwrap()
        }

        /// Run the machine, recording `(hash, chunk_access rows visible)` for
        /// every upload in order.
        async fn run(&self) -> Result<(Duration, Vec<(String, i64)>)> {
            let uploads = Arc::new(Mutex::new(Vec::new()));
            let recorder = Arc::clone(&uploads);
            let db_path = self.db_path.clone();
            let duration = self
                .service
                .write_through_to_r2(
                    &self.objects_dir,
                    &self.artifact_path,
                    &self.hashes,
                    &self.exact_sizes,
                    move |hash, _data| {
                        let recorder = Arc::clone(&recorder);
                        let db_path = db_path.clone();
                        async move {
                            let rows: i64 = rusqlite::Connection::open(&db_path)
                                .unwrap()
                                .query_row("SELECT COUNT(*) FROM chunk_access", [], |row| {
                                    row.get(0)
                                })
                                .unwrap();
                            recorder.lock().unwrap().push((hash, rows));
                            Ok(())
                        }
                    },
                )
                .await?;
            let uploads = uploads.lock().unwrap().clone();
            Ok((duration, uploads))
        }
    }

    /// The observed production race: a concurrent GC unlinks a chunk this
    /// conversion is writing through. The conversion must survive it, and must
    /// restart the pass rather than resume -- the same GC snapshot can delete
    /// objects this pass already uploaded, so resuming would publish a chunk
    /// list pointing at R2 objects that are gone.
    #[tokio::test]
    async fn write_through_restarts_the_whole_pass_when_a_chunk_is_deleted_mid_flight() {
        let harness = WriteThroughHarness::new(13, 900_000);
        let missing = harness.last_visited_hash();
        let total = harness.hashes.len();

        harness.delete_chunk(&missing);

        let (_, uploads) = harness
            .run()
            .await
            .expect("write-through must survive a chunk deleted mid-pass");

        // First pass uploads everything before the missing chunk and stops;
        // the restart then uploads all of them again plus the repaired one.
        assert_eq!(
            uploads.len(),
            (total - 1) + total,
            "the restart must re-upload the chunks the abandoned pass had sent,              not resume at the missing one"
        );
        let restarted: Vec<String> = uploads[total - 1..]
            .iter()
            .map(|(hash, _)| hash.clone())
            .collect();
        assert_eq!(
            restarted,
            harness.hashes.iter().cloned().collect::<Vec<_>>(),
            "the second pass must cover every chunk in order"
        );
        assert!(
            object_path(&harness.objects_dir, &missing)
                .unwrap()
                .exists(),
            "the repair must restore the deleted chunk file"
        );
    }

    /// A GC deletes the chunk file and its `chunk_access` row together. The
    /// restart must put the protection rows back before it spends any more time
    /// uploading, or the second pass runs unprotected too.
    #[tokio::test]
    async fn write_through_restores_chunk_access_before_re_uploading() {
        let harness = WriteThroughHarness::new(29, 900_000);
        let missing = harness.last_visited_hash();
        let total = harness.hashes.len();

        harness
            .service
            .persist_chunk_sizes(&harness.exact_sizes)
            .await
            .unwrap();
        assert_eq!(harness.chunk_access_rows(), total as i64);

        // What a GC run does: unlink the chunk, then drop its tracking rows.
        harness.delete_chunk(&missing);
        crate::server::open_runtime_db(&harness.db_path)
            .unwrap()
            .execute("DELETE FROM chunk_access", [])
            .unwrap();

        let (_, uploads) = harness.run().await.unwrap();

        let (abandoned, restarted) = uploads.split_at(total - 1);
        assert!(
            abandoned.iter().all(|(_, rows)| *rows == 0),
            "the abandoned pass ran after the GC dropped the rows"
        );
        assert!(
            restarted.iter().all(|(_, rows)| *rows == total as i64),
            "every upload after the restart must see its protection row restored"
        );
    }

    /// Exactly one restart. The artifact has just been re-chunked, so a chunk
    /// still missing in the second pass is not this race, and it fails with the
    /// error a missing chunk has always produced.
    #[tokio::test]
    async fn write_through_restarts_at_most_once() {
        let harness = WriteThroughHarness::new(21, 900_000);
        let missing = harness.last_visited_hash();
        harness.delete_chunk(&missing);

        // Replace the artifact so the repair cannot reproduce the deleted chunk.
        std::fs::write(&harness.artifact_path, artifact_data(99, 900_000)).unwrap();

        let error = harness.run().await.unwrap_err().to_string();
        assert!(
            error.contains(&format!(
                "read local CAS chunk {missing} for R2 write-through"
            )),
            "{error}"
        );
    }

    /// Corruption is not a garbage-collection race. It is a hard error in the
    /// first pass, where a restart would otherwise have been allowed, so a
    /// rewrite can never paper over it.
    #[tokio::test]
    async fn write_through_rejects_a_corrupt_local_chunk_without_restarting() {
        let harness = WriteThroughHarness::new(44, 900_000);
        let corrupt = harness.last_visited_hash();
        std::fs::write(
            object_path(&harness.objects_dir, &corrupt).unwrap(),
            b"not the chunk",
        )
        .unwrap();

        let error = harness.run().await.unwrap_err().to_string();
        assert!(error.contains("failed R2 integrity check"), "{error}");
        assert_eq!(
            std::fs::read(object_path(&harness.objects_dir, &corrupt).unwrap()).unwrap(),
            b"not the chunk",
            "a corrupt chunk must be left as evidence, not rewritten"
        );
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
