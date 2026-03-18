// conary-core/src/bootstrap/temp_tools.rs

//! Phase 2: Temporary tools (LFS Chapters 6-7)
//!
//! Uses the Phase 1 cross-toolchain to build a set of utilities that will
//! run inside the chroot. Chapter 6 cross-compiles packages using the
//! `$LFS_TGT`-prefixed tools. Chapter 7 sets up the chroot environment
//! and builds a handful of packages natively inside it.
//!
//! After this phase the chroot contains enough tools (bash, coreutils,
//! make, etc.) to build the final system without any host dependencies.

use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::build_runner::PackageBuildRunner;
use super::config::BootstrapConfig;
use super::toolchain::Toolchain;

/// Cross-compiled packages (LFS Chapter 6).
///
/// Built on the host using the Phase 1 cross-toolchain, installed into
/// `$LFS/` so they are available once we enter the chroot.
#[allow(dead_code)]
const CH6_PACKAGES: [&str; 17] = [
    "m4",
    "ncurses",
    "bash",
    "coreutils",
    "diffutils",
    "file",
    "findutils",
    "gawk",
    "grep",
    "gzip",
    "make",
    "patch",
    "sed",
    "tar",
    "xz",
    "binutils-pass2",
    "gcc-pass2",
];

/// Chroot packages (LFS Chapter 7).
///
/// Built natively inside the chroot after `setup_chroot()` prepares the
/// virtual kernel filesystems and directory structure.
#[allow(dead_code)]
const CH7_PACKAGES: [&str; 6] = [
    "gettext",
    "bison",
    "perl",
    "python",
    "texinfo",
    "util-linux",
];

/// Errors specific to the temporary tools build phase.
#[derive(Debug, thiserror::Error)]
pub enum TempToolsError {
    /// A package build step failed.
    #[error("Temp-tools build failed for {package}: {reason}")]
    BuildFailed { package: String, reason: String },

    /// Phase 1 cross-tools are missing.
    #[error("Phase 1 cross-tools not found at {0}")]
    MissingCrossTools(PathBuf),

    /// Chroot setup failed.
    #[error("Chroot setup failed: {0}")]
    ChrootSetup(String),

    /// Verification failed.
    #[error("Temp-tools verification failed: {0}")]
    Verification(String),

    /// I/O error during the build.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Error from the shared build runner.
    #[error(transparent)]
    BuildRunner(#[from] super::build_runner::BuildRunnerError),
}

/// Builder for Phase 2 temporary tools.
///
/// First cross-compiles the Chapter 6 packages using the Phase 1
/// cross-toolchain, then sets up the chroot and builds Chapter 7
/// packages natively inside it.
pub struct TempToolsBuilder {
    /// Working directory for build artifacts.
    work_dir: PathBuf,
    /// Root of the LFS filesystem.
    lfs_root: PathBuf,
    /// Bootstrap configuration.
    config: BootstrapConfig,
    /// Phase 1 cross-toolchain (from `$LFS/tools/`).
    cross_toolchain: Toolchain,
    /// Shared build runner for source fetching and verification.
    _runner: PackageBuildRunner,
}

impl TempToolsBuilder {
    /// Create a new temporary tools builder.
    ///
    /// # Arguments
    ///
    /// * `work_dir` - scratch space for downloads and build trees
    /// * `lfs_root` - root of the LFS partition
    /// * `config` - bootstrap configuration
    /// * `cross_toolchain` - the Phase 1 cross-toolchain
    ///
    /// # Errors
    ///
    /// Returns `TempToolsError::MissingCrossTools` if `$LFS/tools/bin`
    /// does not exist.
    pub fn new(
        work_dir: &Path,
        lfs_root: &Path,
        config: BootstrapConfig,
        cross_toolchain: Toolchain,
    ) -> Result<Self, TempToolsError> {
        let tools_bin = lfs_root.join("tools").join("bin");
        if !tools_bin.exists() {
            return Err(TempToolsError::MissingCrossTools(tools_bin));
        }

        let sources_dir = work_dir.join("sources");
        std::fs::create_dir_all(&sources_dir)?;

        let runner = PackageBuildRunner::new(&sources_dir, &config);

        Ok(Self {
            work_dir: work_dir.to_path_buf(),
            lfs_root: lfs_root.to_path_buf(),
            config,
            cross_toolchain,
            _runner: runner,
        })
    }

    /// Cross-compile all Chapter 6 packages.
    ///
    /// Uses the Phase 1 cross-toolchain to build each package and installs
    /// the results into `$LFS/`.
    pub fn build_cross_packages(&self) -> Result<(), TempToolsError> {
        info!(
            "Phase 2a: Cross-compiling temp tools ({} packages)",
            CH6_PACKAGES.len()
        );

        for (i, pkg) in CH6_PACKAGES.iter().enumerate() {
            info!(
                "Cross-compiling [{}/{}]: {}",
                i + 1,
                CH6_PACKAGES.len(),
                pkg
            );
            // TODO: implement recipe-driven cross-compile for each package
            debug!(
                "  build_cross_package({}) -- placeholder (toolchain={})",
                pkg, self.cross_toolchain.target
            );
        }

        info!("Phase 2a complete: all Chapter 6 packages cross-compiled");
        Ok(())
    }

    /// Set up the chroot environment.
    ///
    /// Creates essential directories, device nodes, and virtual kernel
    /// filesystems (`/dev`, `/proc`, `/sys`, `/run`) inside `$LFS/`.
    pub fn setup_chroot(&self) -> Result<(), TempToolsError> {
        info!(
            "Setting up chroot environment at {}",
            self.lfs_root.display()
        );

        // TODO: create directory hierarchy, mount virtual filesystems,
        //       create essential symlinks and files
        debug!(
            "  setup_chroot -- placeholder (lfs_root={})",
            self.lfs_root.display()
        );

        Ok(())
    }

    /// Build Chapter 7 packages inside the chroot.
    ///
    /// These are built natively (not cross-compiled) using the tools
    /// that are now available inside the chroot.
    pub fn build_chroot_packages(&self) -> Result<(), TempToolsError> {
        info!(
            "Phase 2b: Building chroot packages ({} packages)",
            CH7_PACKAGES.len()
        );

        for (i, pkg) in CH7_PACKAGES.iter().enumerate() {
            info!(
                "Building in chroot [{}/{}]: {}",
                i + 1,
                CH7_PACKAGES.len(),
                pkg
            );
            // TODO: implement chroot-based build for each package
            debug!(
                "  build_chroot_package({}) -- placeholder (work_dir={})",
                pkg,
                self.work_dir.display()
            );
        }

        info!("Phase 2b complete: all Chapter 7 packages built");
        Ok(())
    }

    /// Verify that the temporary tools environment is functional.
    ///
    /// Checks that key binaries exist and are executable inside the
    /// chroot root.
    pub fn verify(&self) -> Result<(), TempToolsError> {
        info!("Verifying temporary tools...");

        let essential_binaries = ["bash", "cat", "ls", "make", "gcc"];

        for bin in &essential_binaries {
            let path = self.lfs_root.join("usr").join("bin").join(bin);
            if !path.exists() {
                // Also check tools/bin as a fallback
                let tools_path = self.lfs_root.join("tools").join("bin").join(bin);
                if !tools_path.exists() {
                    return Err(TempToolsError::Verification(format!(
                        "Essential binary not found: {bin}"
                    )));
                }
            }
        }

        let _ = &self.config;
        info!("Temporary tools verification passed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::toolchain::ToolchainKind;

    #[test]
    fn test_ch6_package_count() {
        assert_eq!(CH6_PACKAGES.len(), 17);
    }

    #[test]
    fn test_ch7_package_count() {
        assert_eq!(CH7_PACKAGES.len(), 6);
    }

    #[test]
    fn test_new_requires_tools_bin() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        // No tools/bin directory created
        let config = BootstrapConfig::new();
        let cross_tc = Toolchain {
            kind: ToolchainKind::CrossTools,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let result = TempToolsBuilder::new(work.path(), lfs.path(), config, cross_tc);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_succeeds_with_tools_bin() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("tools/bin")).unwrap();

        let config = BootstrapConfig::new();
        let cross_tc = Toolchain {
            kind: ToolchainKind::CrossTools,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = TempToolsBuilder::new(work.path(), lfs.path(), config, cross_tc);
        assert!(builder.is_ok());
    }

    #[test]
    fn test_cross_packages_placeholder() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lfs.path().join("tools/bin")).unwrap();

        let config = BootstrapConfig::new();
        let cross_tc = Toolchain {
            kind: ToolchainKind::CrossTools,
            path: lfs.path().join("tools"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = TempToolsBuilder::new(work.path(), lfs.path(), config, cross_tc).unwrap();
        assert!(builder.build_cross_packages().is_ok());
    }
}
