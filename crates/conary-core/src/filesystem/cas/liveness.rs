// crates/conary-core/src/filesystem/cas/liveness.rs

//! CAS object liveness exclusion shared by generation sealing and collection.

use fs2::FileExt as _;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

const CAS_OBJECT_LIVENESS_LOCK: &str = ".object-liveness.lock";

/// Shared lease preventing Conary's collector from deleting canonical objects.
///
/// Writers may continue publishing immutable objects while this lease is held.
/// Only canonical-object deletion is excluded.
#[derive(Debug)]
pub(crate) struct CasObjectLivenessLease {
    canonical_objects_dir: PathBuf,
    _lock: File,
}

impl CasObjectLivenessLease {
    pub(crate) fn acquire(objects_dir: &Path) -> crate::Result<Self> {
        let (canonical_objects_dir, lock) = open_lock_file(objects_dir)?;
        lock.lock_shared().map_err(|error| {
            crate::Error::IoError(format!(
                "failed to acquire shared CAS object-liveness lock in {}: {error}",
                canonical_objects_dir.display()
            ))
        })?;
        Ok(Self {
            canonical_objects_dir,
            _lock: lock,
        })
    }

    pub(crate) fn canonical_objects_dir(&self) -> &Path {
        &self.canonical_objects_dir
    }
}

/// Exclusive session binding a complete reachability plan to CAS deletion.
///
/// Acquire this before resolving live authority and retain it until collection
/// finishes. Generation presence proofs hold the corresponding shared lease
/// through completion, so a collector either observes the completed generation
/// or finishes before that generation verifies its inputs.
#[derive(Debug)]
pub struct CasObjectCollectionSession {
    canonical_objects_dir: PathBuf,
    _lock: File,
}

impl CasObjectCollectionSession {
    pub fn acquire(objects_dir: &Path) -> crate::Result<Self> {
        let (canonical_objects_dir, lock) = open_lock_file(objects_dir)?;
        lock.lock_exclusive().map_err(|error| {
            crate::Error::IoError(format!(
                "failed to acquire exclusive CAS object-liveness lock in {}: {error}",
                canonical_objects_dir.display()
            ))
        })?;
        Ok(Self {
            canonical_objects_dir,
            _lock: lock,
        })
    }

    pub fn objects_dir(&self) -> &Path {
        &self.canonical_objects_dir
    }
}

fn open_lock_file(objects_dir: &Path) -> crate::Result<(PathBuf, File)> {
    fs::create_dir_all(objects_dir)?;
    let canonical_objects_dir = fs::canonicalize(objects_dir)?;
    let lock_path = canonical_objects_dir.join(CAS_OBJECT_LIVENESS_LOCK);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            crate::Error::IoError(format!(
                "failed to open CAS object-liveness lock {}: {error}",
                lock_path.display()
            ))
        })?;
    Ok((canonical_objects_dir, lock))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_liveness_lease_excludes_collection_session() {
        let tmp = tempfile::tempdir().unwrap();
        let objects_dir = tmp.path().join("objects");
        let lease = CasObjectLivenessLease::acquire(&objects_dir).unwrap();
        let (_, competing_lock) = open_lock_file(&objects_dir).unwrap();

        assert!(competing_lock.try_lock_exclusive().is_err());
        drop(lease);
        competing_lock.try_lock_exclusive().unwrap();
    }
}
