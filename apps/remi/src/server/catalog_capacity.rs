// apps/remi/src/server/catalog_capacity.rs

//! Shared filesystem-scoped admission for immutable catalog scratch space.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use conary_core::repository::catalog::{
    CatalogCopyScratchV1, CatalogFinalizationScratchV2, CatalogMetadataScratchV1,
    CatalogMetadataStreamAdmission, CatalogMetadataStreamScratchV1,
    CatalogProfileCandidateScratchV1, CatalogProjectionSpoolScratchV1, CatalogScratchAdmission,
    CatalogScratchCapacityError, CatalogSourceCandidateScratchV1,
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

    fn stream_projection_spool(
        &self,
        work_directory: &Path,
        requirement: CatalogProjectionSpoolScratchV1,
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
mod tests;
