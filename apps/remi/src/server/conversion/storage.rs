// apps/remi/src/server/conversion/storage.rs
//! Signed CCS object persistence and durable R2 publication.

use super::ConversionService;
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use conary_core::ccs::convert::ConversionResult;
use conary_core::ccs::transport::CcsTransportObjectV1;
use conary_core::db::models::ChunkAccess;
use conary_core::filesystem::{CasStore, VerifiedObjectBatchMetrics, object_path};
use futures::{StreamExt, stream};
use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const R2_PUBLICATION_CONCURRENCY: usize = 16;

pub(super) struct StoredTransport {
    pub(super) transport: conary_core::ccs::CcsTransportEnvelopeV1,
    pub(super) verification_duration: Duration,
    pub(super) cas_duration: Duration,
    pub(super) cas_metrics: VerifiedObjectBatchMetrics,
    pub(super) r2_duration: Option<Duration>,
    pub(super) r2_work: crate::server::conversion_timing::ConversionR2Work,
}

#[async_trait]
trait ConversionChunkStore: Send + Sync {
    async fn head_chunk(&self, hash: &str) -> Result<bool>;
    async fn put_chunk(&self, hash: &str, data: &[u8]) -> Result<()>;
}

#[async_trait]
impl ConversionChunkStore for crate::server::R2Store {
    async fn head_chunk(&self, hash: &str) -> Result<bool> {
        crate::server::R2Store::head_chunk(self, hash).await
    }

    async fn put_chunk(&self, hash: &str, data: &[u8]) -> Result<()> {
        crate::server::R2Store::put_chunk(self, hash, data).await
    }
}

async fn publish_transport_objects<S: ConversionChunkStore + ?Sized + 'static>(
    store: Arc<S>,
    objects_dir: &Path,
    objects: &[CcsTransportObjectV1],
    concurrency: usize,
) -> Result<crate::server::conversion_timing::ConversionR2Work> {
    ensure!(
        concurrency > 0,
        "R2 publication concurrency must be positive"
    );

    let objects_dir = objects_dir.to_path_buf();
    let outcomes = stream::iter(objects.iter().cloned())
        .map(|object| {
            let store = Arc::clone(&store);
            let objects_dir = objects_dir.clone();
            async move {
                let mut work = crate::server::conversion_timing::ConversionR2Work {
                    head_requests: 1,
                    ..Default::default()
                };
                if store.head_chunk(&object.sha256).await? {
                    work.hits = 1;
                    return Ok(work);
                }

                work.misses = 1;
                let path = object_path(&objects_dir, &object.sha256)?;
                let data = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("read signed CCS object {}", object.sha256))?;
                conary_core::hash::verify_sha256(&data, &object.sha256)?;
                store.put_chunk(&object.sha256, &data).await?;
                work.put_requests = 1;
                work.bytes_written = object.size;
                Ok::<_, anyhow::Error>(work)
            }
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;

    outcomes.into_iter().try_fold(
        crate::server::conversion_timing::ConversionR2Work::default(),
        |mut total, outcome| {
            let work = outcome?;
            total.head_requests += work.head_requests;
            total.hits += work.hits;
            total.misses += work.misses;
            total.put_requests += work.put_requests;
            total.bytes_written += work.bytes_written;
            Ok(total)
        },
    )
}

async fn publish_transport_objects_then<S, F, Fut>(
    store: Arc<S>,
    objects_dir: &Path,
    objects: &[CcsTransportObjectV1],
    concurrency: usize,
    after_durable_publication: F,
) -> Result<(Duration, crate::server::conversion_timing::ConversionR2Work)>
where
    S: ConversionChunkStore + ?Sized + 'static,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let started = Instant::now();
    let work = publish_transport_objects(store, objects_dir, objects, concurrency).await?;
    let duration = started.elapsed();
    after_durable_publication().await?;
    Ok((duration, work))
}

impl ConversionService {
    pub(super) async fn store_transport_with_timing(
        &self,
        result: &ConversionResult,
        source_profile: &str,
    ) -> Result<StoredTransport> {
        let package_path = result
            .package_path
            .as_deref()
            .context("conversion did not emit a CCS artifact")?;
        self.store_signed_ccs_path_with_timing(package_path, source_profile)
            .await
    }

    /// Verify the signed CCS authority, persist exactly its canonical objects,
    /// and publish only absent objects to the configured durable store.
    pub(super) async fn store_signed_ccs_path_with_timing(
        &self,
        package_path: &Path,
        source_profile: &str,
    ) -> Result<StoredTransport> {
        let objects_dir = self.chunk_dir.join("objects");
        let keys_dir = self.repository_keys_dir.as_ref().context(
            "Remi conversion requires release_publish.repository_keys_dir for CCS transport verification",
        )?;
        let signing_key = crate::server::signing_authority::load_role_key(
            keys_dir,
            source_profile,
            crate::server::signing_authority::RepositorySigningRole::Targets,
        )?;
        let policy = conary_core::ccs::TrustPolicy::strict(vec![signing_key.public_key_base64()]);

        let verify_started = Instant::now();
        let local_artifact = package_path.to_path_buf();
        let verification = tokio::task::spawn_blocking(move || {
            conary_core::ccs::verify::verify_package(&local_artifact, &policy)
        })
        .await
        .context("join emitted CCS transport verification task")??;
        let verification_duration = verify_started.elapsed();
        let transport =
            conary_core::ccs::CcsTransportEnvelopeV1::from_verified_archive(&verification)?;
        let exact_sizes = transport
            .objects
            .iter()
            .map(|object| {
                Ok((
                    object.sha256.clone(),
                    i64::try_from(object.size).context("CCS object size exceeds i64")?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        let local_objects_dir = objects_dir.clone();
        let cas_started = Instant::now();
        let verified_set = tokio::task::spawn_blocking(move || {
            let cas =
                CasStore::new(local_objects_dir).context("initialize signed CCS object CAS")?;
            conary_core::ccs::transport::persist_verified_archive_objects(&verification, &cas)
        })
        .await
        .context("join signed CCS object persistence task")??;
        let cas_duration = cas_started.elapsed();
        let cas_metrics = verified_set.metrics();

        // Publish cache bookkeeping only after every durable write succeeds. A
        // failed R2 write must not leave a non-evictable local-only cache row.
        // The upsert also refreshes the GC grace authority for CAS hits.
        let (r2_duration, r2_work) = if let Some(r2) = self.r2_store.clone() {
            let (duration, work) = publish_transport_objects_then(
                r2,
                &objects_dir,
                &transport.objects,
                R2_PUBLICATION_CONCURRENCY,
                || self.persist_chunk_sizes(&exact_sizes),
            )
            .await?;
            (Some(duration), work)
        } else {
            self.persist_chunk_sizes(&exact_sizes).await?;
            (
                None,
                crate::server::conversion_timing::ConversionR2Work::default(),
            )
        };

        if let (Some(r2), Some(bounded_cache)) = (&self.r2_store, &self.bounded_cache) {
            bounded_cache
                .enforce(r2.as_ref())
                .await
                .context("enforce bounded local cache after durable R2 publication")?;
        }

        Ok(StoredTransport {
            transport,
            verification_duration,
            cas_duration,
            cas_metrics,
            r2_duration,
            r2_work,
        })
    }

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
                let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                for (hash, size) in exact_sizes {
                    if let Some(existing) = ChunkAccess::find_by_hash(&tx, &hash)?
                        && existing.size_bytes != size
                    {
                        anyhow::bail!(
                            "chunk {hash} size disagrees with persisted CAS authority: {} != {size}",
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
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct InFlight<'a>(&'a AtomicUsize);

    impl Drop for InFlight<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct InjectedStore {
        hits: HashSet<String>,
        failed_put: Option<String>,
        current: AtomicUsize,
        maximum: AtomicUsize,
        completed: AtomicUsize,
        uploaded: Mutex<Vec<String>>,
    }

    impl InjectedStore {
        fn enter(&self) -> InFlight<'_> {
            let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(current, Ordering::SeqCst);
            InFlight(&self.current)
        }

        async fn overlap(&self, hash: &str) {
            let delay = 1 + usize::from(hash.as_bytes()[0]) % 4;
            tokio::time::sleep(Duration::from_millis(delay as u64)).await;
        }
    }

    #[async_trait]
    impl ConversionChunkStore for InjectedStore {
        async fn head_chunk(&self, hash: &str) -> Result<bool> {
            let _in_flight = self.enter();
            self.overlap(hash).await;
            let hit = self.hits.contains(hash);
            if hit {
                self.completed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(hit)
        }

        async fn put_chunk(&self, hash: &str, _data: &[u8]) -> Result<()> {
            let _in_flight = self.enter();
            self.overlap(hash).await;
            self.uploaded.lock().unwrap().push(hash.to_string());
            self.completed.fetch_add(1, Ordering::SeqCst);
            ensure!(
                self.failed_put.as_deref() != Some(hash),
                "injected PUT failure for {hash}"
            );
            Ok(())
        }
    }

    fn stored_objects(
        objects_dir: &Path,
        count: usize,
    ) -> (Vec<CcsTransportObjectV1>, Vec<Vec<u8>>) {
        let cas = CasStore::new(objects_dir).unwrap();
        let payloads = (0..count)
            .map(|index| format!("parallel publication object {index}").into_bytes())
            .collect::<Vec<_>>();
        let objects = payloads
            .iter()
            .map(|payload| CcsTransportObjectV1 {
                sha256: cas.store(payload).unwrap(),
                size: payload.len() as u64,
            })
            .collect();
        (objects, payloads)
    }

    #[test]
    fn calculated_sha256_is_typed_and_serializes_with_algorithm() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.pkg");
        std::fs::write(&file_path, b"hello world").unwrap();

        let checksum = ConversionService::calculate_sha256(&file_path).unwrap();
        assert_eq!(
            checksum.to_prefixed_string(),
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn calculate_sha256_rejects_a_missing_file() {
        assert!(ConversionService::calculate_sha256(Path::new("/nonexistent/file.pkg")).is_err());
    }

    #[tokio::test]
    async fn r2_publication_is_bounded_and_accounts_for_out_of_order_work() {
        let temp = tempfile::TempDir::new().unwrap();
        let (objects, payloads) = stored_objects(temp.path(), 9);
        let hit_hashes = [objects[1].sha256.clone(), objects[7].sha256.clone()];
        let store = Arc::new(InjectedStore {
            hits: hit_hashes.into_iter().collect(),
            ..Default::default()
        });
        let bookkeeping_ran = Arc::new(AtomicBool::new(false));
        let object_count = objects.len();

        let (duration, work) =
            publish_transport_objects_then(Arc::clone(&store), temp.path(), &objects, 3, {
                let store = Arc::clone(&store);
                let bookkeeping_ran = Arc::clone(&bookkeeping_ran);
                move || async move {
                    assert_eq!(store.current.load(Ordering::SeqCst), 0);
                    assert_eq!(store.completed.load(Ordering::SeqCst), object_count);
                    bookkeeping_ran.store(true, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await
            .unwrap();

        assert!(duration > Duration::ZERO);
        assert!(bookkeeping_ran.load(Ordering::SeqCst));
        assert!(store.maximum.load(Ordering::SeqCst) > 1);
        assert!(store.maximum.load(Ordering::SeqCst) <= 3);
        assert_eq!(work.head_requests, objects.len() as u64);
        assert_eq!(work.hits, 2);
        assert_eq!(work.misses, 7);
        assert_eq!(work.put_requests, 7);
        let expected_bytes = objects
            .iter()
            .zip(&payloads)
            .filter(|(object, _)| !store.hits.contains(&object.sha256))
            .map(|(_, payload)| payload.len() as u64)
            .sum::<u64>();
        assert_eq!(work.bytes_written, expected_bytes);
        assert_eq!(store.uploaded.lock().unwrap().len(), 7);
    }

    #[tokio::test]
    async fn r2_publication_failure_prevents_bookkeeping() {
        let temp = tempfile::TempDir::new().unwrap();
        let (objects, _) = stored_objects(temp.path(), 6);
        let failed_hash = objects[3].sha256.clone();
        let store = Arc::new(InjectedStore {
            failed_put: Some(failed_hash.clone()),
            ..Default::default()
        });
        let bookkeeping_ran = Arc::new(AtomicBool::new(false));

        let result =
            publish_transport_objects_then(Arc::clone(&store), temp.path(), &objects, 2, {
                let bookkeeping_ran = Arc::clone(&bookkeeping_ran);
                move || async move {
                    bookkeeping_ran.store(true, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await;

        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains(&format!("injected PUT failure for {failed_hash}"))
        );
        assert!(!bookkeeping_ran.load(Ordering::SeqCst));
        assert_eq!(store.current.load(Ordering::SeqCst), 0);
        assert_eq!(store.completed.load(Ordering::SeqCst), objects.len());
        assert!(store.maximum.load(Ordering::SeqCst) <= 2);
    }
}
