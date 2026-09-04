// apps/remi/src/server/conversion/storage.rs
//! Signed CCS object persistence and durable R2 publication.

pub(super) mod artifact;

use super::ConversionService;
use anyhow::{Context, Result, ensure};
use artifact::{
    ConversionArchiveWork, PublishedConversionArtifact, stage_finalize_and_publish_then,
};
use async_trait::async_trait;
use conary_core::ccs::convert::{ConversionResult, PendingConversionResult};
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
    pub(super) verification_and_cas_duration: Duration,
    pub(super) archive_decode_metrics: conary_core::ccs::VerifiedArchiveDecodeMetrics,
    pub(super) cas_metrics: VerifiedObjectBatchMetrics,
    pub(super) r2_duration: Option<Duration>,
    pub(super) r2_work: crate::server::conversion_timing::ConversionR2Work,
}

pub(super) struct StoredConversion {
    pub(super) stored_transport: StoredTransport,
    pub(super) conversion: ConversionResult,
    pub(super) artifact: PublishedConversionArtifact,
    pub(super) archive_work: ConversionArchiveWork,
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
        pending: PendingConversionResult,
        source_profile: &str,
    ) -> Result<StoredConversion> {
        self.store_transport_with_timing_then(pending, source_profile, |_| Ok(()))
            .await
    }

    async fn store_transport_with_timing_then<F>(
        &self,
        pending: PendingConversionResult,
        source_profile: &str,
        after_publication: F,
    ) -> Result<StoredConversion>
    where
        F: FnOnce(&Path) -> Result<()> + Send + 'static,
    {
        let objects_dir = self.chunk_dir.join("objects");
        let packages_dir = self.cache_dir.join("packages");
        let keys_dir = self.repository_keys_dir.as_ref().context(
            "Remi conversion requires release_publish.repository_keys_dir for CCS transport verification",
        )?;
        let signing_key = crate::server::signing_authority::load_role_key(
            keys_dir,
            source_profile,
            crate::server::signing_authority::RepositorySigningRole::Targets,
        )?;
        let policy = conary_core::ccs::TrustPolicy::strict(vec![signing_key.public_key_base64()]);

        let local_objects_dir = objects_dir.clone();
        let archive_cpu = self.archive_cpu.clone();
        let finalized = tokio::task::spawn_blocking(move || {
            let cas =
                CasStore::new(local_objects_dir).context("initialize signed CCS object CAS")?;
            stage_finalize_and_publish_then(
                pending,
                &policy,
                &cas,
                &archive_cpu,
                &packages_dir,
                after_publication,
            )
        })
        .await
        .context("join emitted CCS transport verification and CAS ingestion task")??;
        let artifact::FinalizedConversionArtifact {
            conversion,
            verification,
            artifact,
            work: archive_work,
            verification_and_cas_duration,
        } = finalized;
        let stored_transport = self
            .publish_verified_transport(
                verification,
                verification_and_cas_duration,
                &objects_dir,
                Some(&artifact),
            )
            .await?;
        Ok(StoredConversion {
            stored_transport,
            conversion,
            artifact,
            archive_work,
        })
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

        let verify_and_cas_started = Instant::now();
        let local_artifact = package_path.to_path_buf();
        let local_objects_dir = objects_dir.clone();
        let archive_cpu = self.archive_cpu.clone();
        let verification = tokio::task::spawn_blocking(move || {
            let cas =
                CasStore::new(local_objects_dir).context("initialize signed CCS object CAS")?;
            conary_core::ccs::verify::verify_package_into_cas_with_archive_cpu_admission(
                &local_artifact,
                &policy,
                &cas,
                &archive_cpu,
            )
        })
        .await
        .context("join emitted CCS transport verification and CAS ingestion task")??;
        let verification_and_cas_duration = verify_and_cas_started.elapsed();
        self.publish_verified_transport(
            verification,
            verification_and_cas_duration,
            &objects_dir,
            None,
        )
        .await
    }

    async fn publish_verified_transport(
        &self,
        verification: conary_core::ccs::VerifiedCcsArchive,
        verification_and_cas_duration: Duration,
        objects_dir: &Path,
        conversion_artifact: Option<&PublishedConversionArtifact>,
    ) -> Result<StoredTransport> {
        let cas_metrics = verification
            .verified_object_metrics()
            .context("permanent CCS verification omitted verified-CAS work evidence")?;
        let archive_decode_metrics = verification
            .archive_decode_metrics()
            .context("permanent CCS verification omitted archive-decode work evidence")?;
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
        drop(verification);
        if let Some(artifact) = conversion_artifact {
            artifact.require_publication_binding()?;
        }

        // Publish cache bookkeeping only after every durable write succeeds. A
        // failed R2 write must not leave a non-evictable local-only cache row.
        // The upsert also refreshes the GC grace authority for CAS hits.
        let (r2_duration, r2_work) = if let Some(r2) = self.r2_store.clone() {
            let (duration, work) = publish_transport_objects_then(
                r2,
                objects_dir,
                &transport.objects,
                R2_PUBLICATION_CONCURRENCY,
                || async {
                    if let Some(artifact) = conversion_artifact {
                        artifact.require_publication_binding()?;
                    }
                    self.persist_chunk_sizes(&exact_sizes).await
                },
            )
            .await?;
            (Some(duration), work)
        } else {
            if let Some(artifact) = conversion_artifact {
                artifact.require_publication_binding()?;
            }
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
            verification_and_cas_duration,
            archive_decode_metrics,
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
mod tests;
