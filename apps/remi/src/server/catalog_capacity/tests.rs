// apps/remi/src/server/catalog_capacity/tests.rs

use super::*;
use conary_core::repository::catalog::{
    CatalogMetadataObjectScratchV1, CatalogProfileMemberScratchV1, SourceMetadataObjectRoleV1,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct MutableProbe(AtomicU64);

impl MutableProbe {
    fn new(bytes: u64) -> Self {
        Self(AtomicU64::new(bytes))
    }

    fn set(&self, bytes: u64) {
        self.0.store(bytes, Ordering::SeqCst);
    }
}

impl AvailableSpaceProbe for MutableProbe {
    fn available_space(&self, _path: &Path) -> std::io::Result<u64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

fn requirement(database_bytes: u64) -> CatalogFinalizationScratchV2 {
    CatalogFinalizationScratchV2::from_page_facts(1, database_bytes).unwrap()
}

fn copy_requirement(required_bytes: u64) -> CatalogCopyScratchV1 {
    CatalogCopyScratchV1::from_exact_bytes(required_bytes - 1, 1).unwrap()
}

fn metadata_requirement(required_bytes: u64) -> CatalogMetadataScratchV1 {
    CatalogMetadataScratchV1::from_signed_objects(vec![CatalogMetadataObjectScratchV1 {
        role: SourceMetadataObjectRoleV1::DebianPackages,
        source_path: "main/binary-amd64/Packages.gz".to_string(),
        size: required_bytes,
    }])
    .unwrap()
}

fn stream_requirement() -> CatalogMetadataStreamScratchV1 {
    CatalogMetadataStreamScratchV1::new(SourceMetadataObjectRoleV1::ArchDatabase, "core.db")
        .unwrap()
}

fn profile_requirement() -> CatalogProfileCandidateScratchV1 {
    CatalogProfileCandidateScratchV1::from_members(vec![CatalogProfileMemberScratchV1 {
        ordinal: 0,
        catalog_bytes: 4096,
        package_count: 1,
    }])
    .unwrap()
}

fn source_requirement() -> CatalogSourceCandidateScratchV1 {
    CatalogSourceCandidateScratchV1::from_projection_facts(4096, 1).unwrap()
}

fn candidate(root: &Path) -> PathBuf {
    root.join("candidate.sqlite3")
}

fn refused(result: conary_core::Result<Box<dyn Send>>) -> conary_core::Error {
    match result {
        Ok(_) => panic!("expected catalog scratch refusal"),
        Err(error) => error,
    }
}

#[test]
fn exact_bound_succeeds_and_one_byte_short_is_typed() {
    let root = tempfile::tempdir().unwrap();
    let probe = Arc::new(MutableProbe::new(4096));
    let coordinator = CatalogScratchCoordinator::with_probe(probe.clone());
    let lease = coordinator
        .reserve_finalization(&candidate(root.path()), requirement(4096))
        .unwrap();
    drop(lease);

    probe.set(4095);
    let error =
        refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(4096)));
    let conary_core::Error::CatalogScratchCapacity(error) = error else {
        panic!("expected typed catalog capacity refusal");
    };
    assert_eq!(error.required_bytes, 4096);
    assert_eq!(error.available_bytes, 4095);
    assert_eq!(error.reserved_bytes, 0);
}

#[test]
fn profile_growth_exact_bound_and_shared_ledger_are_typed() {
    let root = tempfile::tempdir().unwrap();
    let profile_scratch = profile_requirement();
    let probe = Arc::new(MutableProbe::new(profile_scratch.required_additional_bytes));
    let coordinator = CatalogScratchCoordinator::with_probe(probe.clone());
    let lease = coordinator
        .reserve_profile_candidate(&candidate(root.path()), profile_scratch.clone())
        .unwrap();
    drop(lease);

    probe.set(profile_scratch.required_additional_bytes - 1);
    let error = refused(
        coordinator.reserve_profile_candidate(&candidate(root.path()), profile_scratch.clone()),
    );
    let conary_core::Error::CatalogScratchCapacity(error) = error else {
        panic!("expected typed profile growth refusal");
    };
    assert_eq!(
        error.required_bytes,
        profile_scratch.required_additional_bytes
    );
    assert_eq!(
        error.available_bytes,
        profile_scratch.required_additional_bytes - 1
    );
    assert_eq!(error.reserved_bytes, 0);

    probe.set(profile_scratch.required_additional_bytes + 1);
    let growth = coordinator
        .reserve_profile_candidate(&candidate(root.path()), profile_scratch.clone())
        .unwrap();
    let error = refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(2)));
    let conary_core::Error::CatalogScratchCapacity(error) = error else {
        panic!("expected shared-ledger capacity refusal");
    };
    assert_eq!(error.required_bytes, 2);
    assert_eq!(
        error.reserved_bytes,
        profile_scratch.required_additional_bytes
    );
    drop(growth);
    coordinator
        .reserve_finalization(&candidate(root.path()), requirement(2))
        .unwrap();
}

#[test]
fn source_growth_refuses_one_byte_short_and_shares_the_filesystem_ledger() {
    let root = tempfile::tempdir().unwrap();
    let source_scratch = source_requirement();
    let probe = Arc::new(MutableProbe::new(source_scratch.required_additional_bytes));
    let coordinator = CatalogScratchCoordinator::with_probe(probe.clone());
    let lease = coordinator
        .reserve_source_candidate(&candidate(root.path()), source_scratch)
        .unwrap();
    drop(lease);

    probe.set(source_scratch.required_additional_bytes - 1);
    let error =
        refused(coordinator.reserve_source_candidate(&candidate(root.path()), source_scratch));
    let conary_core::Error::CatalogScratchCapacity(error) = error else {
        panic!("expected typed source growth refusal");
    };
    assert_eq!(
        error.required_bytes,
        source_scratch.required_additional_bytes
    );
    assert_eq!(
        error.available_bytes,
        source_scratch.required_additional_bytes - 1
    );
    assert_eq!(error.reserved_bytes, 0);

    probe.set(source_scratch.required_additional_bytes + 1);
    let growth = coordinator
        .reserve_source_candidate(&candidate(root.path()), source_scratch)
        .unwrap();
    let error = refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(2)));
    let conary_core::Error::CatalogScratchCapacity(error) = error else {
        panic!("expected shared-ledger capacity refusal");
    };
    assert_eq!(
        error.reserved_bytes,
        source_scratch.required_additional_bytes
    );
    drop(growth);
    coordinator
        .reserve_finalization(&candidate(root.path()), requirement(2))
        .unwrap();
}

#[test]
fn metadata_exact_bound_succeeds_and_one_byte_short_precedes_staging() {
    let root = tempfile::tempdir().unwrap();
    let probe = Arc::new(MutableProbe::new(4096));
    let coordinator = CatalogScratchCoordinator::with_probe(probe.clone());
    let lease = coordinator
        .reserve_metadata(root.path(), metadata_requirement(4096))
        .unwrap();
    drop(lease);

    probe.set(4095);
    let error = refused(coordinator.reserve_metadata(root.path(), metadata_requirement(4096)));
    let conary_core::Error::CatalogScratchCapacity(error) = error else {
        panic!("expected typed catalog capacity refusal");
    };
    assert_eq!(error.required_bytes, 4096);
    assert_eq!(error.available_bytes, 4095);
    assert_eq!(error.reserved_bytes, 0);
    assert!(root.path().read_dir().unwrap().next().is_none());
}

#[test]
fn streamed_chunk_exact_bound_and_concurrent_permit_are_typed() {
    let root = tempfile::tempdir().unwrap();
    let probe = Arc::new(MutableProbe::new(4096));
    let coordinator = CatalogScratchCoordinator::with_probe(probe.clone());
    let stream = coordinator
        .stream_metadata(root.path(), stream_requirement())
        .unwrap();
    let permit = stream.reserve_next(4096).unwrap();

    let error = refused(stream.reserve_next(1));
    let conary_core::Error::CatalogScratchCapacity(error) = error else {
        panic!("expected typed catalog capacity refusal");
    };
    assert_eq!(error.required_bytes, 1);
    assert_eq!(error.available_bytes, 4096);
    assert_eq!(error.reserved_bytes, 4096);

    drop(permit);
    probe.set(4095);
    let error = refused(stream.reserve_next(4096));
    let conary_core::Error::CatalogScratchCapacity(error) = error else {
        panic!("expected typed catalog capacity refusal");
    };
    assert_eq!(error.required_bytes, 4096);
    assert_eq!(error.available_bytes, 4095);
    assert_eq!(error.reserved_bytes, 0);
}

#[test]
fn concurrent_reservations_cannot_overcommit_and_drop_releases() {
    let root = tempfile::tempdir().unwrap();
    let coordinator = CatalogScratchCoordinator::with_probe(Arc::new(MutableProbe::new(100)));
    let first = coordinator
        .reserve_finalization(&candidate(root.path()), requirement(60))
        .unwrap();
    let error = refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(41)));
    let conary_core::Error::CatalogScratchCapacity(error) = error else {
        panic!("expected typed catalog capacity refusal");
    };
    assert_eq!(error.reserved_bytes, 60);

    drop(first);
    coordinator
        .reserve_finalization(&candidate(root.path()), requirement(100))
        .unwrap();
}

#[test]
fn metadata_copy_and_finalization_share_one_filesystem_ledger() {
    let root = tempfile::tempdir().unwrap();
    let coordinator = CatalogScratchCoordinator::with_probe(Arc::new(MutableProbe::new(100)));
    let metadata = coordinator
        .reserve_metadata(root.path(), metadata_requirement(30))
        .unwrap();
    let copy = coordinator
        .reserve_copy(root.path(), copy_requirement(30))
        .unwrap();
    let error = refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(41)));
    let conary_core::Error::CatalogScratchCapacity(error) = error else {
        panic!("expected typed catalog capacity refusal");
    };
    assert_eq!(error.required_bytes, 41);
    assert_eq!(error.reserved_bytes, 60);

    drop(metadata);
    drop(copy);
    coordinator
        .reserve_copy(root.path(), copy_requirement(100))
        .unwrap();
}

#[test]
fn each_admission_observes_changed_free_space_and_restart_has_no_stale_lease() {
    let root = tempfile::tempdir().unwrap();
    let probe = Arc::new(MutableProbe::new(100));
    let coordinator = CatalogScratchCoordinator::with_probe(probe.clone());
    let first = coordinator
        .reserve_finalization(&candidate(root.path()), requirement(40))
        .unwrap();
    probe.set(69);
    assert!(
        coordinator
            .reserve_finalization(&candidate(root.path()), requirement(30))
            .is_err()
    );
    drop(first);

    let restarted = CatalogScratchCoordinator::with_probe(probe);
    restarted
        .reserve_finalization(&candidate(root.path()), requirement(68))
        .unwrap();
}
