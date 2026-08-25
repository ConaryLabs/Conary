// crates/conary-core/src/repository/sync/immutable_catalog/tests.rs

use super::*;
use crate::db::models::{RepositoryPolicyScope, RepositorySourcePolicy, RepositoryUpdateMode};
use crate::repository::catalog::{
    CatalogCopyScratchV1, CatalogFinalizationScratchV1, CatalogMetadataObjectScratchV1,
    CatalogMetadataScratchV1, CatalogMetadataStreamAdmission, CatalogMetadataStreamScratchV1,
    CatalogPackageOriginV1, CatalogScopeV1, CatalogScratchAdmission, CatalogSourceEvidenceV1,
    SourceMetadataObjectRoleV1, write_catalog_candidate,
};
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::parsers::PackageMetadata;
use crate::repository::sync::synced_package_row;
use crate::repository::versioning::VersionScheme;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct RecordingAdmission {
    metadata: Mutex<Vec<CatalogMetadataScratchV1>>,
    streams: Mutex<Vec<CatalogMetadataStreamScratchV1>>,
    stream_chunks: Arc<Mutex<Vec<u64>>>,
    lease_drops: Arc<AtomicUsize>,
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

    fn reserve_finalization(
        &self,
        _candidate_path: &Path,
        _requirement: CatalogFinalizationScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("metadata lease test must not finalize a catalog")
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
    SourceMetadataObjectV1 {
        role: SourceMetadataObjectRoleV1::RpmPrimary,
        source_path: "repodata/primary.xml.zst".to_string(),
        sha256: "d".repeat(64),
        size: 2048,
    }
}

#[test]
fn immutable_sink_retains_metadata_lease_until_work_files_are_removed() {
    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("catalog.sqlite");
    let lease_drops = Arc::new(AtomicUsize::new(0));
    let admission = Arc::new(RecordingAdmission {
        metadata: Mutex::new(Vec::new()),
        streams: Mutex::new(Vec::new()),
        stream_chunks: Arc::new(Mutex::new(Vec::new())),
        lease_drops: Arc::clone(&lease_drops),
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
        metadata: Mutex::new(Vec::new()),
        streams: Mutex::new(Vec::new()),
        stream_chunks: Arc::new(Mutex::new(Vec::new())),
        lease_drops: Arc::new(AtomicUsize::new(0)),
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
            sha256: "d".repeat(64),
            size: 2048,
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
