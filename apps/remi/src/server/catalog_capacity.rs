// apps/remi/src/server/catalog_capacity.rs

//! Shared filesystem-scoped admission for immutable catalog scratch space.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use conary_core::repository::catalog::{
    CatalogCopyScratchV1, CatalogFinalizationScratchV2, CatalogMetadataScratchV1,
    CatalogMetadataStreamAdmission, CatalogMetadataStreamScratchV1,
    CatalogProfileCandidateScratchV1, CatalogScratchAdmission, CatalogScratchCapacityError,
    CatalogSourceCandidateScratchV1,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum FilesystemIdentity {
    #[cfg(unix)]
    Device(u64),
    #[cfg(not(unix))]
    Root(std::path::PathBuf),
}

trait AvailableSpaceProbe: Send + Sync {
    fn available_space(&self, path: &Path) -> std::io::Result<u64>;
}

struct HostAvailableSpace;

impl AvailableSpaceProbe for HostAvailableSpace {
    fn available_space(&self, path: &Path) -> std::io::Result<u64> {
        fs2::available_space(path)
    }
}

#[derive(Default)]
struct ReservationState {
    by_filesystem: BTreeMap<FilesystemIdentity, u64>,
}

/// Shared process-local ledger for transient catalog allocations.
pub(crate) struct CatalogScratchCoordinator {
    probe: Arc<dyn AvailableSpaceProbe>,
    state: Arc<Mutex<ReservationState>>,
}

impl Default for CatalogScratchCoordinator {
    fn default() -> Self {
        Self {
            probe: Arc::new(HostAvailableSpace),
            state: Arc::new(Mutex::new(ReservationState::default())),
        }
    }
}

impl CatalogScratchCoordinator {
    #[cfg(test)]
    fn with_probe(probe: Arc<dyn AvailableSpaceProbe>) -> Self {
        Self {
            probe,
            state: Arc::new(Mutex::new(ReservationState::default())),
        }
    }
}

impl CatalogScratchAdmission for CatalogScratchCoordinator {
    fn reserve_source_candidate(
        &self,
        candidate_path: &Path,
        requirement: CatalogSourceCandidateScratchV1,
    ) -> conary_core::Result<Box<dyn Send>> {
        let parent = candidate_path.parent().ok_or_else(|| {
            conary_core::Error::InvalidPath(
                "source catalog candidate has no parent for growth admission".to_string(),
            )
        })?;
        requirement.validate()?;
        self.reserve_on_filesystem(parent, requirement.required_additional_bytes)
    }

    fn reserve_profile_candidate(
        &self,
        candidate_path: &Path,
        requirement: CatalogProfileCandidateScratchV1,
    ) -> conary_core::Result<Box<dyn Send>> {
        let parent = candidate_path.parent().ok_or_else(|| {
            conary_core::Error::InvalidPath(
                "profile catalog candidate has no parent for growth admission".to_string(),
            )
        })?;
        requirement.validate()?;
        self.reserve_on_filesystem(parent, requirement.required_additional_bytes)
    }

    fn reserve_metadata(
        &self,
        work_directory: &Path,
        requirement: CatalogMetadataScratchV1,
    ) -> conary_core::Result<Box<dyn Send>> {
        requirement.validate()?;
        self.reserve_on_filesystem(work_directory, requirement.required_additional_bytes)
    }

    fn stream_metadata(
        &self,
        work_directory: &Path,
        requirement: CatalogMetadataStreamScratchV1,
    ) -> conary_core::Result<Box<dyn CatalogMetadataStreamAdmission>> {
        requirement.validate()?;
        filesystem_identity(work_directory)?;
        Ok(Box::new(CatalogMetadataStreamCoordinator {
            path: work_directory.to_path_buf(),
            probe: Arc::clone(&self.probe),
            state: Arc::clone(&self.state),
        }))
    }

    fn reserve_finalization(
        &self,
        candidate_path: &Path,
        requirement: CatalogFinalizationScratchV2,
    ) -> conary_core::Result<Box<dyn Send>> {
        let parent = candidate_path.parent().ok_or_else(|| {
            conary_core::Error::InvalidPath(
                "catalog candidate has no parent for scratch admission".to_string(),
            )
        })?;
        requirement.validate()?;
        self.reserve_on_filesystem(parent, requirement.required_additional_bytes)
    }

    fn reserve_copy(
        &self,
        destination_root: &Path,
        requirement: CatalogCopyScratchV1,
    ) -> conary_core::Result<Box<dyn Send>> {
        requirement.validate()?;
        self.reserve_on_filesystem(destination_root, requirement.required_additional_bytes)
    }
}

impl CatalogScratchCoordinator {
    fn reserve_on_filesystem(
        &self,
        path: &Path,
        required_bytes: u64,
    ) -> conary_core::Result<Box<dyn Send>> {
        reserve_exact_bytes(&self.probe, &self.state, path, required_bytes)
    }
}

struct CatalogMetadataStreamCoordinator {
    path: std::path::PathBuf,
    probe: Arc<dyn AvailableSpaceProbe>,
    state: Arc<Mutex<ReservationState>>,
}

impl CatalogMetadataStreamAdmission for CatalogMetadataStreamCoordinator {
    fn reserve_next(&self, additional_bytes: u64) -> conary_core::Result<Box<dyn Send>> {
        if additional_bytes == 0 {
            return Err(conary_core::Error::ConfigError(
                "catalog metadata stream chunk admission requires positive bytes".to_string(),
            ));
        }
        reserve_exact_bytes(&self.probe, &self.state, &self.path, additional_bytes)
    }
}

fn reserve_exact_bytes(
    probe: &Arc<dyn AvailableSpaceProbe>,
    state: &Arc<Mutex<ReservationState>>,
    path: &Path,
    required_bytes: u64,
) -> conary_core::Result<Box<dyn Send>> {
    let filesystem = filesystem_identity(path)?;
    let available_bytes = probe.available_space(path)?;
    let mut state_guard = state.lock().map_err(|_| {
        conary_core::Error::InternalError(
            "catalog scratch reservation ledger is poisoned".to_string(),
        )
    })?;
    let reserved_bytes = state_guard
        .by_filesystem
        .get(&filesystem)
        .copied()
        .unwrap_or(0);
    let total = reserved_bytes
        .checked_add(required_bytes)
        .ok_or(CatalogScratchCapacityError {
            required_bytes,
            available_bytes,
            reserved_bytes,
        })?;
    if total > available_bytes {
        return Err(CatalogScratchCapacityError {
            required_bytes,
            available_bytes,
            reserved_bytes,
        }
        .into());
    }
    state_guard.by_filesystem.insert(filesystem.clone(), total);
    drop(state_guard);
    Ok(Box::new(CatalogScratchLease {
        filesystem,
        bytes: required_bytes,
        state: Arc::clone(state),
    }))
}

struct CatalogScratchLease {
    filesystem: FilesystemIdentity,
    bytes: u64,
    state: Arc<Mutex<ReservationState>>,
}

impl Drop for CatalogScratchLease {
    fn drop(&mut self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(reserved) = state.by_filesystem.get_mut(&self.filesystem) else {
            return;
        };
        *reserved = reserved.saturating_sub(self.bytes);
        if *reserved == 0 {
            state.by_filesystem.remove(&self.filesystem);
        }
    }
}

#[cfg(unix)]
fn filesystem_identity(path: &Path) -> conary_core::Result<FilesystemIdentity> {
    use std::os::unix::fs::MetadataExt;

    Ok(FilesystemIdentity::Device(std::fs::metadata(path)?.dev()))
}

#[cfg(not(unix))]
fn filesystem_identity(path: &Path) -> conary_core::Result<FilesystemIdentity> {
    Ok(FilesystemIdentity::Root(path.canonicalize()?))
}

#[cfg(test)]
mod tests {
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
        let error =
            refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(2)));
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
        let error =
            refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(2)));
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
        let error =
            refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(41)));
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
        let error =
            refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(41)));
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
}
