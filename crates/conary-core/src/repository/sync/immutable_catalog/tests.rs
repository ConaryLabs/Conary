// crates/conary-core/src/repository/sync/immutable_catalog/tests.rs

use super::*;
use crate::db::models::{
    NativeSourceEcosystem, NativeSourceStream, RepositoryPolicyScope, RepositorySourcePolicy,
    RepositoryUpdateMode,
};
use crate::repository::catalog::{
    CATALOG_FILE_NAME, CatalogCopyScratchV1, CatalogFinalizationScratchV2,
    CatalogMetadataObjectScratchV1, CatalogMetadataScratchV1, CatalogMetadataStreamAdmission,
    CatalogMetadataStreamScratchV1, CatalogPackageOriginV1, CatalogProjectionSpoolScratchV1,
    CatalogScopeV1, CatalogScratchAdmission, CatalogScratchCapacityError,
    CatalogSourceCandidateScratchV1, CatalogSourceEvidenceV1, SourceMetadataObjectRoleV1,
    write_catalog_candidate, write_source_catalog_manifest,
};
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::parsers::PackageMetadata;
use crate::repository::sync::synced_package_row;
use crate::repository::sync::types::SyncedPackageRow;
use crate::repository::versioning::VersionScheme;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct RecordingAdmission {
    source_candidates: Mutex<Vec<CatalogSourceCandidateScratchV1>>,
    finalizations: Mutex<Vec<CatalogFinalizationScratchV2>>,
    metadata: Mutex<Vec<CatalogMetadataScratchV1>>,
    streams: Mutex<Vec<CatalogMetadataStreamScratchV1>>,
    projection_spools: Mutex<Vec<CatalogProjectionSpoolScratchV1>>,
    stream_chunks: Arc<Mutex<Vec<u64>>>,
    lease_drops: Arc<AtomicUsize>,
    refuse_source: bool,
}

struct RecordingLease(Arc<AtomicUsize>);

struct RecordingStreamAdmission(Arc<Mutex<Vec<u64>>>);

impl CatalogMetadataStreamAdmission for RecordingStreamAdmission {
    fn reserve_next(&self, additional_bytes: u64) -> Result<Box<dyn Send>> {
        self.0.lock().unwrap().push(additional_bytes);
        Ok(Box::new(()))
    }
}

impl Drop for RecordingLease {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl CatalogScratchAdmission for RecordingAdmission {
    fn reserve_source_candidate(
        &self,
        _candidate_path: &Path,
        requirement: CatalogSourceCandidateScratchV1,
    ) -> Result<Box<dyn Send>> {
        self.source_candidates.lock().unwrap().push(requirement);
        if self.refuse_source {
            return Err(CatalogScratchCapacityError {
                required_bytes: requirement.required_additional_bytes,
                available_bytes: requirement.required_additional_bytes - 1,
                reserved_bytes: 0,
            }
            .into());
        }
        Ok(Box::new(RecordingLease(Arc::clone(&self.lease_drops))))
    }

    fn reserve_profile_candidate(
        &self,
        _candidate_path: &Path,
        _requirement: crate::repository::catalog::CatalogProfileCandidateScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("source catalog sink must not request profile growth admission")
    }

    fn reserve_metadata(
        &self,
        _work_directory: &Path,
        requirement: CatalogMetadataScratchV1,
    ) -> Result<Box<dyn Send>> {
        self.metadata.lock().unwrap().push(requirement);
        Ok(Box::new(RecordingLease(Arc::clone(&self.lease_drops))))
    }

    fn stream_metadata(
        &self,
        _work_directory: &Path,
        requirement: CatalogMetadataStreamScratchV1,
    ) -> Result<Box<dyn CatalogMetadataStreamAdmission>> {
        self.streams.lock().unwrap().push(requirement);
        Ok(Box::new(RecordingStreamAdmission(Arc::clone(
            &self.stream_chunks,
        ))))
    }

    fn stream_projection_spool(
        &self,
        _work_directory: &Path,
        requirement: CatalogProjectionSpoolScratchV1,
    ) -> Result<Box<dyn CatalogMetadataStreamAdmission>> {
        self.projection_spools.lock().unwrap().push(requirement);
        Ok(Box::new(RecordingStreamAdmission(Arc::clone(
            &self.stream_chunks,
        ))))
    }

    fn reserve_finalization(
        &self,
        _candidate_path: &Path,
        requirement: CatalogFinalizationScratchV2,
    ) -> Result<Box<dyn Send>> {
        self.finalizations.lock().unwrap().push(requirement);
        Ok(Box::new(RecordingLease(Arc::clone(&self.lease_drops))))
    }

    fn reserve_copy(
        &self,
        _destination_root: &Path,
        _requirement: CatalogCopyScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("metadata lease test must not publish a cache copy")
    }
}

fn repository() -> Repository {
    let mut repository = Repository::new(
        "fedora-everything".to_string(),
        "https://example.test/fedora/44/everything/x86_64".to_string(),
    );
    repository.id = Some(7);
    repository.source_profile = Some("fedora-44".to_string());
    repository
        .set_parser_config(RepositoryParserConfig::Rpm {
            architecture: "x86_64".to_string(),
        })
        .unwrap();
    repository
        .set_trust_policy(RepositoryTrustPolicy::Rpm {
            metadata: RpmMetadataAuthority::Metalink {
                url: "https://example.test/metalink".to_string(),
            },
            package_keys: vec![
                OpenPgpTrustRoot::new(
                    "https://example.test/fedora.gpg".to_string(),
                    "A".repeat(40),
                )
                .unwrap(),
            ],
        })
        .unwrap();
    repository
        .set_native_source_policy(
            RepositorySourcePolicy::new(
                "fedora-project",
                RepositoryPolicyScope::repository("fedora-everything-x86_64").unwrap(),
                NativeSourceEcosystem::Rpm,
                NativeSourceStream::release("44").unwrap(),
                RepositoryUpdateMode::Follow,
            )
            .unwrap(),
            "fedora-everything-x86_64",
            None,
        )
        .unwrap();
    repository
}

fn package(repository: &Repository) -> SyncedPackageRow {
    let mut package = PackageMetadata::new(
        "bash".to_string(),
        "5.2.37-1".to_string(),
        "c".repeat(64),
        4096,
        "packages/bash-5.2.37-1.x86_64.rpm".to_string(),
        RepositoryDependencyFlavor::Rpm,
        VersionScheme::Rpm,
    );
    package.architecture = Some("x86_64".to_string());
    synced_package_row(
        repository.id.unwrap(),
        repository.source_profile.as_deref(),
        &repository.url,
        repository.content_url.as_deref(),
        package,
    )
}

fn authenticated_object() -> SourceMetadataObjectV1 {
    let bytes = authenticated_object_bytes();
    SourceMetadataObjectV1 {
        role: SourceMetadataObjectRoleV1::RpmPrimary,
        source_path: "repodata/primary.xml.zst".to_string(),
        sha256: crate::hash::sha256(&bytes),
        size: bytes.len() as u64,
    }
}

fn authenticated_object_bytes() -> Vec<u8> {
    vec![b'x'; 2048]
}

fn durable_source_reuse(
    root: &Path,
    repository: &Repository,
    snapshot: AuthenticatedSnapshotIdentity,
) -> (DurableSourceCatalogReuseV1, AuthenticatedProjectionInputV1) {
    let object = authenticated_object();
    let source = source_catalog_candidate(
        repository,
        vec![package(repository)],
        snapshot,
        vec![object.clone()],
    )
    .unwrap();
    let bundle = root.join("durable-source");
    std::fs::create_dir(&bundle).unwrap();
    let binding = write_catalog_candidate(bundle.join("catalog.sqlite"), source.content()).unwrap();
    let manifest = source.bind(&binding).unwrap();
    let work = tempfile::Builder::new()
        .prefix("durable-source-object-")
        .tempdir_in(root)
        .unwrap();
    let object_path = work.path().join("rpm-primary");
    std::fs::write(&object_path, authenticated_object_bytes()).unwrap();
    retain_source_metadata_object(&bundle, work.path(), &object_path, &object).unwrap();
    drop(write_source_catalog_manifest(&bundle, &manifest).unwrap());
    let projection_input = AuthenticatedProjectionInputV1::with_authenticated_decoded_size(
        object,
        authenticated_object_bytes().len() as u64,
    );
    (
        DurableSourceCatalogReuseV1::new(manifest, bundle).unwrap(),
        projection_input,
    )
}

#[test]
fn private_source_runtime_drives_async_io_without_an_ambient_runtime() {
    let result = drive_native_source_future_on_private_runtime(async {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        Ok::<_, Error>(42)
    })
    .unwrap();

    assert_eq!(result, 42);
}

#[test]
fn immutable_sink_retains_metadata_lease_until_work_files_are_removed() {
    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("catalog.sqlite");
    let lease_drops = Arc::new(AtomicUsize::new(0));
    let admission = Arc::new(RecordingAdmission {
        source_candidates: Mutex::new(Vec::new()),
        finalizations: Mutex::new(Vec::new()),
        metadata: Mutex::new(Vec::new()),
        streams: Mutex::new(Vec::new()),
        projection_spools: Mutex::new(Vec::new()),
        stream_chunks: Arc::new(Mutex::new(Vec::new())),
        lease_drops: Arc::clone(&lease_drops),
        refuse_source: false,
    });
    let mut sink =
        NativeCatalogSnapshotSink::create(&repository(), &candidate, None, Some(admission.clone()))
            .unwrap();
    let work_directory = sink.work_directory().to_path_buf();
    let requirement =
        CatalogMetadataScratchV1::from_signed_objects(vec![CatalogMetadataObjectScratchV1 {
            role: SourceMetadataObjectRoleV1::RpmPrimary,
            source_path: "repodata/primary.xml.zst".to_string(),
            size: 4096,
        }])
        .unwrap();

    sink.reserve_authenticated_metadata(requirement.clone())
        .unwrap();
    assert_eq!(
        admission.metadata.lock().unwrap().as_slice(),
        &[requirement]
    );
    assert_eq!(lease_drops.load(Ordering::SeqCst), 0);
    assert!(work_directory.exists());

    drop(sink);
    assert!(!work_directory.exists());
    assert_eq!(lease_drops.load(Ordering::SeqCst), 1);
}

#[test]
fn immutable_sink_routes_unknown_length_metadata_to_stream_admission() {
    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("catalog.sqlite");
    let admission = Arc::new(RecordingAdmission {
        source_candidates: Mutex::new(Vec::new()),
        finalizations: Mutex::new(Vec::new()),
        metadata: Mutex::new(Vec::new()),
        streams: Mutex::new(Vec::new()),
        projection_spools: Mutex::new(Vec::new()),
        stream_chunks: Arc::new(Mutex::new(Vec::new())),
        lease_drops: Arc::new(AtomicUsize::new(0)),
        refuse_source: false,
    });
    let mut sink =
        NativeCatalogSnapshotSink::create(&repository(), &candidate, None, Some(admission.clone()))
            .unwrap();
    let requirement =
        CatalogMetadataStreamScratchV1::new(SourceMetadataObjectRoleV1::ArchDatabase, "core.db")
            .unwrap();

    let stream = sink
        .streamed_authenticated_metadata(requirement.clone())
        .unwrap();
    let permit = stream.reserve_next(2048).unwrap();
    assert_eq!(admission.streams.lock().unwrap().as_slice(), &[requirement]);
    assert_eq!(admission.stream_chunks.lock().unwrap().as_slice(), &[2048]);
    drop(permit);
}

#[test]
fn native_candidate_admission_precedes_file_creation_and_refusal_leaves_no_file() {
    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("catalog.sqlite");
    let admission = Arc::new(RecordingAdmission {
        source_candidates: Mutex::new(Vec::new()),
        finalizations: Mutex::new(Vec::new()),
        metadata: Mutex::new(Vec::new()),
        streams: Mutex::new(Vec::new()),
        projection_spools: Mutex::new(Vec::new()),
        stream_chunks: Arc::new(Mutex::new(Vec::new())),
        lease_drops: Arc::new(AtomicUsize::new(0)),
        refuse_source: false,
    });
    let repository = repository();
    let mut sink =
        NativeCatalogSnapshotSink::create(&repository, &candidate, None, Some(admission.clone()))
            .unwrap();
    let mut package = PackageMetadata::new(
        "bash".to_string(),
        "5.2.37-1".to_string(),
        "c".repeat(64),
        4096,
        "packages/bash.rpm".to_string(),
        RepositoryDependencyFlavor::Rpm,
        VersionScheme::Rpm,
    );
    let transient_projection = "x".repeat(128 * 1024);
    package.description = Some(transient_projection.clone());

    sink.preflight_package(package.clone()).unwrap();
    assert!(!candidate.exists());
    assert_eq!(
        sink.begin_source_candidate().unwrap(),
        SourceCandidatePreflightOutcome::CompleteProjection {
            provide_update: SnapshotProvideUpdate::default(),
        }
    );
    assert!(candidate.exists());
    let requirement = admission.source_candidates.lock().unwrap()[0];
    assert_eq!(requirement.package_count, 1);
    assert!(requirement.canonical_projection_bytes > transient_projection.len() as u64);
    assert_eq!(admission.projection_spools.lock().unwrap().len(), 1);
    assert!(!admission.stream_chunks.lock().unwrap().is_empty());
    let authenticated_bytes = b"x";
    let authenticated_path = sink.work_directory.path().join("rpm-primary");
    std::fs::write(&authenticated_path, authenticated_bytes).unwrap();
    sink.authenticated_object(
        AuthenticatedMetadataObject {
            role: SourceMetadataObjectRoleV1::RpmPrimary,
            source_path: "repodata/primary.xml.gz".to_string(),
            sha256: crate::hash::sha256(authenticated_bytes),
            size: authenticated_bytes.len() as u64,
        },
        &authenticated_path,
    )
    .unwrap();
    sink.finish(
        &repository,
        AuthenticatedSnapshotIdentity::for_bytes(b"signed repomd"),
    )
    .unwrap();
    assert!(candidate.exists());
    assert_eq!(admission.finalizations.lock().unwrap().len(), 1);

    let refusing = Arc::new(RecordingAdmission {
        source_candidates: Mutex::new(Vec::new()),
        finalizations: Mutex::new(Vec::new()),
        metadata: Mutex::new(Vec::new()),
        streams: Mutex::new(Vec::new()),
        projection_spools: Mutex::new(Vec::new()),
        stream_chunks: Arc::new(Mutex::new(Vec::new())),
        lease_drops: Arc::new(AtomicUsize::new(0)),
        refuse_source: true,
    });
    let refused_candidate = root.path().join("refused.sqlite");
    let mut sink =
        NativeCatalogSnapshotSink::create(&repository, &refused_candidate, None, Some(refusing))
            .unwrap();
    sink.preflight_package(PackageMetadata::new(
        "bash".to_string(),
        "5.2.37-1".to_string(),
        "c".repeat(64),
        4096,
        "packages/bash.rpm".to_string(),
        RepositoryDependencyFlavor::Rpm,
        VersionScheme::Rpm,
    ))
    .unwrap();
    let error = sink.begin_source_candidate().unwrap_err();
    assert!(matches!(error, Error::CatalogScratchCapacity(_)), "{error}");
    assert!(!refused_candidate.exists());
}

#[test]
fn authenticated_root_churn_reuses_exact_child_projection_and_binds_new_root() {
    let root = tempfile::tempdir().unwrap();
    let repository = repository();
    let cached_snapshot = AuthenticatedSnapshotIdentity::for_bytes(b"signed repomd cache revision");
    let refreshed_snapshot =
        AuthenticatedSnapshotIdentity::for_bytes(b"signed repomd refreshed revision");
    let object = authenticated_object();
    let projection_input = AuthenticatedProjectionInputV1::with_authenticated_decoded_size(
        object.clone(),
        authenticated_object_bytes().len() as u64,
    );
    let source = source_catalog_candidate(
        &repository,
        vec![package(&repository)],
        cached_snapshot.clone(),
        vec![object.clone()],
    )
    .unwrap();
    let cached_candidate = root.path().join("cached-source.sqlite");
    let cached_binding = write_catalog_candidate(&cached_candidate, source.content()).unwrap();
    let cache_root = root.path().join("cache");
    super::super::projection_cache::ProjectionCache::open(
        &cache_root,
        repository.stream_binding_sha256.as_deref().unwrap(),
    )
    .unwrap()
    .publish(
        std::slice::from_ref(&projection_input),
        &cached_binding,
        &cached_candidate,
    )
    .unwrap();

    let admission = Arc::new(RecordingAdmission {
        source_candidates: Mutex::new(Vec::new()),
        finalizations: Mutex::new(Vec::new()),
        metadata: Mutex::new(Vec::new()),
        streams: Mutex::new(Vec::new()),
        projection_spools: Mutex::new(Vec::new()),
        stream_chunks: Arc::new(Mutex::new(Vec::new())),
        lease_drops: Arc::new(AtomicUsize::new(0)),
        refuse_source: false,
    });
    let candidate = root.path().join("cache-hit.sqlite");
    let mut sink = NativeCatalogSnapshotSink::create(
        &repository,
        &candidate,
        Some(&cache_root),
        Some(admission.clone()),
    )
    .unwrap();

    assert!(
        sink.reuse_cached_projection(&refreshed_snapshot, std::slice::from_ref(&projection_input),)
            .unwrap(),
        "an authenticated-root change over identical parser inputs must hit"
    );
    assert_eq!(
        std::fs::read(&candidate).unwrap(),
        std::fs::read(&cached_candidate).unwrap()
    );
    let requirement = admission.source_candidates.lock().unwrap()[0];
    assert_eq!(
        requirement.canonical_projection_bytes,
        cached_binding.artifact.size
    );
    assert_eq!(requirement.package_count, cached_binding.counts.packages);
    assert!(admission.finalizations.lock().unwrap().is_empty());
    let object_path = sink.work_directory.path().join("rpm-primary");
    std::fs::write(&object_path, authenticated_object_bytes()).unwrap();
    sink.authenticated_object(object.clone(), &object_path)
        .unwrap();
    let refreshed = sink
        .finish(&repository, refreshed_snapshot.clone())
        .unwrap();
    assert_eq!(
        refreshed.manifest.authenticated_root.sha256,
        refreshed_snapshot.sha256()
    );
    assert_eq!(
        refreshed.manifest.authenticated_root.size,
        refreshed_snapshot.size().unwrap()
    );
    assert_ne!(
        refreshed.manifest.authenticated_root.sha256,
        cached_snapshot.sha256()
    );
    assert!(candidate.exists());
    let retained = candidate
        .parent()
        .unwrap()
        .join(crate::repository::catalog::SOURCE_METADATA_DIRECTORY_NAME)
        .join(&object.sha256);
    assert_eq!(
        std::fs::read(retained).unwrap(),
        authenticated_object_bytes()
    );
    assert!(admission.finalizations.lock().unwrap().is_empty());

    let unadmitted_candidate = root.path().join("cache-hit-without-admission.sqlite");
    let mut unadmitted = NativeCatalogSnapshotSink::create(
        &repository,
        &unadmitted_candidate,
        Some(&cache_root),
        None,
    )
    .unwrap();
    assert!(
        unadmitted
            .reuse_cached_projection(&refreshed_snapshot, std::slice::from_ref(&projection_input),)
            .unwrap()
    );
    assert_eq!(
        std::fs::read(&unadmitted_candidate).unwrap(),
        std::fs::read(&cached_candidate).unwrap()
    );
    let object_path = unadmitted.work_directory.path().join("rpm-primary");
    std::fs::write(&object_path, authenticated_object_bytes()).unwrap();
    unadmitted
        .authenticated_object(object.clone(), &object_path)
        .unwrap();
    unadmitted.finish(&repository, refreshed_snapshot).unwrap();
}

#[test]
fn exact_registered_source_reuse_creates_no_candidate_catalog_or_scratch_reservation() {
    let root = tempfile::tempdir().unwrap();
    let repository = repository();
    let snapshot = AuthenticatedSnapshotIdentity::for_bytes(b"signed repomd durable revision");
    let (reuse, projection_input) =
        durable_source_reuse(root.path(), &repository, snapshot.clone());
    let expected_manifest = reuse.manifest().clone();
    let durable_path = reuse.bundle_path().to_path_buf();
    let candidate_directory = root.path().join("candidate");
    std::fs::create_dir(&candidate_directory).unwrap();
    let candidate = candidate_directory.join("catalog.sqlite");
    let admission = Arc::new(RecordingAdmission {
        source_candidates: Mutex::new(Vec::new()),
        finalizations: Mutex::new(Vec::new()),
        metadata: Mutex::new(Vec::new()),
        streams: Mutex::new(Vec::new()),
        projection_spools: Mutex::new(Vec::new()),
        stream_chunks: Arc::new(Mutex::new(Vec::new())),
        lease_drops: Arc::new(AtomicUsize::new(0)),
        refuse_source: false,
    });
    let mut sink = NativeCatalogSnapshotSink::create_with_reuse(
        &repository,
        &candidate,
        None,
        Some(admission.clone()),
        Some(reuse),
    )
    .unwrap();

    assert!(
        sink.reuse_cached_projection(&snapshot, std::slice::from_ref(&projection_input))
            .unwrap()
    );
    assert!(!candidate.exists());
    assert!(admission.source_candidates.lock().unwrap().is_empty());
    let object_path = sink.work_directory.path().join("rpm-primary");
    std::fs::write(&object_path, authenticated_object_bytes()).unwrap();
    sink.authenticated_object(projection_input.object.clone(), &object_path)
        .unwrap();
    let result = sink.finish(&repository, snapshot).unwrap();

    assert_eq!(result.manifest, expected_manifest);
    assert_eq!(
        result.materialization,
        SourceCatalogMaterializationV1::DurableReuse {
            bundle_path: durable_path,
        }
    );
    assert!(!candidate.exists());
    assert!(admission.finalizations.lock().unwrap().is_empty());
}

#[test]
fn changed_authenticated_root_does_not_reuse_registered_source() {
    let root = tempfile::tempdir().unwrap();
    let repository = repository();
    let registered = AuthenticatedSnapshotIdentity::for_bytes(b"registered repomd revision");
    let changed = AuthenticatedSnapshotIdentity::for_bytes(b"changed repomd revision");
    let (reuse, projection_input) = durable_source_reuse(root.path(), &repository, registered);
    let candidate_directory = root.path().join("candidate");
    std::fs::create_dir(&candidate_directory).unwrap();
    let admission = Arc::new(RecordingAdmission {
        source_candidates: Mutex::new(Vec::new()),
        finalizations: Mutex::new(Vec::new()),
        metadata: Mutex::new(Vec::new()),
        streams: Mutex::new(Vec::new()),
        projection_spools: Mutex::new(Vec::new()),
        stream_chunks: Arc::new(Mutex::new(Vec::new())),
        lease_drops: Arc::new(AtomicUsize::new(0)),
        refuse_source: false,
    });
    let mut sink = NativeCatalogSnapshotSink::create_with_reuse(
        &repository,
        &candidate_directory.join("catalog.sqlite"),
        None,
        Some(admission),
        Some(reuse),
    )
    .unwrap();

    assert!(
        !sink
            .reuse_cached_projection(&changed, std::slice::from_ref(&projection_input))
            .unwrap()
    );
}

#[test]
fn corrupted_registered_source_fails_closed_without_candidate_fallback() {
    let root = tempfile::tempdir().unwrap();
    let repository = repository();
    let snapshot = AuthenticatedSnapshotIdentity::for_bytes(b"registered repomd revision");
    let (reuse, projection_input) =
        durable_source_reuse(root.path(), &repository, snapshot.clone());
    let catalog_path = reuse.bundle_path().join(CATALOG_FILE_NAME);
    let mut bytes = std::fs::read(&catalog_path).unwrap();
    let last = bytes.last_mut().unwrap();
    *last ^= 0xff;
    std::fs::write(&catalog_path, bytes).unwrap();
    let candidate_directory = root.path().join("candidate");
    std::fs::create_dir(&candidate_directory).unwrap();
    let candidate = candidate_directory.join(CATALOG_FILE_NAME);
    let admission = Arc::new(RecordingAdmission {
        source_candidates: Mutex::new(Vec::new()),
        finalizations: Mutex::new(Vec::new()),
        metadata: Mutex::new(Vec::new()),
        streams: Mutex::new(Vec::new()),
        projection_spools: Mutex::new(Vec::new()),
        stream_chunks: Arc::new(Mutex::new(Vec::new())),
        lease_drops: Arc::new(AtomicUsize::new(0)),
        refuse_source: false,
    });
    let mut sink = NativeCatalogSnapshotSink::create_with_reuse(
        &repository,
        &candidate,
        None,
        Some(admission),
        Some(reuse),
    )
    .unwrap();

    let error = sink
        .reuse_cached_projection(&snapshot, std::slice::from_ref(&projection_input))
        .unwrap_err();
    assert!(
        error.to_string().contains("Checksum mismatch")
            || error.to_string().contains("SHA-256")
            || error.to_string().contains("hash"),
        "{error}"
    );
    assert!(!candidate.exists());
}

#[test]
fn exact_native_evidence_becomes_a_bound_source_snapshot_without_operational_ids() {
    let repository = repository();
    let candidate = source_catalog_candidate(
        &repository,
        vec![package(&repository)],
        AuthenticatedSnapshotIdentity::for_bytes(b"signed repomd.xml"),
        vec![authenticated_object()],
    )
    .unwrap();

    assert_eq!(
        candidate.content().scope,
        CatalogScopeV1::Source {
            source_profile: "fedora-44".to_string(),
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        }
    );
    assert_eq!(
        candidate.content().source_evidence,
        vec![CatalogSourceEvidenceV1::AuthenticatedObject {
            role: SourceMetadataObjectRoleV1::RpmPrimary,
            source_path: "repodata/primary.xml.zst".to_string(),
            sha256: authenticated_object().sha256,
            size: authenticated_object().size,
        }]
    );
    assert!(matches!(
        candidate.content().packages[0].origin,
        CatalogPackageOriginV1::Source { .. }
    ));

    let directory = tempfile::tempdir().unwrap();
    let binding =
        write_catalog_candidate(directory.path().join("catalog.sqlite"), candidate.content())
            .unwrap();
    let manifest = candidate.bind(&binding).unwrap();
    assert_eq!(manifest.authenticated_objects, vec![authenticated_object()]);
    assert_eq!(manifest.catalog, binding.artifact);
    assert_eq!(manifest.counts.packages, 1);
}

#[test]
fn legacy_sha_only_root_cannot_claim_byte_complete_source_authority() {
    let repository = repository();
    let error = source_catalog_candidate(
        &repository,
        vec![package(&repository)],
        AuthenticatedSnapshotIdentity::from_sha256("a".repeat(64)).unwrap(),
        vec![authenticated_object()],
    )
    .err()
    .expect("legacy SHA-only identity must fail");
    assert!(error.to_string().contains("no exact byte length"));
}

#[test]
fn pinned_native_root_mismatch_is_refused_before_catalog_construction() {
    let mut repository = repository();
    let pinned = AuthenticatedSnapshotIdentity::for_bytes(b"pinned repomd.xml");
    repository
        .set_native_source_policy(
            RepositorySourcePolicy::new(
                "fedora-project",
                RepositoryPolicyScope::repository("fedora-everything-x86_64").unwrap(),
                NativeSourceEcosystem::Rpm,
                NativeSourceStream::release("44").unwrap(),
                RepositoryUpdateMode::Pin,
            )
            .unwrap(),
            "fedora-everything-x86_64",
            Some(pinned),
        )
        .unwrap();

    let error = source_catalog_candidate(
        &repository,
        vec![package(&repository)],
        AuthenticatedSnapshotIdentity::for_bytes(b"different repomd.xml"),
        vec![authenticated_object()],
    )
    .err()
    .expect("pinned snapshot mismatch must fail");
    assert!(matches!(error, Error::TrustError(_)), "{error}");
}
