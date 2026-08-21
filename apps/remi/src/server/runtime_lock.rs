// apps/remi/src/server/runtime_lock.rs

//! Process-wide ownership of one Remi runtime root.

use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

const RUNTIME_LOCK_FILE: &str = ".remi-runtime.lock";

#[derive(Debug, thiserror::Error)]
pub(crate) enum RuntimeRootLockError {
    #[error("cannot create Remi runtime root {root:?}: {source}")]
    CreateRoot {
        root: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot resolve Remi runtime root {root:?}: {source}")]
    ResolveRoot {
        root: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot open Remi runtime lock {lock_path:?}: {source}")]
    OpenLock {
        lock_path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Remi runtime root {root:?} is already owned by another process through {lock_path:?}")]
    AlreadyOwned { root: PathBuf, lock_path: PathBuf },
    #[error("cannot acquire Remi runtime lock {lock_path:?}: {source}")]
    AcquireLock {
        lock_path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// An exclusive kernel lock retained for the full server lifetime.
#[derive(Debug)]
pub(crate) struct RuntimeRootLock {
    _file: File,
    root: PathBuf,
    lock_path: PathBuf,
}

impl RuntimeRootLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self, RuntimeRootLockError> {
        std::fs::create_dir_all(root).map_err(|source| RuntimeRootLockError::CreateRoot {
            root: root.to_path_buf(),
            source,
        })?;
        let root =
            std::fs::canonicalize(root).map_err(|source| RuntimeRootLockError::ResolveRoot {
                root: root.to_path_buf(),
                source,
            })?;
        let lock_path = root.join(RUNTIME_LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        }
        let file = options
            .open(&lock_path)
            .map_err(|source| RuntimeRootLockError::OpenLock {
                lock_path: lock_path.clone(),
                source,
            })?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Self {
                _file: file,
                root,
                lock_path,
            }),
            Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                Err(RuntimeRootLockError::AlreadyOwned { root, lock_path })
            }
            Err(source) => Err(RuntimeRootLockError::AcquireLock { lock_path, source }),
        }
    }

    pub(crate) fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_runtime_root_has_one_owner_and_drop_releases_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let first = RuntimeRootLock::acquire(&root).unwrap();

        let error = RuntimeRootLock::acquire(&root).unwrap_err();
        assert!(matches!(error, RuntimeRootLockError::AlreadyOwned { .. }));
        assert_eq!(
            first.lock_path(),
            std::fs::canonicalize(&root)
                .unwrap()
                .join(RUNTIME_LOCK_FILE)
        );
        assert_eq!(first.root(), std::fs::canonicalize(&root).unwrap());

        drop(first);
        RuntimeRootLock::acquire(&root).unwrap();
    }

    #[test]
    fn distinct_runtime_roots_can_be_owned_concurrently() {
        let temp = tempfile::tempdir().unwrap();
        let first = RuntimeRootLock::acquire(&temp.path().join("first")).unwrap();
        let second = RuntimeRootLock::acquire(&temp.path().join("second")).unwrap();

        assert_ne!(first.lock_path(), second.lock_path());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_of_one_runtime_root_share_the_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let alias = temp.path().join("runtime-alias");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let first = RuntimeRootLock::acquire(&root).unwrap();

        let error = RuntimeRootLock::acquire(&alias).unwrap_err();

        assert!(matches!(error, RuntimeRootLockError::AlreadyOwned { .. }));
        assert_eq!(
            first.lock_path(),
            std::fs::canonicalize(alias)
                .unwrap()
                .join(RUNTIME_LOCK_FILE)
        );
    }

    #[cfg(unix)]
    #[test]
    fn lock_file_symlinks_are_rejected_instead_of_followed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        std::fs::create_dir(&root).unwrap();
        let target = temp.path().join("unrelated");
        File::create(&target).unwrap();
        std::os::unix::fs::symlink(&target, root.join(RUNTIME_LOCK_FILE)).unwrap();

        let error = RuntimeRootLock::acquire(&root).unwrap_err();

        assert!(matches!(error, RuntimeRootLockError::OpenLock { .. }));
    }
}
