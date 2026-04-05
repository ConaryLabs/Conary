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
#[cfg(feature = "composefs-rs")]
use composefs::fsverity::{FsVerityHashValue, Sha256HashValue};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationMountOutcome {
    ComposefsVerity,
    ComposefsPlain,
    ErofsFallback,
}

impl MountOptions {
    /// Build the argument list for mounting a composefs generation.
    ///
    /// Uses the composefs mount helper (`mount -t composefs`), which takes
    /// the EROFS image as the device and resolves external CAS references
    /// via the `basedir=` option. This matches the dracut boot path in
    /// `packaging/dracut/90conary/conary-generator.sh`.
    ///
    /// Requires the composefs mount helper (part of the composefs userspace
    /// package) and kernel 6.2+ with `CONFIG_EROFS_FS`.
    ///
    /// The returned `Vec<String>` can be passed directly to `Command::args`.
    #[must_use]
    pub fn to_mount_args(&self) -> Vec<String> {
        let mut opts = vec![format!("basedir={}", self.basedir.display())];

        if self.verity {
            opts.push("verity_check=1".to_string());
        }

        if let Some(digest) = &self.digest {
            opts.push(format!("digest={digest}"));
        }

        vec![
            "-t".to_string(),
            "composefs".to_string(),
            self.image_path.to_string_lossy().into_owned(),
            self.mount_point.to_string_lossy().into_owned(),
            "-o".to_string(),
            opts.join(","),
        ]
    }

    /// Build argument list for a plain EROFS loopback mount (fallback).
    ///
    /// Used when composefs mount helper isn't available or for bootstrap
    /// seed mounts. The EROFS image must contain file data inline (not
    /// metadata-only with external CAS references), as plain EROFS has
    /// no CAS resolution.
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

#[cfg(feature = "composefs-rs")]
fn verify_erofs_verity_digest(image_path: &Path, expected_digest: &str) -> crate::Result<()> {
    let image_bytes = std::fs::read(image_path).map_err(|e| {
        Error::IoError(format!(
            "Failed to read EROFS image {} for verity verification: {e}",
            image_path.display()
        ))
    })?;
    let actual_digest =
        composefs::fsverity::compute_verity::<Sha256HashValue>(&image_bytes).to_hex();

    if actual_digest == expected_digest {
        Ok(())
    } else {
        Err(Error::ChecksumMismatch {
            expected: expected_digest.to_string(),
            actual: actual_digest,
        })
    }
}

#[cfg(not(feature = "composefs-rs"))]
fn verify_erofs_verity_digest(_image_path: &Path, _expected_digest: &str) -> crate::Result<()> {
    Err(Error::NotImplemented(
        "EROFS verity verification requires the 'composefs-rs' feature".to_string(),
    ))
}

/// Mount a composefs generation image.
///
/// Uses `mount -t composefs` (the composefs mount helper) which resolves
/// external CAS references via `basedir=`. Falls back to plain EROFS
/// loopback mount if the composefs helper isn't available. Note: the EROFS
/// fallback only works for images with inline file data, not metadata-only
/// images that rely on CAS.
///
/// This function only mounts the composefs image at `mount_point`. The
/// `/etc` overlay must be set up separately by the caller using
/// [`mount_etc_overlay`], because the lower, upper, and target paths
/// differ between staging (composefs at `/conary/mnt`, overlay onto
/// `/etc`) and boot (composefs at `/`, overlay onto `/etc`). The
/// `upperdir` and `workdir` fields on `MountOptions` are retained for
/// informational use but are NOT consumed here.
pub fn mount_generation(opts: &MountOptions) -> crate::Result<GenerationMountOutcome> {
    if let Some(expected_digest) = &opts.digest {
        verify_erofs_verity_digest(&opts.image_path, expected_digest)?;
    }

    let args = opts.to_mount_args();
    let output = Command::new("mount")
        .args(&args)
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| Error::IoError(format!("Failed to execute mount: {e}")))?;

    if output.status.success() {
        info!(
            "Mounted composefs generation at {}",
            opts.mount_point.display()
        );
        return Ok(if opts.verity {
            GenerationMountOutcome::ComposefsVerity
        } else {
            GenerationMountOutcome::ComposefsPlain
        });
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    info!(
        "composefs mount failed ({}), falling back to EROFS loopback for {}",
        stderr.trim(),
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
        Ok(GenerationMountOutcome::ErofsFallback)
    } else {
        Err(Error::IoError(format!(
            "Both composefs and EROFS mount failed for image {}",
            opts.image_path.display()
        )))
    }
}

#[must_use]
pub fn verity_downgrade_warning(
    requested_verity: bool,
    outcome: GenerationMountOutcome,
    image_path: &Path,
) -> Option<String> {
    if !requested_verity {
        return None;
    }

    match outcome {
        GenerationMountOutcome::ComposefsVerity => None,
        GenerationMountOutcome::ComposefsPlain => Some(format!(
            "Mounted generation image {} without fs-verity enforcement after a fallback retry. Integrity protection is downgraded until composefs verity can be restored.",
            image_path.display()
        )),
        GenerationMountOutcome::ErofsFallback => Some(format!(
            "Mounted generation image {} via plain EROFS fallback instead of a verity-enforced composefs mount. Integrity protection is downgraded until composefs verity can be restored.",
            image_path.display()
        )),
    }
}

/// Mount an overlayfs layer for `/etc` on top of a composefs generation.
///
/// This provides a writable `/etc` where user modifications persist across
/// generations in the upper directory.
///
/// Arguments:
/// - `etc_lower`: path to the /etc directory inside the composefs staging
///   mount (e.g. `/conary/mnt/etc`). This is the read-only lower layer.
/// - `etc_target`: path where the overlay should be mounted (e.g. `/etc`).
///   This must be a different path from `etc_lower` -- mounting lowerdir
///   onto itself is invalid.
/// - `upperdir`: persistent directory for user modifications.
/// - `workdir`: overlayfs work directory (must be on the same filesystem
///   as `upperdir`).
pub fn mount_etc_overlay(
    etc_lower: &Path,
    etc_target: &Path,
    upperdir: &Path,
    workdir: &Path,
) -> crate::Result<()> {
    if !etc_lower.exists() {
        return Ok(()); // No /etc in this generation, nothing to overlay.
    }

    std::fs::create_dir_all(upperdir)
        .map_err(|e| Error::IoError(format!("Failed to create etc upperdir: {e}")))?;
    std::fs::create_dir_all(workdir)
        .map_err(|e| Error::IoError(format!("Failed to create etc workdir: {e}")))?;

    let opts = format!(
        "lowerdir={},upperdir={},workdir={}",
        etc_lower.display(),
        upperdir.display(),
        workdir.display()
    );

    let status = Command::new("mount")
        .args([
            "-t",
            "overlay",
            "overlay",
            "-o",
            &opts,
            &etc_target.to_string_lossy(),
        ])
        .status()
        .map_err(|e| Error::IoError(format!("Failed to execute etc overlay mount: {e}")))?;

    if status.success() {
        info!("Mounted /etc overlay at {}", etc_target.display());
        Ok(())
    } else {
        Err(Error::IoError(format!(
            "Failed to mount /etc overlay at {}",
            etc_target.display()
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

/// Check whether a path is currently mounted as a composefs or overlay filesystem.
///
/// Parses `/proc/mounts` to find an entry with filesystem type `composefs`,
/// `overlay`, or `erofs` at the given path.
pub fn is_overlay_mount(path: &Path) -> crate::Result<bool> {
    let mounts = std::fs::read_to_string("/proc/mounts")
        .map_err(|e| Error::IoError(format!("Failed to read /proc/mounts: {e}")))?;

    let path_str = path.to_string_lossy();
    let found = mounts.lines().any(|line| {
        let mut parts = line.split_whitespace();
        let _device = parts.next();
        let mount_point = parts.next();
        let fs_type = parts.next();
        mount_point == Some(path_str.as_ref())
            && matches!(fs_type, Some("composefs" | "overlay" | "erofs"))
    });

    Ok(found)
}

/// Check whether a specific generation's EROFS image is currently mounted.
///
/// More precise than [`is_overlay_mount`]: verifies the device/source in
/// `/proc/mounts` contains the expected EROFS image path. This prevents
/// recovery from short-circuiting when a stale or unrelated generation is
/// mounted at the same mount point.
pub(crate) fn is_generation_mounted(
    mount_point: &Path,
    expected_image: &Path,
) -> crate::Result<bool> {
    let mounts = std::fs::read_to_string("/proc/mounts")
        .map_err(|e| Error::IoError(format!("Failed to read /proc/mounts: {e}")))?;

    let mp_str: &str = &mount_point.to_string_lossy();
    let image_str: &str = &expected_image.to_string_lossy();

    let found = mounts.lines().any(|line| {
        let mut parts = line.split_whitespace();
        let device = parts.next().unwrap_or("");
        let mp = parts.next().unwrap_or("");
        let fs_type = parts.next().unwrap_or("");
        let options = parts.next().unwrap_or("");

        if mp != mp_str {
            return false;
        }

        match fs_type {
            // composefs mount: device is the EROFS image path
            "composefs" => device == image_str,
            // erofs loopback: device is the image path (or a loop device,
            // in which case we check mount options for the path)
            "erofs" => device == image_str || options.contains(image_str),
            // overlay: the image path appears in lowerdir= options
            "overlay" => options.contains(image_str),
            _ => false,
        }
    });

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    #[cfg(feature = "composefs-rs")]
    use tempfile::TempDir;

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
    fn mount_command_uses_composefs() {
        let opts = base_opts();
        let args = opts.to_mount_args();

        // Must use composefs filesystem type
        assert_eq!(args[0], "-t");
        assert_eq!(args[1], "composefs");
        // Image path as device
        assert_eq!(args[2], "/conary/generations/5/root.erofs");
        // Mount point
        assert_eq!(args[3], "/conary/mnt");
        assert_eq!(args[4], "-o");

        let opts_str = &args[5];
        assert!(
            opts_str.contains("basedir=/conary/objects"),
            "basedir missing: {opts_str}"
        );
    }

    #[test]
    fn mount_command_with_verity() {
        let opts = MountOptions {
            verity: true,
            ..base_opts()
        };
        let args = opts.to_mount_args();
        let opts_str = &args[5];
        assert!(
            opts_str.contains("verity_check=1"),
            "verity_check=1 missing"
        );
    }

    #[test]
    fn mount_command_with_digest() {
        let opts = MountOptions {
            verity: true,
            digest: Some("abc123".to_string()),
            ..base_opts()
        };
        let args = opts.to_mount_args();
        let opts_str = &args[5];
        assert!(
            opts_str.contains("digest=abc123"),
            "digest missing: {opts_str}"
        );
    }

    #[test]
    fn mount_command_without_verity() {
        let opts = base_opts();
        let args = opts.to_mount_args();
        let opts_str = &args[5];
        assert!(
            !opts_str.contains("verity"),
            "verity must be absent when not requested"
        );
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

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn verify_erofs_verity_digest_accepts_matching_digest() {
        let tmp = TempDir::new().unwrap();
        let image_path = tmp.path().join("root.erofs");
        std::fs::write(&image_path, b"synthetic erofs bytes").unwrap();
        let digest =
            composefs::fsverity::compute_verity::<Sha256HashValue>(b"synthetic erofs bytes")
                .to_hex();

        verify_erofs_verity_digest(&image_path, &digest).unwrap();
    }

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn verify_erofs_verity_digest_rejects_mismatch() {
        let tmp = TempDir::new().unwrap();
        let image_path = tmp.path().join("root.erofs");
        std::fs::write(&image_path, b"synthetic erofs bytes").unwrap();

        let err = verify_erofs_verity_digest(&image_path, &"00".repeat(32)).unwrap_err();
        assert!(matches!(err, Error::ChecksumMismatch { .. }));
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

    #[test]
    fn verity_downgrade_warning_only_emits_for_real_downgrades() {
        assert!(
            verity_downgrade_warning(
                true,
                GenerationMountOutcome::ComposefsVerity,
                Path::new("/conary/generations/1/root.erofs")
            )
            .is_none()
        );

        let plain = verity_downgrade_warning(
            true,
            GenerationMountOutcome::ComposefsPlain,
            Path::new("/conary/generations/1/root.erofs"),
        )
        .unwrap();
        assert!(plain.contains("downgraded"));

        assert!(
            verity_downgrade_warning(
                false,
                GenerationMountOutcome::ComposefsPlain,
                Path::new("/conary/generations/1/root.erofs")
            )
            .is_none()
        );
    }
}
