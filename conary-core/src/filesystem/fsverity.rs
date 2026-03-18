// conary-core/src/filesystem/fsverity.rs

//! fs-verity enablement for CAS objects
//!
//! Enables the Linux fs-verity feature on CAS objects. Once enabled,
//! the kernel computes and caches a Merkle tree hash over the file
//! contents. composefs uses these hashes for integrity verification
//! at read time.

use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, warn};

/// Errors that can occur during fs-verity operations
#[derive(Debug, Error)]
pub enum FsVerityError {
    /// Failed to open the file for fs-verity enablement
    #[error("Failed to open {path} for fs-verity: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Filesystem does not support fs-verity
    #[error("Filesystem does not support fs-verity: {0}")]
    NotSupported(PathBuf),

    /// ioctl failed with an unexpected error
    #[error("Failed to enable fs-verity on {path}: {source}")]
    IoctlFailed {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// `FS_IOC_ENABLE_VERITY` ioctl number
/// `_IOW('f', 0x85, struct fsverity_enable_arg)` = `0x40806685`
const FS_IOC_ENABLE_VERITY: libc::c_ulong = 0x4080_6685;

/// `FS_VERITY_HASH_ALG_SHA256`
const FS_VERITY_HASH_ALG_SHA256: u32 = 1;

/// Kernel struct for the fs-verity enable ioctl.
#[repr(C)]
struct FsverityEnableArg {
    version: u32,
    hash_algorithm: u32,
    block_size: u32,
    salt_size: u32,
    salt_ptr: u64,
    sig_size: u32,
    reserved1: u32,
    sig_ptr: u64,
    reserved2: [u64; 11],
}

/// Enable fs-verity on a single file.
///
/// Returns `Ok(true)` if verity was newly enabled, `Ok(false)` if already
/// enabled, or an error if the operation fails for a reason other than
/// "already enabled".
pub fn enable_fsverity(path: &Path) -> Result<bool, FsVerityError> {
    // Open read-only (fs-verity requires the file not be open for writing)
    let file = std::fs::File::open(path).map_err(|e| FsVerityError::Open {
        path: path.to_path_buf(),
        source: e,
    })?;

    let arg = FsverityEnableArg {
        version: 1,
        hash_algorithm: FS_VERITY_HASH_ALG_SHA256,
        block_size: 4096,
        salt_size: 0,
        salt_ptr: 0,
        sig_size: 0,
        reserved1: 0,
        sig_ptr: 0,
        reserved2: [0; 11],
    };

    // SAFETY: We pass a properly initialized FsverityEnableArg struct to the
    // FS_IOC_ENABLE_VERITY ioctl on a valid file descriptor.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_ENABLE_VERITY, &arg as *const _) };

    if result == 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    let errno = err.raw_os_error().unwrap_or(0);

    // EEXIST (17) = already enabled, that's fine
    if errno == libc::EEXIST {
        return Ok(false);
    }

    // EOPNOTSUPP (95) = filesystem doesn't support verity
    if errno == libc::EOPNOTSUPP {
        return Err(FsVerityError::NotSupported(path.to_path_buf()));
    }

    Err(FsVerityError::IoctlFailed {
        path: path.to_path_buf(),
        source: err,
    })
}

/// Enable fs-verity on all CAS objects in the given objects directory.
///
/// Uses `CasStore::iter_objects()` to walk the directory and enables verity
/// on each file.
///
/// Returns `(enabled_count, already_enabled_count, error_count)`.
pub fn enable_fsverity_on_cas(objects_dir: &Path) -> (u64, u64, u64) {
    let mut enabled = 0u64;
    let mut already = 0u64;
    let mut errors = 0u64;

    let cas = match super::CasStore::new(objects_dir) {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to open CAS objects dir: {}", e);
            return (0, 0, 1);
        }
    };

    for result in cas.iter_objects() {
        let (_hash, path) = match result {
            Ok(v) => v,
            Err(e) => {
                debug!("fs-verity: error iterating CAS: {}", e);
                errors += 1;
                continue;
            }
        };

        match enable_fsverity(&path) {
            Ok(true) => enabled += 1,
            Ok(false) => already += 1,
            Err(e) => {
                debug!("fs-verity error on {}: {}", path.display(), e);
                errors += 1;
            }
        }
    }

    debug!(
        "fs-verity: {} enabled, {} already, {} errors",
        enabled, already, errors
    );

    (enabled, already, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fsverity_enable_arg_size() {
        // Verify struct layout matches kernel expectations
        assert_eq!(
            std::mem::size_of::<FsverityEnableArg>(),
            128,
            "FsverityEnableArg must be 128 bytes to match kernel struct"
        );
    }

    #[test]
    fn test_enable_fsverity_nonexistent_file() {
        let result = enable_fsverity(Path::new("/nonexistent/file"));
        assert!(result.is_err());
    }

    #[test]
    fn test_enable_fsverity_on_cas_empty_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (enabled, already, errors) = enable_fsverity_on_cas(tmp.path());
        assert_eq!(enabled, 0);
        assert_eq!(already, 0);
        assert_eq!(errors, 0);
    }
}
