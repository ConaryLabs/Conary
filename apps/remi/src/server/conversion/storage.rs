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
mod tests {
    use super::*;
    use conary_core::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests;
    use conary_core::ccs::signing::SigningKeyPair;
    use conary_core::ccs::v3::schema::{
        AuthorityDocumentV3, ComponentAuthorityV3, FORMAT_VERSION_V3, FileAuthorityV3,
        FileContentLayoutV3, LifecycleAuthorityV3, PackageDataV3, PackageIdentityV3,
        PackageKindTagV3, PackageKindV3, ProvenanceAuthorityV3,
    };
    use conary_core::packages::source_authority::{CcsPackageAuthority, SourcePackageAuthority};
    use conary_core::packages::traits::{ExtractedFile, PackageFile};
    use conary_core::payload::{PayloadContentAuthority, PayloadNode};
    use conary_core::repository::versioning::VersionScheme;
    use std::collections::HashSet;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
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

    fn signed_package(root: &Path, signer: &SigningKeyPair, payload: &[u8]) -> std::path::PathBuf {
        let path = root.join("direct-cas.ccs");
        let payload_path = "/usr/bin/direct-cas".to_string();
        let authority = AuthorityDocumentV3 {
            format_version: FORMAT_VERSION_V3,
            identity: PackageIdentityV3 {
                name: "direct-cas".to_string(),
                version: "1.0.0".to_string(),
                version_scheme: VersionScheme::Conary,
                release: "1".to_string(),
                architecture: Some("x86_64".to_string()),
                debian_multi_arch: None,
                platform: Some("linux".to_string()),
                kind: PackageKindTagV3::Package,
            },
            kind: PackageKindV3::Package(PackageDataV3 {
                files: vec![FileAuthorityV3 {
                    path: payload_path.clone(),
                    node: PayloadNode::regular(0o755),
                    content: Some(PayloadContentAuthority {
                        sha256: conary_core::hash::sha256(payload),
                        size: payload.len() as u64,
                    }),
                    content_layout: FileContentLayoutV3::WholeObject,
                    component: "main".to_string(),
                    config: None,
                    conflict: Default::default(),
                }],
                ..Default::default()
            }),
            provided_capabilities: Vec::new(),
            requirements: Vec::new(),
            relations: Vec::new(),
            execution_capabilities: None,
            file_capabilities: Vec::new(),
            components: BTreeMap::from([(
                "main".to_string(),
                ComponentAuthorityV3 {
                    name: "main".to_string(),
                    default: true,
                    file_count: 1,
                    total_size: payload.len() as u64,
                },
            )]),
            lifecycle: LifecycleAuthorityV3::default(),
            provenance: ProvenanceAuthorityV3 {
                origin_class: Some("native-built".to_string()),
                hardening_level: Some("hermetic".to_string()),
                build_input_identity: Some("sha256:build-input".to_string()),
                hermetic_evidence_hash: Some("sha256:evidence".to_string()),
                foreign_conversion_boundary_hash: None,
            },
            debug_toml_sha256: None,
        };
        write_v3_ccs_package_from_bounded_memory_for_tests(
            &authority,
            &BTreeMap::from([(payload_path, payload.to_vec())]),
            &path,
            signer,
            None,
            None,
            None,
        )
        .unwrap();
        path
    }

    fn direct_cas_service(
        root: &Path,
        signer: &SigningKeyPair,
    ) -> (ConversionService, std::path::PathBuf) {
        let keys_dir = root.join("keys");
        let profile_dir = keys_dir.join("fedora-44");
        fs::create_dir_all(&profile_dir).unwrap();
        fs::set_permissions(&keys_dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&profile_dir, fs::Permissions::from_mode(0o700)).unwrap();
        crate::server::signing_authority::save_fixture_key_pair(
            signer,
            &profile_dir.join("targets.private"),
            &profile_dir.join("targets.public"),
        )
        .unwrap();

        let db_path = root.join("remi.db");
        conary_core::db::init(&db_path).unwrap();
        let chunk_dir = root.join("chunks");
        let service = ConversionService::new(chunk_dir.clone(), root.join("cache"), db_path, None)
            .with_repository_keys_dir(Some(keys_dir));
        (service, chunk_dir)
    }

    fn pending_conversion(root: &Path, signer: Arc<SigningKeyPair>) -> PendingConversionResult {
        let payload_bytes = b"verified staging payload";
        let payload_path = "/usr/bin/verified-staging".to_string();
        let content_authority = PayloadContentAuthority {
            sha256: conary_core::hash::sha256(payload_bytes),
            size: payload_bytes.len() as u64,
        };
        let mut metadata = conary_core::ccs::convert::ForeignConversionInput::new(
            Path::new("verified-staging-1.0-1.x86_64.rpm").to_path_buf(),
            "verified-staging".to_string(),
            "1.0".to_string(),
            VersionScheme::Rpm,
        );
        metadata.source_authority = SourcePackageAuthority::Ccs(CcsPackageAuthority {
            name: "verified-staging".to_string(),
            version: "1.0".to_string(),
            version_scheme: VersionScheme::Rpm,
            architecture: Some("x86_64".to_string()),
            debian_multi_arch: None,
            capabilities: Vec::new(),
            config: Vec::new(),
        });
        metadata.files = vec![PackageFile {
            path: payload_path.clone(),
            node: PayloadNode::regular(0o755),
            content: Some(content_authority.clone()),
        }];
        let payload =
            conary_core::packages::PackagePayload::from_extracted_in_memory(vec![ExtractedFile {
                path: payload_path,
                node: PayloadNode::regular(0o755),
                content: payload_bytes.to_vec(),
                content_authority: Some(content_authority),
            }])
            .unwrap();
        conary_core::ccs::convert::NativePackageConverter::new(
            conary_core::ccs::convert::ConversionOptions {
                output_dir: root.to_path_buf(),
            },
        )
        .with_source_profile("fedora-44")
        .with_source_release("1")
        .with_conversion_tool("remi-storage-test")
        .with_signing_key(signer)
        .convert_payload(
            &metadata,
            payload.files(),
            "rpm",
            &conary_core::hash::Hash::new(conary_core::hash::HashAlgorithm::Sha256, "d".repeat(64))
                .unwrap(),
        )
        .unwrap()
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
    async fn remi_verification_streams_once_into_cold_and_warm_permanent_cas() {
        let temp = tempfile::tempdir().unwrap();
        let signer = SigningKeyPair::generate().with_key_id("targets");
        let payload = b"direct verified CAS payload";
        let package = signed_package(temp.path(), &signer, payload);
        let (service, chunk_dir) = direct_cas_service(temp.path(), &signer);

        let cold = service
            .store_signed_ccs_path_with_timing(&package, "fedora-44")
            .await
            .unwrap();
        assert_eq!(cold.transport.objects.len(), 1);
        assert_eq!(cold.cas_metrics.misses, 1);
        assert_eq!(cold.cas_metrics.hits, 0);
        assert_eq!(cold.cas_metrics.incoming_bytes_hashed, payload.len() as u64);
        assert_eq!(
            cold.cas_metrics.persistent_bytes_written,
            payload.len() as u64
        );
        assert_eq!(cold.cas_metrics.objects_hashed, 1);
        assert_eq!(cold.cas_metrics.staged_data_barriers, 1);
        assert_eq!(cold.cas_metrics.canonical_name_barriers, 1);
        assert_eq!(cold.cas_metrics.canonical_bytes_reread, 0);
        let object = &cold.transport.objects[0];
        let cas = CasStore::new(chunk_dir.join("objects")).unwrap();
        assert_eq!(
            fs::read(cas.hash_to_path(&object.sha256).unwrap()).unwrap(),
            payload
        );
        let conn = crate::server::open_runtime_db(&service.db_path).unwrap();
        let persisted_size: i64 = conn
            .query_row(
                "SELECT size_bytes FROM chunk_access WHERE hash = ?1",
                [&object.sha256],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(persisted_size, payload.len() as i64);
        drop(conn);

        let warm = service
            .store_signed_ccs_path_with_timing(&package, "fedora-44")
            .await
            .unwrap();
        assert_eq!(warm.transport, cold.transport);
        assert_eq!(warm.cas_metrics.misses, 0);
        assert_eq!(warm.cas_metrics.hits, 1);
        assert_eq!(warm.cas_metrics.incoming_bytes_hashed, payload.len() as u64);
        assert_eq!(warm.cas_metrics.objects_hashed, 1);
        assert_eq!(warm.cas_metrics.persistent_bytes_written, 0);
        assert_eq!(warm.cas_metrics.staged_data_barriers, 0);
        assert_eq!(warm.cas_metrics.canonical_name_barriers, 0);
        assert_eq!(warm.cas_metrics.canonical_bytes_reread, 0);
    }

    #[tokio::test]
    async fn remi_direct_cas_verification_rejects_an_untrusted_archive_before_bookkeeping() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKeyPair::generate().with_key_id("targets");
        let untrusted = SigningKeyPair::generate().with_key_id("targets");
        let package = signed_package(temp.path(), &untrusted, b"untrusted payload");
        let (service, chunk_dir) = direct_cas_service(temp.path(), &trusted);

        let error = match service
            .store_signed_ccs_path_with_timing(&package, "fedora-44")
            .await
        {
            Ok(_) => panic!("untrusted signed archive was stored"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("signature"));
        assert_eq!(
            fs::read_dir(chunk_dir.join("objects"))
                .map(|entries| entries.count())
                .unwrap_or(0),
            0
        );
        let conn = crate::server::open_runtime_db(&service.db_path).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunk_access", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[tokio::test]
    async fn post_finalizer_same_size_mutation_fails_before_publication_bookkeeping() {
        let temp = tempfile::tempdir().unwrap();
        let signer = Arc::new(SigningKeyPair::generate().with_key_id("targets"));
        let authored = temp.path().join("authored");
        fs::create_dir(&authored).unwrap();
        let pending = pending_conversion(&authored, Arc::clone(&signer));
        let expected_bytes = pending.metrics().ccs_write.ccs_output_bytes;
        let expected_digest = pending.metrics().ccs_write.ccs_output_sha256.clone();
        let (service, _chunk_dir) = direct_cas_service(temp.path(), signer.as_ref());

        let error = match service
            .store_transport_with_timing_then(pending, "fedora-44", move |path| {
                let mut bytes = fs::read(path)?;
                ensure!(bytes.len() as u64 == expected_bytes);
                let offset = bytes.len() / 2;
                bytes[offset] ^= 0x01;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
                fs::write(path, &bytes)?;
                fs::set_permissions(path, fs::Permissions::from_mode(0o400))?;
                fs::File::open(path)?.sync_all()?;
                Ok(())
            })
            .await
        {
            Ok(_) => panic!("post-finalizer mutation was published"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("changed after finalization"),
            "{error:#}"
        );

        let packages = service.cache_dir.join("packages");
        let canonical = packages.join(format!("{expected_digest}.ccs"));
        assert!(canonical.exists());
        assert_ne!(
            conary_core::hash::sha256(&fs::read(&canonical).unwrap()),
            expected_digest
        );
        assert_eq!(fs::read_dir(&packages).unwrap().count(), 1);
        let conn = crate::server::open_runtime_db(&service.db_path).unwrap();
        let chunk_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunk_access", [], |row| row.get(0))
            .unwrap();
        let conversion_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM converted_packages", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(chunk_rows, 0);
        assert_eq!(conversion_rows, 0);
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
