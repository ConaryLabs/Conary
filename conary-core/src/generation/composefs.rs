// conary-core/src/generation/composefs.rs

//! Kernel composefs and fs-verity capability detection.
//!
//! Provides runtime checks for EROFS/composefs kernel support and
//! fs-verity filesystem support. Used by the generation builder to
//! decide whether to enable integrity verification.

use std::path::Path;

use anyhow::{Result, anyhow};
use tracing::debug;

use crate::filesystem::fsverity::{FsVerityError, enable_fsverity};

/// Capabilities detected during preflight.
#[derive(Debug)]
pub struct ComposefsCaps {
    /// Whether fs-verity is supported on the CAS filesystem.
    pub fsverity: bool,
}

/// Check if the running kernel supports composefs.
///
/// Modern composefs uses EROFS under the hood (not a separate filesystem type).
/// Checks /proc/filesystems for "erofs" entry, which is present when the
/// erofs kernel module is loaded or built-in (`CONFIG_EROFS_FS`).
#[must_use]
pub fn supports_composefs() -> bool {
    match std::fs::read_to_string("/proc/filesystems") {
        Ok(contents) => contents.lines().any(|line| line.trim().ends_with("erofs")),
        Err(_) => false,
    }
}

/// Check if fs-verity is supported on the filesystem containing the given path.
///
/// Creates a temporary probe file and calls `enable_fsverity()` from the
/// canonical implementation in `crate::filesystem::fsverity`. Returns false
/// (without error) if the filesystem does not support verity.
#[must_use]
pub fn supports_fsverity(path: &Path) -> bool {
    // Use a unique temp file name to avoid races
    let pid = std::process::id();
    let test_path = path.join(format!(".conary-fsverity-probe-{pid}"));

    // Write content (fs-verity needs non-empty file on some implementations),
    // then close the write handle so enable_fsverity can open read-only.
    if std::fs::write(&test_path, b"verity-probe").is_err() {
        return false;
    }

    let result = enable_fsverity(&test_path);
    let _ = std::fs::remove_file(&test_path);

    match result {
        // Verity was successfully enabled on the probe file
        Ok(true) => true,
        // Already enabled (shouldn't happen on a fresh file, but means supported)
        Ok(false) => true,
        // Filesystem does not support fs-verity
        Err(FsVerityError::NotSupported(_)) => false,
        // Could not open the probe file (race / permissions)
        Err(FsVerityError::Open { .. }) => false,
        // Other ioctl error -- likely means verity is supported but something
        // else went wrong (e.g., EBUSY, EPERM). Conservative: treat as supported.
        Err(FsVerityError::IoctlFailed { .. }) => {
            debug!("fs-verity probe got unexpected ioctl error, assuming supported");
            true
        }
    }
}

/// Run composefs preflight checks.
///
/// Returns capabilities on success, or an error if composefs is not supported.
pub fn preflight_composefs(cas_dir: &Path) -> Result<ComposefsCaps> {
    if !supports_composefs() {
        return Err(anyhow!(
            "Composefs not supported by running kernel. \
             Requires Linux 6.2+ with CONFIG_EROFS_FS and composefs module."
        ));
    }

    let fsverity = supports_fsverity(cas_dir);
    if !fsverity {
        debug!(
            "fs-verity not supported on CAS filesystem; \
             composefs will work without integrity verification"
        );
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
        // On most dev machines, composefs won't be available.
        // Just verify the function returns a proper Result.
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

    #[test]
    fn test_composefs_caps_debug() {
        // Verify ComposefsCaps derives Debug without panic
        let caps = ComposefsCaps { fsverity: true };
        let debug_str = format!("{caps:?}");
        assert!(debug_str.contains("fsverity: true"));
    }
}
