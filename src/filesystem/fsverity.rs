// src/filesystem/fsverity.rs

//! fs-verity enablement for CAS objects
//!
//! Enables the Linux fs-verity feature on CAS objects. Once enabled,
//! the kernel computes and caches a Merkle tree hash over the file
//! contents. composefs uses these hashes for integrity verification
//! at read time.

use std::os::unix::io::AsRawFd;
use std::path::Path;

use anyhow::{Context, Result};
use tracing::{debug, warn};

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
pub fn enable_fsverity(path: &Path) -> Result<bool> {
    // Open read-only (fs-verity requires the file not be open for writing)
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open {} for fs-verity", path.display()))?;

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
    let result =
        unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_ENABLE_VERITY, &arg as *const _) };

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
        return Err(anyhow::anyhow!(
            "Filesystem does not support fs-verity: {}",
            path.display()
        ));
    }

    Err(err).with_context(|| format!("Failed to enable fs-verity on {}", path.display()))
}

/// Enable fs-verity on all CAS objects in the given objects directory.
///
/// CAS objects are stored as `objects/{2-char-prefix}/{hash}`.
/// Walks the directory and enables verity on each file.
///
/// Returns `(enabled_count, already_enabled_count, error_count)`.
pub fn enable_fsverity_on_cas(objects_dir: &Path) -> (u64, u64, u64) {
    let mut enabled = 0u64;
    let mut already = 0u64;
    let mut errors = 0u64;

    let entries = match std::fs::read_dir(objects_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("Failed to read CAS objects dir: {}", e);
            return (0, 0, 1);
        }
    };

    for prefix_entry in entries.flatten() {
        if !prefix_entry
            .file_type()
            .map(|ft| ft.is_dir())
            .unwrap_or(false)
        {
            continue;
        }

        let sub_entries = match std::fs::read_dir(prefix_entry.path()) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for file_entry in sub_entries.flatten() {
            if !file_entry
                .file_type()
                .map(|ft| ft.is_file())
                .unwrap_or(false)
            {
                continue;
            }

            match enable_fsverity(&file_entry.path()) {
                Ok(true) => enabled += 1,
                Ok(false) => already += 1,
                Err(e) => {
                    debug!("fs-verity error on {}: {}", file_entry.path().display(), e);
                    errors += 1;
                }
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
