// apps/remi/src/server/catalog_capacity.rs

//! Shared filesystem-scoped admission for immutable catalog scratch space.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use conary_core::repository::catalog::{
    CatalogCopyScratchV1, CatalogFinalizationScratchV1, CatalogScratchAdmission,
    CatalogScratchCapacityError,
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
    fn reserve_finalization(
        &self,
        candidate_path: &Path,
        requirement: CatalogFinalizationScratchV1,
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
        let filesystem = filesystem_identity(path)?;
        let available_bytes = self.probe.available_space(path)?;
        let mut state = self.state.lock().map_err(|_| {
            conary_core::Error::InternalError(
                "catalog scratch reservation ledger is poisoned".to_string(),
            )
        })?;
        let reserved_bytes = state.by_filesystem.get(&filesystem).copied().unwrap_or(0);
        let total =
            reserved_bytes
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
        state.by_filesystem.insert(filesystem.clone(), total);
        drop(state);
        Ok(Box::new(CatalogScratchLease {
            filesystem,
            bytes: required_bytes,
            state: Arc::clone(&self.state),
        }))
    }
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

    fn requirement(database_bytes: u64) -> CatalogFinalizationScratchV1 {
        CatalogFinalizationScratchV1::from_page_facts(1, database_bytes).unwrap()
    }

    fn copy_requirement(required_bytes: u64) -> CatalogCopyScratchV1 {
        CatalogCopyScratchV1::from_exact_bytes(required_bytes - 1, 1).unwrap()
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
        let probe = Arc::new(MutableProbe::new(8192));
        let coordinator = CatalogScratchCoordinator::with_probe(probe.clone());
        let lease = coordinator
            .reserve_finalization(&candidate(root.path()), requirement(4096))
            .unwrap();
        drop(lease);

        probe.set(8191);
        let error =
            refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(4096)));
        let conary_core::Error::CatalogScratchCapacity(error) = error else {
            panic!("expected typed catalog capacity refusal");
        };
        assert_eq!(error.required_bytes, 8192);
        assert_eq!(error.available_bytes, 8191);
        assert_eq!(error.reserved_bytes, 0);
    }

    #[test]
    fn concurrent_reservations_cannot_overcommit_and_drop_releases() {
        let root = tempfile::tempdir().unwrap();
        let coordinator = CatalogScratchCoordinator::with_probe(Arc::new(MutableProbe::new(100)));
        let first = coordinator
            .reserve_finalization(&candidate(root.path()), requirement(30))
            .unwrap();
        let error =
            refused(coordinator.reserve_finalization(&candidate(root.path()), requirement(21)));
        let conary_core::Error::CatalogScratchCapacity(error) = error else {
            panic!("expected typed catalog capacity refusal");
        };
        assert_eq!(error.reserved_bytes, 60);

        drop(first);
        coordinator
            .reserve_finalization(&candidate(root.path()), requirement(50))
            .unwrap();
    }

    #[test]
    fn copy_and_finalization_share_one_filesystem_ledger() {
        let root = tempfile::tempdir().unwrap();
        let coordinator = CatalogScratchCoordinator::with_probe(Arc::new(MutableProbe::new(100)));
        let finalizer = coordinator
            .reserve_finalization(&candidate(root.path()), requirement(30))
            .unwrap();
        let error = refused(coordinator.reserve_copy(root.path(), copy_requirement(41)));
        let conary_core::Error::CatalogScratchCapacity(error) = error else {
            panic!("expected typed catalog capacity refusal");
        };
        assert_eq!(error.required_bytes, 41);
        assert_eq!(error.reserved_bytes, 60);

        drop(finalizer);
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
            .reserve_finalization(&candidate(root.path()), requirement(20))
            .unwrap();
        probe.set(69);
        assert!(
            coordinator
                .reserve_finalization(&candidate(root.path()), requirement(15))
                .is_err()
        );
        drop(first);

        let restarted = CatalogScratchCoordinator::with_probe(probe);
        restarted
            .reserve_finalization(&candidate(root.path()), requirement(34))
            .unwrap();
    }
}
