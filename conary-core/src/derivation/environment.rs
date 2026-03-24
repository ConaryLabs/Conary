// conary-core/src/derivation/environment.rs

//! Build environment lifecycle for CAS-layered bootstrap derivations.
//!
//! A `BuildEnvironment` manages a composefs mount used as the build sysroot
//! for a derivation. It creates the mount point directory, mounts the EROFS
//! image via composefs, and unmounts on drop.

use std::path::PathBuf;
use std::process::Command;

use tracing::{info, warn};

use crate::generation::mount::MountOptions;

/// Errors specific to build environment mount/unmount operations.
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("mount failed: {0}")]
    Mount(String),
    #[error("unmount failed: {0}")]
    Unmount(String),
}

/// A composefs mount used as a build sysroot for a derivation.
///
/// Owns the lifecycle of a single mount: construct with `new`, call `mount()`
/// to bring it up, and either call `unmount()` explicitly or let `Drop` handle
/// cleanup.
pub struct BuildEnvironment {
    /// Where the composefs image is mounted (the sysroot for builds).
    pub mount_point: PathBuf,
    /// Path to the EROFS image file.
    pub image_path: PathBuf,
    /// CAS object directory passed as `basedir` to composefs.
    pub cas_dir: PathBuf,
    /// SHA-256 hash identifying this build environment image.
    pub build_env_hash: String,
    /// Whether the composefs image is currently mounted.
    mounted: bool,
}

impl BuildEnvironment {
    /// Construct a new build environment (not yet mounted).
    #[must_use]
    pub fn new(
        image_path: PathBuf,
        cas_dir: PathBuf,
        mount_point: PathBuf,
        build_env_hash: String,
    ) -> Self {
        Self {
            mount_point,
            image_path,
            cas_dir,
            build_env_hash,
            mounted: false,
        }
    }

    /// Mount the composefs image at the configured mount point.
    ///
    /// Creates the mount point directory if it does not exist, then invokes
    /// `mount -t composefs` via the generation mount infrastructure.
    ///
    /// # Errors
    ///
    /// Returns `EnvironmentError::Mount` if directory creation fails or the
    /// mount command exits non-zero.
    pub fn mount(&mut self) -> Result<(), EnvironmentError> {
        if self.mounted {
            return Ok(());
        }

        // Ensure the mount point directory exists.
        std::fs::create_dir_all(&self.mount_point).map_err(|e| {
            EnvironmentError::Mount(format!(
                "failed to create mount point {}: {e}",
                self.mount_point.display()
            ))
        })?;

        let opts = MountOptions {
            image_path: self.image_path.clone(),
            basedir: self.cas_dir.clone(),
            mount_point: self.mount_point.clone(),
            verity: false,
            digest: None,
            upperdir: None,
            workdir: None,
        };

        // mount_generation tries overlayfs composefs first, falls back to
        // plain EROFS loopback automatically.
        crate::generation::mount::mount_generation(&opts).map_err(|e| {
            EnvironmentError::Mount(format!("mount failed: {e}"))
        })?;

        info!(
            "Mounted build environment at {} (hash: {})",
            self.mount_point.display(),
            self.build_env_hash,
        );
        self.mounted = true;
        Ok(())
    }

    /// Unmount the composefs image.
    ///
    /// Uses `nix::mount::umount2` with `MNT_DETACH` for a lazy unmount,
    /// falling back to `umount(8)` if the nix call fails.
    ///
    /// # Errors
    ///
    /// Returns `EnvironmentError::Unmount` if both the nix call and the
    /// command fallback fail.
    pub fn unmount(&mut self) -> Result<(), EnvironmentError> {
        if !self.mounted {
            return Ok(());
        }

        // Try nix::mount::umount2 first (lazy detach).
        let nix_result = nix::mount::umount2(&self.mount_point, nix::mount::MntFlags::MNT_DETACH);

        if let Err(nix_err) = nix_result {
            // Fallback to umount(8) command.
            warn!(
                "nix umount2 failed ({}), falling back to umount command",
                nix_err,
            );

            let status = Command::new("umount")
                .arg(&self.mount_point)
                .status()
                .map_err(|e| EnvironmentError::Unmount(format!("failed to execute umount: {e}")))?;

            if !status.success() {
                return Err(EnvironmentError::Unmount(format!(
                    "umount exited with status {status} for {}",
                    self.mount_point.display()
                )));
            }
        }

        info!(
            "Unmounted build environment at {}",
            self.mount_point.display(),
        );
        self.mounted = false;
        Ok(())
    }

    /// Returns whether the composefs image is currently mounted.
    #[must_use]
    pub fn is_mounted(&self) -> bool {
        self.mounted
    }
}

impl Drop for BuildEnvironment {
    fn drop(&mut self) {
        if self.mounted
            && let Err(e) = self.unmount()
        {
            warn!(
                "Failed to unmount build environment on drop at {}: {e}",
                self.mount_point.display(),
            );
        }
    }
}

/// A mutable build sysroot using overlayfs on top of a seed image.
///
/// The seed image is mounted read-only first, then overlayfs is stacked
/// with a writable upperdir. Package installs go to the upperdir; the
/// seed stays pristine. The upper directory is persisted across runs
/// within the same seed for resume support.
pub struct MutableEnvironment {
    /// Path to the seed EROFS image.
    image_path: PathBuf,
    /// CAS object directory for composefs seeds.
    cas_dir: PathBuf,
    /// Base working directory (contains upper/, work/, sysroot/, seed_ro/).
    base_dir: PathBuf,
    /// SHA-256 of the seed image.
    seed_id: String,
    /// Whether the overlay is currently mounted.
    mounted: bool,
    /// Inner read-only mount (kept alive for overlayfs lowerdir).
    seed_env: Option<BuildEnvironment>,
}

impl MutableEnvironment {
    /// Construct a new mutable environment (not yet mounted).
    #[must_use]
    pub fn new(
        image_path: PathBuf,
        cas_dir: PathBuf,
        base_dir: PathBuf,
        seed_id: String,
    ) -> Self {
        Self {
            image_path,
            cas_dir,
            base_dir,
            seed_id,
            mounted: false,
            seed_env: None,
        }
    }

    /// Returns the writable upper directory path (`base_dir/upper`).
    #[must_use]
    pub fn upper_dir(&self) -> PathBuf {
        self.base_dir.join("upper")
    }

    /// Returns the overlayfs work directory path (`base_dir/work`).
    #[must_use]
    pub fn work_dir(&self) -> PathBuf {
        self.base_dir.join("work")
    }

    /// Returns the merged sysroot path (`base_dir/sysroot`).
    #[must_use]
    pub fn sysroot(&self) -> PathBuf {
        self.base_dir.join("sysroot")
    }

    /// Returns the read-only seed mount path (`base_dir/seed_ro`).
    #[must_use]
    pub fn seed_ro_dir(&self) -> PathBuf {
        self.base_dir.join("seed_ro")
    }

    /// Returns whether the overlay is currently mounted.
    #[must_use]
    pub fn is_mounted(&self) -> bool {
        self.mounted
    }

    /// Check if the upper directory was created for a different seed.
    ///
    /// Reads `base_dir/.seed_id`. Returns `true` if the file contains a
    /// different seed ID (stale upper dir). Returns `false` if the file does
    /// not exist (fresh directory) or matches the current seed ID.
    #[must_use]
    pub fn needs_reset(&self) -> bool {
        let marker = self.base_dir.join(".seed_id");
        match std::fs::read_to_string(&marker) {
            Ok(contents) => contents.trim() != self.seed_id.as_str(),
            Err(_) => false,
        }
    }

    /// Mount the seed as a mutable overlayfs sysroot.
    ///
    /// Creates required subdirectories, optionally wipes a stale upper dir,
    /// mounts the seed image read-only, then stacks overlayfs on top.
    ///
    /// # Errors
    ///
    /// Returns `EnvironmentError::Mount` if any directory creation, seed
    /// mount, or overlayfs mount fails.
    pub fn mount(&mut self) -> Result<(), EnvironmentError> {
        if self.mounted {
            return Ok(());
        }

        let upper = self.upper_dir();
        let work = self.work_dir();
        let sysroot = self.sysroot();
        let seed_ro = self.seed_ro_dir();

        // Create all required subdirectories.
        for dir in [&upper, &work, &sysroot, &seed_ro] {
            std::fs::create_dir_all(dir).map_err(|e| {
                EnvironmentError::Mount(format!("failed to create {}: {e}", dir.display()))
            })?;
        }

        // Wipe upper and work dirs if the seed changed.
        if self.needs_reset() {
            for dir in [&upper, &work] {
                std::fs::remove_dir_all(dir).map_err(|e| {
                    EnvironmentError::Mount(format!(
                        "failed to wipe stale dir {}: {e}",
                        dir.display()
                    ))
                })?;
                std::fs::create_dir_all(dir).map_err(|e| {
                    EnvironmentError::Mount(format!(
                        "failed to recreate dir {}: {e}",
                        dir.display()
                    ))
                })?;
            }
        }

        // Mount seed image read-only via BuildEnvironment.
        let mut seed_env = BuildEnvironment::new(
            self.image_path.clone(),
            self.cas_dir.clone(),
            seed_ro.clone(),
            self.seed_id.clone(),
        );
        seed_env.mount()?;

        // Mount overlayfs: lowerdir=seed_ro, upperdir=upper, workdir=work -> sysroot.
        let overlay_opts = format!(
            "lowerdir={},upperdir={},workdir={}",
            seed_ro.display(),
            upper.display(),
            work.display(),
        );
        let status = Command::new("mount")
            .args([
                "-t",
                "overlay",
                "overlay",
                "-o",
                &overlay_opts,
                &sysroot.to_string_lossy(),
            ])
            .status()
            .map_err(|e| EnvironmentError::Mount(format!("failed to execute mount: {e}")))?;

        if !status.success() {
            return Err(EnvironmentError::Mount(format!(
                "overlayfs mount exited with status {status} for {}",
                sysroot.display()
            )));
        }

        // Persist the seed ID marker for future reset detection.
        let marker = self.base_dir.join(".seed_id");
        std::fs::write(&marker, &self.seed_id).map_err(|e| {
            EnvironmentError::Mount(format!(
                "failed to write seed marker {}: {e}",
                marker.display()
            ))
        })?;

        self.seed_env = Some(seed_env);
        self.mounted = true;

        info!(
            "Mounted mutable environment at {} (seed: {})",
            sysroot.display(),
            self.seed_id,
        );
        Ok(())
    }

    /// Unmount overlay then seed (reverse order).
    ///
    /// Uses `nix::mount::umount2` with `MNT_DETACH` for a lazy unmount of the
    /// overlay, falling back to `umount -l` if the nix call fails, then
    /// tears down the inner seed mount.
    ///
    /// # Errors
    ///
    /// Returns `EnvironmentError::Unmount` if unmounting the overlay sysroot
    /// fails.
    pub fn unmount(&mut self) -> Result<(), EnvironmentError> {
        if !self.mounted {
            return Ok(());
        }

        // Unmount the overlay sysroot first.
        let sysroot = self.sysroot();
        let nix_result =
            nix::mount::umount2(&sysroot, nix::mount::MntFlags::MNT_DETACH);

        if let Err(nix_err) = nix_result {
            warn!(
                "nix umount2 failed for overlay ({nix_err}), falling back to umount -l",
            );
            let status = Command::new("umount")
                .args(["-l", sysroot.to_string_lossy().as_ref()])
                .status()
                .map_err(|e| {
                    EnvironmentError::Unmount(format!("failed to execute umount: {e}"))
                })?;

            if !status.success() {
                return Err(EnvironmentError::Unmount(format!(
                    "umount exited with status {status} for {}",
                    sysroot.display()
                )));
            }
        }

        // Tear down the inner seed mount.
        if let Some(mut seed_env) = self.seed_env.take()
            && let Err(e) = seed_env.unmount()
        {
            warn!("Failed to unmount seed environment: {e}");
        }

        self.mounted = false;

        info!(
            "Unmounted mutable environment at {}",
            sysroot.display(),
        );
        Ok(())
    }
}

impl Drop for MutableEnvironment {
    fn drop(&mut self) {
        if self.mounted {
            let _ = self.unmount();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_env() -> BuildEnvironment {
        BuildEnvironment::new(
            PathBuf::from("/conary/builds/env-abc123/root.erofs"),
            PathBuf::from("/conary/objects"),
            PathBuf::from("/tmp/conary-build-abc123"),
            "abc123def456".to_owned(),
        )
    }

    #[test]
    fn new_environment_is_not_mounted() {
        let env = sample_env();
        assert!(!env.is_mounted());
    }

    #[test]
    fn build_env_hash_is_accessible() {
        let env = sample_env();
        assert_eq!(env.build_env_hash, "abc123def456");
    }

    #[test]
    fn is_mounted_returns_false_initially() {
        let env = sample_env();
        assert!(!env.is_mounted());
    }

    #[test]
    fn public_fields_are_accessible() {
        let env = sample_env();
        assert_eq!(
            env.image_path,
            PathBuf::from("/conary/builds/env-abc123/root.erofs")
        );
        assert_eq!(env.cas_dir, PathBuf::from("/conary/objects"));
        assert_eq!(env.mount_point, PathBuf::from("/tmp/conary-build-abc123"));
    }

    // --- MutableEnvironment tests ---

    #[test]
    fn mutable_env_directory_paths() {
        let base = tempfile::TempDir::new().unwrap();
        let env = MutableEnvironment::new(
            PathBuf::from("/fake/seed.erofs"),
            PathBuf::from("/fake/cas"),
            base.path().to_path_buf(),
            "seed_abc".into(),
        );
        assert_eq!(env.upper_dir(), base.path().join("upper"));
        assert_eq!(env.work_dir(), base.path().join("work"));
        assert_eq!(env.sysroot(), base.path().join("sysroot"));
        assert_eq!(env.seed_ro_dir(), base.path().join("seed_ro"));
    }

    #[test]
    fn mutable_env_not_mounted_initially() {
        let env = MutableEnvironment::new(
            PathBuf::from("/fake/seed.erofs"),
            PathBuf::from("/fake/cas"),
            PathBuf::from("/tmp/fake-base"),
            "seed_abc".into(),
        );
        assert!(!env.is_mounted());
    }

    #[test]
    fn needs_reset_no_marker() {
        let base = tempfile::TempDir::new().unwrap();
        let env = MutableEnvironment::new(
            PathBuf::from("/fake/seed.erofs"),
            PathBuf::from("/fake/cas"),
            base.path().to_path_buf(),
            "seed_abc".into(),
        );
        assert!(!env.needs_reset());
    }

    #[test]
    fn needs_reset_same_seed() {
        let base = tempfile::TempDir::new().unwrap();
        std::fs::write(base.path().join(".seed_id"), "seed_abc").unwrap();
        let env = MutableEnvironment::new(
            PathBuf::from("/fake/seed.erofs"),
            PathBuf::from("/fake/cas"),
            base.path().to_path_buf(),
            "seed_abc".into(),
        );
        assert!(!env.needs_reset());
    }

    #[test]
    fn needs_reset_different_seed() {
        let base = tempfile::TempDir::new().unwrap();
        std::fs::write(base.path().join(".seed_id"), "seed_abc").unwrap();
        let env = MutableEnvironment::new(
            PathBuf::from("/fake/seed.erofs"),
            PathBuf::from("/fake/cas"),
            base.path().to_path_buf(),
            "seed_xyz".into(),
        );
        assert!(env.needs_reset());
    }
}
