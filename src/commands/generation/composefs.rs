// src/commands/generation/composefs.rs
//! Composefs kernel support detection and preflight checks
// TODO(generation): Remove once generation builder wires up composefs preflight
#![allow(dead_code)]

use std::path::Path;

use anyhow::{Result, anyhow};
use tracing::debug;

/// Capabilities detected during preflight
#[derive(Debug)]
pub struct ComposefsCaps {
    /// Whether fs-verity is supported on the CAS filesystem
    pub fsverity: bool,
}

/// Check if the running kernel supports composefs.
///
/// Checks /proc/filesystems for "composefs" entry, which is present
/// when the composefs kernel module is loaded or built-in.
pub fn supports_composefs() -> bool {
    match std::fs::read_to_string("/proc/filesystems") {
        Ok(contents) => contents.lines().any(|line| line.trim().ends_with("composefs")),
        Err(_) => false,
    }
}

/// Check if fs-verity is supported on the filesystem containing the given path.
///
/// Creates a temporary file and attempts the `FS_IOC_ENABLE_VERITY` ioctl.
/// Returns false (without error) if unsupported.
pub fn supports_fsverity(path: &Path) -> bool {
    use std::os::unix::io::AsRawFd;

    // Create a temp file in the target directory
    let test_path = path.join(".conary-fsverity-test");
    let file = match std::fs::File::create(&test_path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    // Must close and reopen read-only for fs-verity (file must not be open for writing)
    drop(file);

    // Write some content first (fs-verity needs non-empty file on some implementations)
    let _ = std::fs::write(&test_path, b"test");

    let file = match std::fs::File::open(&test_path) {
        Ok(f) => f,
        Err(_) => {
            let _ = std::fs::remove_file(&test_path);
            return false;
        }
    };

    // FS_IOC_ENABLE_VERITY = _IOW('f', 0x85, struct fsverity_enable_arg)
    // On x86_64: 0x40806685
    const FS_IOC_ENABLE_VERITY: libc::c_ulong = 0x4080_6685;

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

    let arg = FsverityEnableArg {
        version: 1,
        hash_algorithm: 1, // FS_VERITY_HASH_ALG_SHA256
        block_size: 4096,
        salt_size: 0,
        salt_ptr: 0,
        sig_size: 0,
        reserved1: 0,
        sig_ptr: 0,
        reserved2: [0; 11],
    };

    let result = unsafe {
        libc::ioctl(file.as_raw_fd(), FS_IOC_ENABLE_VERITY, &arg as *const _)
    };

    let _ = std::fs::remove_file(&test_path);

    // Success (0) or EEXIST means fs-verity is supported
    // ENOTSUP/EOPNOTSUPP means not supported
    if result == 0 {
        return true;
    }

    let err = std::io::Error::last_os_error();
    let errno = err.raw_os_error().unwrap_or(0);
    // EOPNOTSUPP = 95, ENOTSUP is the same on Linux
    debug!("fs-verity probe returned errno {errno}");
    errno != 95 // If not EOPNOTSUPP, verity is probably supported (got a different error)
}

/// Run composefs preflight checks.
///
/// Returns capabilities on success, or an error if composefs is not supported.
pub fn preflight_composefs(cas_dir: &Path) -> Result<ComposefsCaps> {
    if !supports_composefs() {
        return Err(anyhow!(
            "Composefs not supported by running kernel. Requires Linux 6.2+ with CONFIG_EROFS_FS and composefs module."
        ));
    }

    let fsverity = supports_fsverity(cas_dir);
    if !fsverity {
        debug!("fs-verity not supported on CAS filesystem; composefs will work without integrity verification");
    }

    Ok(ComposefsCaps { fsverity })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_composefs_does_not_panic() {
        // Just verify it doesn't panic -- actual result depends on kernel
        let _ = supports_composefs();
    }

    #[test]
    fn test_supports_fsverity_does_not_panic() {
        // Test with a temp directory
        let tmp = tempfile::TempDir::new().unwrap();
        let _ = supports_fsverity(tmp.path());
    }

    #[test]
    fn test_preflight_error_message() {
        // On most dev machines, composefs won't be available
        // Just verify the function returns a proper Result
        let tmp = tempfile::TempDir::new().unwrap();
        let result = preflight_composefs(tmp.path());
        // Either Ok or Err is fine -- we just verify it doesn't panic
        match result {
            Ok(caps) => {
                // If we're on a composefs-capable system, great
                println!("Composefs supported, fsverity: {}", caps.fsverity);
            }
            Err(e) => {
                assert!(e.to_string().contains("Composefs not supported"));
            }
        }
    }
}
