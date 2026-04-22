// conary-core/src/generation/composefs.rs

//! Kernel composefs and fs-verity capability detection.
//!
//! Provides runtime checks for the composefs kernel/userspace contract and
//! fs-verity filesystem support. Used by the generation builder to decide
//! whether composefs generation mounts are actually possible.

use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;

use crate::error::{Error, Result};
use tracing::debug;

use crate::filesystem::fsverity::{FsVerityError, enable_fsverity};

/// Capabilities detected during preflight.
#[derive(Debug)]
pub struct ComposefsCaps {
    /// Whether fs-verity is supported on the CAS filesystem.
    pub fsverity: bool,
    /// Resolved path to the composefs mount helper.
    pub mount_helper: PathBuf,
}

const COMPOSEFS_HELPER_CANDIDATES: &[&str] = &[
    "/usr/sbin/mount.composefs",
    "/usr/bin/mount.composefs",
    "/sbin/mount.composefs",
    "/bin/mount.composefs",
];

fn has_required_composefs_filesystems(proc_filesystems: &str) -> bool {
    let mut has_overlay = false;
    let mut has_erofs = false;

    for line in proc_filesystems.lines() {
        let fs_name = line.split_whitespace().last().unwrap_or_default();
        match fs_name {
            "overlay" => has_overlay = true,
            "erofs" => has_erofs = true,
            _ => {}
        }
    }

    has_overlay && has_erofs
}

fn find_mount_composefs_in_path_with_fallbacks(
    path_env: Option<&OsStr>,
    fallback_candidates: &[&str],
) -> Option<PathBuf> {
    if let Some(path_env) = path_env {
        for dir in std::env::split_paths(path_env) {
            let candidate = dir.join("mount.composefs");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    fallback_candidates
        .iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

fn find_mount_composefs_in_path(path_env: Option<&OsStr>) -> Option<PathBuf> {
    find_mount_composefs_in_path_with_fallbacks(path_env, COMPOSEFS_HELPER_CANDIDATES)
}

fn has_loop_device_support() -> bool {
    Path::new("/dev/loop-control").exists()
        || std::fs::read_dir("/dev")
            .ok()
            .into_iter()
            .flat_map(|entries| entries.flatten())
            .any(|entry| entry.file_name().to_string_lossy().starts_with("loop"))
}

fn check_composefs_runtime_support(
    proc_filesystems: &str,
    path_env: Option<&OsStr>,
    loop_device_available: bool,
) -> std::result::Result<PathBuf, String> {
    if !has_required_composefs_filesystems(proc_filesystems) {
        return Err(
            "running kernel is missing overlayfs and/or EROFS support required for composefs mounts"
                .to_string(),
        );
    }

    if !loop_device_available {
        return Err(
            "running system is missing loop-device support required for composefs metadata images"
                .to_string(),
        );
    }

    find_mount_composefs_in_path(path_env).ok_or_else(|| {
        "mount.composefs helper not found in PATH or standard sbin/bin locations".to_string()
    })
}

/// Check if the running kernel/userspace can support composefs mounts.
///
/// Modern composefs uses EROFS under the hood (not a separate filesystem type).
/// A truthful runtime therefore requires:
/// - EROFS support in the running kernel
/// - overlayfs support in the running kernel
/// - loop-device support for file-backed metadata images
/// - the `mount.composefs` userspace helper in the runtime environment
#[must_use]
pub fn supports_composefs() -> bool {
    let proc_filesystems = match std::fs::read_to_string("/proc/filesystems") {
        Ok(contents) => contents,
        Err(_) => return false,
    };

    check_composefs_runtime_support(
        &proc_filesystems,
        std::env::var_os("PATH").as_deref(),
        has_loop_device_support(),
    )
    .is_ok()
}

/// Check if fs-verity is supported on the filesystem containing the given path.
///
/// Creates a temporary probe file and calls `enable_fsverity()` from the
/// canonical implementation in `crate::filesystem::fsverity`. Returns false
/// (without error) if the filesystem does not support verity.
#[must_use]
pub fn supports_fsverity(path: &Path) -> bool {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    // Use a unique temp file name to avoid races.
    let pid = std::process::id();
    let test_path = path.join(format!(".conary-fsverity-probe-{pid}"));

    // Create with O_CREAT|O_EXCL|O_WRONLY to avoid following a symlink
    // that an attacker might have placed at the probe path. O_EXCL fails
    // if the path already exists (including as a symlink).
    let probe_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true) // O_CREAT | O_EXCL
        .mode(0o600)
        .open(&test_path);

    let mut file = match probe_file {
        Ok(f) => f,
        Err(_) => return false,
    };

    // fs-verity needs non-empty file on some implementations.
    if file.write_all(b"verity-probe").is_err() {
        let _ = std::fs::remove_file(&test_path);
        return false;
    }
    drop(file); // Close write handle so enable_fsverity can open read-only.

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
    let proc_filesystems = std::fs::read_to_string("/proc/filesystems").map_err(|e| {
        Error::IoError(format!(
            "Failed to read /proc/filesystems while checking composefs runtime support: {e}"
        ))
    })?;
    let mount_helper = check_composefs_runtime_support(
        &proc_filesystems,
        std::env::var_os("PATH").as_deref(),
        has_loop_device_support(),
    )
    .map_err(|reason| {
        Error::IoError(format!(
            "Composefs runtime support incomplete: {reason}. \
             Conary requires overlayfs + EROFS + loop-device support and the mount.composefs helper."
        ))
    })?;

    let fsverity = supports_fsverity(cas_dir);
    if !fsverity {
        debug!(
            "fs-verity not supported on CAS filesystem; \
             composefs will work without integrity verification"
        );
    }

    Ok(ComposefsCaps {
        fsverity,
        mount_helper,
    })
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
    fn test_composefs_runtime_support_requires_overlay_and_erofs() {
        let helper_dir = tempfile::TempDir::new().unwrap();
        let helper_path = helper_dir.path().join("mount.composefs");
        std::fs::write(&helper_path, "#!/bin/sh\n").unwrap();

        assert!(
            check_composefs_runtime_support(
                "nodev\toverlay\nnodev\terofs\n",
                Some(helper_dir.path().as_os_str()),
                true,
            )
            .is_ok()
        );

        let err = check_composefs_runtime_support(
            "nodev\terofs\n",
            Some(helper_dir.path().as_os_str()),
            true,
        )
        .unwrap_err();
        assert!(err.contains("overlayfs"));
    }

    #[test]
    fn test_composefs_runtime_support_requires_mount_helper() {
        let err = find_mount_composefs_in_path_with_fallbacks(None, &[])
            .ok_or_else(|| {
                "mount.composefs helper not found in PATH or standard sbin/bin locations"
                    .to_string()
            })
            .unwrap_err();
        assert!(err.contains("mount.composefs"));
    }

    #[test]
    fn test_composefs_runtime_support_requires_loop_devices() {
        let helper_dir = tempfile::TempDir::new().unwrap();
        let helper_path = helper_dir.path().join("mount.composefs");
        std::fs::write(&helper_path, "#!/bin/sh\n").unwrap();

        let err = check_composefs_runtime_support(
            "nodev\toverlay\nnodev\terofs\n",
            Some(helper_dir.path().as_os_str()),
            false,
        )
        .unwrap_err();
        assert!(err.contains("loop-device"));
    }

    #[test]
    fn test_supports_fsverity_does_not_panic() {
        // Test with a temp directory
        let tmp = tempfile::TempDir::new().unwrap();
        let _ = supports_fsverity(tmp.path());
    }

    #[test]
    fn test_composefs_caps_debug() {
        // Verify ComposefsCaps derives Debug without panic
        let caps = ComposefsCaps {
            fsverity: true,
            mount_helper: PathBuf::from("/usr/sbin/mount.composefs"),
        };
        let debug_str = format!("{caps:?}");
        assert!(debug_str.contains("fsverity: true"));
        assert!(debug_str.contains("mount.composefs"));
    }
}
