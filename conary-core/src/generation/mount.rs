// conary-core/src/generation/mount.rs

//! Mount and unmount composefs generations.
//!
//! This module owns the composefs mount/unmount logic that was previously
//! scattered across the CLI layer. It provides a `MountOptions` struct for
//! constructing mount(8) argument lists and functions for managing the
//! `/conary/current` symlink.

use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::info;

use crate::error::Error;

/// Options for mounting a composefs generation image.
///
/// Encapsulates all parameters needed to invoke `mount -t composefs`.
/// The `to_mount_args` method produces the full argument list for the
/// mount(8) command.
#[derive(Debug, Clone)]
pub struct MountOptions {
    /// Path to the EROFS image file (the composefs manifest).
    pub image_path: PathBuf,
    /// Base directory for CAS object storage (passed as `basedir=` option).
    pub basedir: PathBuf,
    /// Filesystem path where the generation will be mounted.
    pub mount_point: PathBuf,
    /// Whether to enable fs-verity checking during mount.
    pub verity: bool,
    /// Hex-encoded fs-verity digest of the EROFS image for kernel-enforced
    /// image integrity. Passed as `digest=` mount option when present.
    pub digest: Option<String>,
    /// Upper directory for an overlayfs layer on top of the composefs mount.
    /// Requires `workdir` to also be set.
    pub upperdir: Option<PathBuf>,
    /// Work directory for the overlayfs layer. Required when `upperdir` is set.
    pub workdir: Option<PathBuf>,
}

impl MountOptions {
    /// Build the argument list for mounting a composefs generation.
    ///
    /// composefs is NOT a kernel filesystem type. It uses overlayfs with
    /// an EROFS metadata image as a data-only lower layer and the CAS
    /// directory as the data source. This requires kernel 6.5+ (overlayfs
    /// data-only lower layers) and 6.6+ for fs-verity.
    ///
    /// The mount is: `mount -t overlay overlay -o lowerdir=<erofs>::<cas>,metacopy=on,redirect_dir=on <mountpoint>`
    ///
    /// The returned `Vec<String>` can be passed directly to `Command::args`.
    #[must_use]
    pub fn to_mount_args(&self) -> Vec<String> {
        // The EROFS image is the metadata layer, CAS objects directory is
        // the data layer. The `::` separator creates a data-only lower layer.
        let mut opts = vec![
            format!(
                "lowerdir={}::{}",
                self.image_path.display(),
                self.basedir.display()
            ),
            "metacopy=on".to_string(),
            "redirect_dir=on".to_string(),
        ];

        if self.verity {
            opts.push("verity=require".to_string());
        }

        if let Some(upperdir) = &self.upperdir {
            opts.push(format!("upperdir={}", upperdir.display()));
        }

        if let Some(workdir) = &self.workdir {
            opts.push(format!("workdir={}", workdir.display()));
        }

        vec![
            "-t".to_string(),
            "overlay".to_string(),
            "overlay".to_string(),
            "-o".to_string(),
            opts.join(","),
            self.mount_point.to_string_lossy().into_owned(),
        ]
    }

    /// Build argument list for a plain EROFS loopback mount (fallback).
    ///
    /// Used when overlayfs data-only layers aren't available (kernel < 6.5)
    /// or when the CAS directory isn't relevant (bootstrap seed mounts).
    /// The EROFS image must contain file data inline, not just metadata.
    #[must_use]
    pub fn to_erofs_mount_args(&self) -> Vec<String> {
        vec![
            "-t".to_string(),
            "erofs".to_string(),
            "-o".to_string(),
            "loop,ro".to_string(),
            self.image_path.to_string_lossy().into_owned(),
            self.mount_point.to_string_lossy().into_owned(),
        ]
    }
}

/// Mount a composefs generation image.
///
/// Tries the full composefs experience first (overlayfs with EROFS metadata +
/// CAS data-only layer, kernel 6.5+). Falls back to plain EROFS loopback
/// mount if overlayfs data-only layers aren't supported.
pub fn mount_generation(opts: &MountOptions) -> crate::Result<()> {
    // Try overlayfs composefs mount first (kernel 6.5+)
    let args = opts.to_mount_args();
    let status = Command::new("mount")
        .args(&args)
        .stderr(std::process::Stdio::piped())
        .status()
        .map_err(|e| Error::IoError(format!("Failed to execute mount: {e}")))?;

    if status.success() {
        info!(
            "Mounted composefs generation (overlayfs+EROFS) at {}",
            opts.mount_point.display()
        );
        return Ok(());
    }

    // Fallback: plain EROFS loopback mount (kernel 5.15+)
    info!(
        "Overlayfs composefs mount failed, falling back to EROFS loopback for {}",
        opts.image_path.display()
    );
    let erofs_args = opts.to_erofs_mount_args();
    let erofs_status = Command::new("mount")
        .args(&erofs_args)
        .status()
        .map_err(|e| Error::IoError(format!("Failed to execute EROFS mount: {e}")))?;

    if erofs_status.success() {
        info!(
            "Mounted generation (EROFS loopback) at {}",
            opts.mount_point.display()
        );
        Ok(())
    } else {
        Err(Error::IoError(format!(
            "Both overlayfs and EROFS mount failed for image {}",
            opts.image_path.display()
        )))
    }
}

/// Unmount a composefs generation mount point.
///
/// Runs `umount <mount_point>`. Returns an error if the command fails
/// to execute or exits non-zero.
pub fn unmount_generation(mount_point: &Path) -> crate::Result<()> {
    let status = Command::new("umount")
        .arg(mount_point)
        .status()
        .map_err(|e| Error::IoError(format!("Failed to execute umount: {e}")))?;

    if status.success() {
        info!(
            "Unmounted composefs generation at {}",
            mount_point.display()
        );
        Ok(())
    } else {
        Err(Error::IoError(format!(
            "umount exited with status {status} for mount point {}",
            mount_point.display()
        )))
    }
}

/// Atomically update the `/conary/current` symlink to point to the given generation.
///
/// Creates a temporary symlink next to the target link and renames it
/// atomically over the existing one. The `conary_root` argument is typically
/// `/conary`; the symlink will be at `<conary_root>/current` and will point
/// to `<conary_root>/generations/<generation_number>`.
pub fn update_current_symlink(conary_root: &Path, generation_number: i64) -> crate::Result<()> {
    let link = conary_root.join("current");
    let target = symlink_target_for_generation(generation_number);
    let tmp_link = conary_root.join("current.tmp");

    // Remove stale temp link if it exists from a previous interrupted update.
    let _ = std::fs::remove_file(&tmp_link);

    std::os::unix::fs::symlink(&target, &tmp_link).map_err(|e| {
        Error::IoError(format!(
            "Failed to create temp symlink {} -> {}: {e}",
            tmp_link.display(),
            target.display()
        ))
    })?;

    std::fs::rename(&tmp_link, &link).map_err(|e| {
        Error::IoError(format!(
            "Failed to rename {} to {}: {e}",
            tmp_link.display(),
            link.display()
        ))
    })?;

    info!("Updated {} -> {}", link.display(), target.display());
    Ok(())
}

/// Read the current generation number from the `<conary_root>/current` symlink.
///
/// Returns `Ok(None)` if the symlink does not exist (no active generation).
/// Returns an error if the symlink exists but its target cannot be parsed.
pub fn current_generation(conary_root: &Path) -> crate::Result<Option<i64>> {
    let link = conary_root.join("current");

    if !link.exists() {
        return Ok(None);
    }

    let target = std::fs::read_link(&link)
        .map_err(|e| Error::IoError(format!("Failed to read symlink {}: {e}", link.display())))?;

    let component = target
        .file_name()
        .ok_or_else(|| {
            Error::ParseError(format!(
                "Symlink target has no filename: {}",
                target.display()
            ))
        })?
        .to_string_lossy()
        .into_owned();

    let gen_number: i64 = component.parse().map_err(|_| {
        Error::ParseError(format!(
            "Failed to parse generation number from '{component}'"
        ))
    })?;

    Ok(Some(gen_number))
}

/// Return the relative symlink target for a generation number.
///
/// For example, generation 5 returns `generations/5`.
/// This is a relative path suitable for use as a symlink target from
/// the conary root directory.
#[must_use]
pub fn symlink_target_for_generation(n: i64) -> PathBuf {
    PathBuf::from(format!("generations/{n}"))
}

/// Check whether a path is currently mounted as an overlay filesystem.
///
/// Parses `/proc/mounts` to find an entry with filesystem type `overlay`
/// at the given path. Returns `false` (not `Err`) if `/proc/mounts` cannot
/// be read, as this is treated as "unknown, assume not overlay".
pub fn is_overlay_mount(path: &Path) -> crate::Result<bool> {
    let mounts = std::fs::read_to_string("/proc/mounts")
        .map_err(|e| Error::IoError(format!("Failed to read /proc/mounts: {e}")))?;

    let path_str = path.to_string_lossy();
    let found = mounts.lines().any(|line| {
        let mut parts = line.split_whitespace();
        let _device = parts.next();
        let mount_point = parts.next();
        let fs_type = parts.next();
        mount_point == Some(path_str.as_ref()) && fs_type == Some("overlay")
    });

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn base_opts() -> MountOptions {
        MountOptions {
            image_path: PathBuf::from("/conary/generations/5/root.erofs"),
            basedir: PathBuf::from("/conary/objects"),
            mount_point: PathBuf::from("/conary/mnt"),
            verity: false,
            digest: None,
            upperdir: None,
            workdir: None,
        }
    }

    #[test]
    fn mount_command_uses_overlayfs() {
        let opts = base_opts();
        let args = opts.to_mount_args();

        // Must use overlay filesystem type
        assert_eq!(args[0], "-t");
        assert_eq!(args[1], "overlay");
        assert_eq!(args[2], "overlay");
        assert_eq!(args[3], "-o");

        let opts_str = &args[4];
        // lowerdir uses :: separator for data-only layer
        assert!(
            opts_str.contains("lowerdir=/conary/generations/5/root.erofs::/conary/objects"),
            "lowerdir with data-only layer missing: {opts_str}"
        );
        assert!(opts_str.contains("metacopy=on"), "metacopy=on missing");
        assert!(
            opts_str.contains("redirect_dir=on"),
            "redirect_dir=on missing"
        );

        assert_eq!(args[5], "/conary/mnt");
    }

    #[test]
    fn mount_command_with_verity() {
        let opts = MountOptions {
            verity: true,
            ..base_opts()
        };
        let args = opts.to_mount_args();
        let opts_str = &args[4];
        assert!(
            opts_str.contains("verity=require"),
            "verity=require missing"
        );
    }

    #[test]
    fn mount_command_without_verity() {
        let opts = base_opts();
        let args = opts.to_mount_args();
        let opts_str = &args[4];
        assert!(
            !opts_str.contains("verity"),
            "verity must be absent when not requested"
        );
    }

    #[test]
    fn mount_command_with_upperdir() {
        let opts = MountOptions {
            upperdir: Some(PathBuf::from("/overlay/upper")),
            workdir: Some(PathBuf::from("/overlay/work")),
            ..base_opts()
        };

        let args = opts.to_mount_args();
        let opts_str = &args[4];
        assert!(opts_str.contains("upperdir=/overlay/upper"));
        assert!(opts_str.contains("workdir=/overlay/work"));
    }

    #[test]
    fn erofs_fallback_args() {
        let opts = base_opts();
        let args = opts.to_erofs_mount_args();

        assert_eq!(args[0], "-t");
        assert_eq!(args[1], "erofs");
        assert_eq!(args[2], "-o");
        assert_eq!(args[3], "loop,ro");
        assert_eq!(args[4], "/conary/generations/5/root.erofs");
        assert_eq!(args[5], "/conary/mnt");
    }

    #[test]
    fn symlink_target_path() {
        let target = symlink_target_for_generation(5);
        assert_eq!(target, PathBuf::from("generations/5"));
    }

    #[test]
    fn current_generation_missing() {
        // Use a temp directory that has no "current" symlink.
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let result = current_generation(tmp.path());
        assert!(result.is_ok(), "should return Ok for missing symlink");
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn current_generation_roundtrip() {
        let tmp = tempfile::TempDir::new().expect("temp dir");

        // Create a generations/7 directory so the symlink target exists.
        let gen_dir = tmp.path().join("generations").join("7");
        std::fs::create_dir_all(&gen_dir).unwrap();

        // Write the symlink: current -> generations/7
        update_current_symlink(tmp.path(), 7).expect("update_current_symlink");

        let n = current_generation(tmp.path())
            .expect("current_generation")
            .expect("should be Some");
        assert_eq!(n, 7);
    }

    #[test]
    fn update_current_symlink_is_idempotent() {
        let tmp = tempfile::TempDir::new().expect("temp dir");

        std::fs::create_dir_all(tmp.path().join("generations").join("1")).unwrap();
        std::fs::create_dir_all(tmp.path().join("generations").join("2")).unwrap();

        update_current_symlink(tmp.path(), 1).expect("first update");
        update_current_symlink(tmp.path(), 2).expect("second update");

        let n = current_generation(tmp.path())
            .expect("current_generation")
            .expect("Some");
        assert_eq!(n, 2, "second update should win");
    }
}
