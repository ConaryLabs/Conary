// conary-core/src/repository/sync/projection_cache/tests.rs

use super::*;
use crate::repository::catalog::{
    CatalogCandidateWriter, CatalogFinalizationScratchV2, CatalogMetadataScratchV1,
    CatalogMetadataStreamAdmission, CatalogMetadataStreamScratchV1,
    CatalogProjectionSpoolScratchV1, CatalogScopeV1, CatalogScratchCapacityError,
    SourceMetadataObjectRoleV1, logical_verification_passes_for_test,
    physical_verification_passes_for_test,
};
use crate::repository::parsers::AuthenticatedMetadataObject;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct RecordingAdmission {
    copies: Mutex<Vec<CatalogCopyScratchV1>>,
    lease_drops: Arc<AtomicUsize>,
    refuse: bool,
}

struct RecordingLease(Arc<AtomicUsize>);

impl Drop for RecordingLease {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl CatalogScratchAdmission for RecordingAdmission {
    fn reserve_source_candidate(
        &self,
        _candidate_path: &Path,
        _requirement: crate::repository::catalog::CatalogSourceCandidateScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("projection cache must not request source growth admission")
    }

    fn reserve_profile_candidate(
        &self,
        _candidate_path: &Path,
        _requirement: crate::repository::catalog::CatalogProfileCandidateScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("projection cache must not request profile growth admission")
    }

    fn reserve_metadata(
        &self,
        _work_directory: &Path,
        _requirement: CatalogMetadataScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("projection cache must not request metadata admission")
    }

    fn stream_metadata(
        &self,
        _work_directory: &Path,
        _requirement: CatalogMetadataStreamScratchV1,
    ) -> Result<Box<dyn CatalogMetadataStreamAdmission>> {
        panic!("projection cache must not request streamed metadata admission")
    }

    fn stream_projection_spool(
        &self,
        _work_directory: &Path,
        _requirement: CatalogProjectionSpoolScratchV1,
    ) -> Result<Box<dyn CatalogMetadataStreamAdmission>> {
        panic!("projection cache must not request projection spool admission")
    }

    fn reserve_finalization(
        &self,
        _candidate_path: &Path,
        _requirement: CatalogFinalizationScratchV2,
    ) -> Result<Box<dyn Send>> {
        panic!("projection cache must not request finalization admission")
    }

    fn reserve_copy(
        &self,
        _destination_root: &Path,
        requirement: CatalogCopyScratchV1,
    ) -> Result<Box<dyn Send>> {
        self.copies.lock().unwrap().push(requirement);
        if self.refuse {
            return Err(CatalogScratchCapacityError {
                required_bytes: requirement.required_additional_bytes,
                available_bytes: requirement.required_additional_bytes - 1,
                reserved_bytes: 0,
            }
            .into());
        }
        Ok(Box::new(RecordingLease(Arc::clone(&self.lease_drops))))
    }
}

fn recording_admission(refuse: bool) -> Arc<RecordingAdmission> {
    Arc::new(RecordingAdmission {
        copies: Mutex::new(Vec::new()),
        lease_drops: Arc::new(AtomicUsize::new(0)),
        refuse,
    })
}

struct Fixture {
    _root: tempfile::TempDir,
    cache: ProjectionCache,
    inputs: Vec<AuthenticatedProjectionInputV1>,
    candidate: PathBuf,
    binding: CatalogBindingV1,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let candidate = root.path().join("candidate.sqlite");
    let object = AuthenticatedMetadataObject {
        role: SourceMetadataObjectRoleV1::DebianPackages,
        source_path: "main/binary-amd64/Packages.xz".to_string(),
        sha256: "b".repeat(64),
        size: 123,
    };
    let evidence = vec![CatalogSourceEvidenceV1::AuthenticatedObject {
        role: object.role.clone(),
        source_path: object.source_path.clone(),
        sha256: object.sha256.clone(),
        size: object.size,
    }];
    let writer = CatalogCandidateWriter::create(
        &candidate,
        CatalogScopeV1::Source {
            source_profile: "ubuntu-26.04".to_string(),
            source_identity: "ubuntu".to_string(),
            repository_identity: "ubuntu-main-amd64".to_string(),
        },
    )
    .unwrap();
    let binding = writer.finish(evidence).unwrap();
    let cache = ProjectionCache::open(&root.path().join("cache"), &"a".repeat(64)).unwrap();
    Fixture {
        _root: root,
        cache,
        inputs: vec![AuthenticatedProjectionInputV1::exact_object(object)],
        candidate,
        binding,
    }
}

#[test]
fn exact_child_projection_key_reopens_verified_catalog_without_native_input() {
    let fixture = fixture();
    fixture
        .cache
        .publish(&fixture.inputs, &fixture.binding, &fixture.candidate)
        .unwrap();

    let reader = fixture
        .cache
        .lookup(&fixture.inputs)
        .unwrap()
        .expect("exact cache key should hit");
    assert_eq!(reader.binding(), &fixture.binding);
    assert!(reader.packages().unwrap().is_empty());
}

#[test]
fn root_independent_key_requires_an_authenticated_child() {
    let fixture = fixture();
    let error = match fixture.cache.lookup(&[]) {
        Err(error) => error,
        Ok(_) => panic!("an empty authenticated child set must fail"),
    };
    assert!(
        error
            .to_string()
            .contains("requires at least one authenticated child object")
    );
    assert_eq!(fs::read_dir(&fixture.cache.root).unwrap().count(), 0);
}

#[test]
fn verified_projection_publication_rehashes_the_copied_artifact() {
    let fixture = fixture();
    let verified = CatalogReader::open_verified(&fixture.candidate, &fixture.binding).unwrap();
    fixture
        .cache
        .publish_verified(
            &fixture.inputs,
            &fixture.binding,
            &fixture.candidate,
            &verified,
        )
        .unwrap();
    let logical_passes_after_publication = logical_verification_passes_for_test();

    let reader = fixture
        .cache
        .lookup(&fixture.inputs)
        .unwrap()
        .expect("verified publication must remain a normal durable cache hit");
    assert_eq!(reader.binding(), &fixture.binding);
    assert!(reader.verification_proof().is_ok());
    assert_eq!(
        logical_verification_passes_for_test(),
        logical_passes_after_publication,
        "durable cache lookup must not replay normalized catalog rows"
    );
}

#[test]
fn exact_materialization_preserves_existing_paths_and_removes_failed_copies() {
    let fixture = fixture();
    fixture
        .cache
        .publish(&fixture.inputs, &fixture.binding, &fixture.candidate)
        .unwrap();
    let reader = fixture.cache.lookup(&fixture.inputs).unwrap().unwrap();

    let materialized = fixture._root.path().join("materialized.sqlite");
    let physical_passes = physical_verification_passes_for_test();
    let materialized_reader = fixture
        .cache
        .materialize_verified(&reader, &materialized)
        .unwrap();
    assert_eq!(
        materialized_reader.path(),
        materialized.canonicalize().unwrap()
    );
    assert_eq!(
        physical_verification_passes_for_test(),
        physical_passes + 1,
        "materialization must perform one complete candidate proof"
    );
    drop(materialized_reader);

    let existing = fixture._root.path().join("existing.sqlite");
    fs::write(&existing, b"belongs to another operation").unwrap();
    assert!(
        fixture
            .cache
            .materialize_verified(&reader, &existing)
            .is_err()
    );
    assert_eq!(
        fs::read(&existing).unwrap(),
        b"belongs to another operation"
    );

    OpenOptions::new()
        .write(true)
        .open(reader.path())
        .unwrap()
        .write_all(b"x")
        .unwrap();
    let failed = fixture._root.path().join("failed.sqlite");
    assert!(
        fixture
            .cache
            .materialize_verified(&reader, &failed)
            .is_err()
    );
    assert!(!failed.exists());
}

#[test]
fn copy_admission_uses_exact_bytes_and_existing_hit_writes_nothing() {
    let fixture = fixture();
    let admission = recording_admission(false);
    let cache = ProjectionCache::open_with_scratch_admission(
        &fixture.cache.root,
        &"a".repeat(64),
        admission.clone(),
    )
    .unwrap();
    cache
        .publish(&fixture.inputs, &fixture.binding, &fixture.candidate)
        .unwrap();

    let copies = admission.copies.lock().unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].catalog_bytes, fixture.binding.artifact.size);
    assert!(copies[0].manifest_bytes > 0);
    assert_eq!(
        copies[0].required_additional_bytes,
        copies[0].catalog_bytes + copies[0].manifest_bytes
    );
    drop(copies);
    assert_eq!(admission.lease_drops.load(Ordering::SeqCst), 1);

    cache
        .publish(&fixture.inputs, &fixture.binding, &fixture.candidate)
        .unwrap();
    assert_eq!(admission.copies.lock().unwrap().len(), 1);
    assert_eq!(admission.lease_drops.load(Ordering::SeqCst), 1);
}

#[test]
fn one_byte_short_refusal_precedes_projection_cache_mutation() {
    let fixture = fixture();
    let admission = recording_admission(true);
    let cache = ProjectionCache::open_with_scratch_admission(
        &fixture.cache.root,
        &"a".repeat(64),
        admission.clone(),
    )
    .unwrap();
    let error = cache
        .publish(&fixture.inputs, &fixture.binding, &fixture.candidate)
        .unwrap_err();
    let Error::CatalogScratchCapacity(error) = error else {
        panic!("expected typed catalog capacity refusal");
    };
    assert_eq!(error.available_bytes + 1, error.required_bytes);
    assert_eq!(admission.copies.lock().unwrap().len(), 1);
    assert_eq!(admission.lease_drops.load(Ordering::SeqCst), 0);
    assert_eq!(fs::read_dir(&fixture.cache.root).unwrap().count(), 0);
}

#[test]
fn altered_child_or_root_derived_projection_input_cannot_reuse_projection() {
    let fixture = fixture();
    fixture
        .cache
        .publish(&fixture.inputs, &fixture.binding, &fixture.candidate)
        .unwrap();

    let mut changed_digest = fixture.inputs.clone();
    changed_digest[0].object.sha256 = "c".repeat(64);
    assert!(fixture.cache.lookup(&changed_digest).unwrap().is_none());
    let mut changed_role = fixture.inputs.clone();
    changed_role[0].object.role = SourceMetadataObjectRoleV1::RpmPrimary;
    assert!(fixture.cache.lookup(&changed_role).unwrap().is_none());
    let mut changed_path = fixture.inputs.clone();
    changed_path[0].object.source_path = "universe/binary-amd64/Packages.xz".to_string();
    assert!(fixture.cache.lookup(&changed_path).unwrap().is_none());
    let mut changed_decoded_size = fixture.inputs.clone();
    changed_decoded_size[0].authenticated_decoded_size = Some(456);
    assert!(
        fixture
            .cache
            .lookup(&changed_decoded_size)
            .unwrap()
            .is_none()
    );
    let other_binding = ProjectionCache::open(&fixture.cache.root, &"d".repeat(64)).unwrap();
    assert!(other_binding.lookup(&fixture.inputs).unwrap().is_none());
}

#[test]
fn tampered_or_version_mixed_manifest_is_discarded_before_reuse() {
    let fixture = fixture();
    fixture
        .cache
        .publish(&fixture.inputs, &fixture.binding, &fixture.candidate)
        .unwrap();
    let key = fixture.cache.key(&fixture.inputs).unwrap();
    let entry = fixture.cache.entry_path(&key).unwrap();
    let manifest_path = entry.join(MANIFEST_FILE_NAME);
    let mut manifest: ProjectionCacheManifestV3 =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest.schema_version = 1;
    fs::write(
        &manifest_path,
        crate::json::canonical_json(&manifest).unwrap(),
    )
    .unwrap();

    assert!(fixture.cache.lookup(&fixture.inputs).unwrap().is_none());
    assert!(!entry.exists());
}

#[test]
fn altered_durable_logical_attestation_is_discarded_before_reuse() {
    let fixture = fixture();
    fixture
        .cache
        .publish(&fixture.inputs, &fixture.binding, &fixture.candidate)
        .unwrap();
    let key = fixture.cache.key(&fixture.inputs).unwrap();
    let entry = fixture.cache.entry_path(&key).unwrap();
    let manifest_path = entry.join(MANIFEST_FILE_NAME);
    let mut manifest: ProjectionCacheManifestV3 =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest.logical_attestation.catalog_binding_sha256 = "0".repeat(64);
    fs::write(
        &manifest_path,
        crate::json::canonical_json(&manifest).unwrap(),
    )
    .unwrap();

    assert!(fixture.cache.lookup(&fixture.inputs).unwrap().is_none());
    assert!(!entry.exists());
}
