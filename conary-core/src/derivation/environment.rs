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

        let args = opts.to_mount_args();
        let status = Command::new("mount")
            .args(&args)
            .status()
            .map_err(|e| EnvironmentError::Mount(format!("failed to execute mount: {e}")))?;

        if !status.success() {
            return Err(EnvironmentError::Mount(format!(
                "mount exited with status {status} for image {}",
                self.image_path.display()
            )));
        }

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
}
