// conary-core/src/bootstrap/cross_tools.rs

//! Phase 1: Cross-compilation tools (LFS Chapter 5)
//!
//! Builds a minimal cross-toolchain targeting `$LFS_TGT` using the host
//! compiler. This produces binutils and GCC that can generate code for the
//! target, plus cross-compiled glibc and libstdc++. The output lives under
//! `$LFS/tools/` and is used by Phase 2 (temp_tools) to build inside the
//! chroot.
//!
//! Build order follows LFS 13 Chapter 5:
//!   1. binutils (pass 1) -- cross-assembler and linker
//!   2. gcc (pass 1)      -- cross-compiler (C only, no threads)
//!   3. linux-headers     -- kernel API headers for glibc
//!   4. glibc             -- C library for the target
//!   5. libstdc++         -- C++ standard library (from GCC source)

use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::build_runner::PackageBuildRunner;
use super::config::BootstrapConfig;
use super::toolchain::{Toolchain, ToolchainKind};

/// Target triplet for the LFS cross-toolchain.
pub const LFS_TGT: &str = "x86_64-conary-linux-gnu";

/// Package build order for Phase 1 (LFS Chapter 5).
#[allow(dead_code)]
const CROSS_TOOLS_ORDER: [&str; 5] = [
    "binutils-pass1",
    "gcc-pass1",
    "linux-headers",
    "glibc",
    "libstdc++",
];

/// Errors specific to the cross-tools build phase.
#[derive(Debug, thiserror::Error)]
pub enum CrossToolsError {
    /// A package build step failed.
    #[error("Cross-tools build failed for {package}: {reason}")]
    BuildFailed { package: String, reason: String },

    /// The host toolchain is missing or broken.
    #[error("Host toolchain not usable: {0}")]
    HostToolchain(String),

    /// The LFS root directory does not exist or is not writable.
    #[error("LFS root not accessible: {0}")]
    LfsRoot(String),

    /// Verification of the cross-toolchain failed.
    #[error("Cross-tools verification failed: {0}")]
    Verification(String),

    /// I/O error during the build.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Error from the shared build runner.
    #[error(transparent)]
    BuildRunner(#[from] super::build_runner::BuildRunnerError),
}

/// Builder for Phase 1 cross-compilation tools.
///
/// Constructs a cross-toolchain under `$LFS/tools/` that targets `LFS_TGT`.
/// The host system's native compiler is used to build the cross tools.
pub struct CrossToolsBuilder {
    /// Working directory for build artifacts.
    work_dir: PathBuf,
    /// Root of the LFS filesystem (typically /mnt/lfs).
    lfs_root: PathBuf,
    /// Bootstrap configuration.
    config: BootstrapConfig,
    /// Host toolchain used to compile the cross tools.
    host_toolchain: Toolchain,
    /// Shared build runner for source fetching and verification.
    _runner: PackageBuildRunner,
}

impl CrossToolsBuilder {
    /// Create a new cross-tools builder.
    ///
    /// # Arguments
    ///
    /// * `work_dir` - scratch space for downloads and build trees
    /// * `lfs_root` - root of the LFS partition (cross-tools install to `$lfs_root/tools/`)
    /// * `config` - bootstrap configuration
    /// * `host_toolchain` - the host system's native toolchain
    ///
    /// # Errors
    ///
    /// Returns `CrossToolsError::LfsRoot` if `lfs_root` does not exist.
    pub fn new(
        work_dir: &Path,
        lfs_root: &Path,
        config: BootstrapConfig,
        host_toolchain: Toolchain,
    ) -> Result<Self, CrossToolsError> {
        if !lfs_root.exists() {
            return Err(CrossToolsError::LfsRoot(format!(
                "LFS root does not exist: {}",
                lfs_root.display()
            )));
        }

        let sources_dir = work_dir.join("sources");
        std::fs::create_dir_all(&sources_dir)?;

        let runner = PackageBuildRunner::new(&sources_dir, &config);

        Ok(Self {
            work_dir: work_dir.to_path_buf(),
            lfs_root: lfs_root.to_path_buf(),
            config,
            host_toolchain,
            _runner: runner,
        })
    }

    /// Build all cross-tools in order, returning the resulting toolchain.
    ///
    /// Iterates through `CROSS_TOOLS_ORDER`, building each package in
    /// sequence. On success, returns a `Toolchain` with `kind: Stage1`
    /// rooted at `$LFS/tools/`.
    pub fn build_all(&self) -> Result<Toolchain, CrossToolsError> {
        info!(
            "Phase 1: Building cross-tools ({} packages)",
            CROSS_TOOLS_ORDER.len()
        );
        info!("  LFS_TGT = {}", LFS_TGT);
        info!("  LFS root: {}", self.lfs_root.display());
        info!(
            "  Host compiler: {}",
            self.host_toolchain.gcc().display()
        );

        for (i, pkg) in CROSS_TOOLS_ORDER.iter().enumerate() {
            info!(
                "Building cross-tool [{}/{}]: {}",
                i + 1,
                CROSS_TOOLS_ORDER.len(),
                pkg
            );
            self.build_package(pkg)?;
        }

        let tools_path = self.lfs_root.join("tools");
        info!(
            "Phase 1 complete: cross-tools installed to {}",
            tools_path.display()
        );

        Ok(Toolchain {
            kind: ToolchainKind::CrossTools,
            path: tools_path,
            target: LFS_TGT.to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        })
    }

    /// Build a single cross-tools package.
    ///
    /// Currently a placeholder -- each package will get its own recipe-driven
    /// build logic in a later task.
    fn build_package(&self, name: &str) -> Result<(), CrossToolsError> {
        // TODO: implement recipe-driven build for each package
        debug!(
            "  build_package({}) -- placeholder (work_dir={}, jobs={})",
            name,
            self.work_dir.display(),
            self.config.jobs
        );
        Ok(())
    }

    /// Verify that the cross-toolchain is functional.
    ///
    /// Writes a minimal `hello.c`, compiles it with the cross-GCC, and checks
    /// that the resulting binary targets the correct architecture using `file`.
    pub fn verify(&self) -> Result<(), CrossToolsError> {
        info!("Verifying cross-toolchain...");

        let tools_bin = self.lfs_root.join("tools").join("bin");
        let cross_gcc = tools_bin.join(format!("{LFS_TGT}-gcc"));

        if !cross_gcc.exists() {
            return Err(CrossToolsError::Verification(format!(
                "Cross-GCC not found at {}",
                cross_gcc.display()
            )));
        }

        // Write a trivial C program
        let test_dir = self.work_dir.join("verify");
        std::fs::create_dir_all(&test_dir)?;

        let hello_c = test_dir.join("hello.c");
        std::fs::write(
            &hello_c,
            b"#include <stdio.h>\nint main() { puts(\"hello\"); return 0; }\n",
        )?;

        let hello_bin = test_dir.join("hello");

        // Compile with cross-GCC
        let output = std::process::Command::new(&cross_gcc)
            .args([
                hello_c
                    .to_str()
                    .ok_or_else(|| CrossToolsError::Verification("invalid path".to_string()))?,
                "-o",
                hello_bin
                    .to_str()
                    .ok_or_else(|| CrossToolsError::Verification("invalid path".to_string()))?,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CrossToolsError::Verification(format!(
                "Cross-GCC compilation failed: {stderr}"
            )));
        }

        // Check architecture with `file`
        let file_output = std::process::Command::new("file")
            .arg(&hello_bin)
            .output()?;

        let file_str = String::from_utf8_lossy(&file_output.stdout);
        debug!("  file output: {}", file_str.trim());

        if !file_str.contains("x86-64") && !file_str.contains("x86_64") {
            return Err(CrossToolsError::Verification(format!(
                "Binary is not x86_64: {file_str}"
            )));
        }

        // Clean up
        let _ = std::fs::remove_dir_all(&test_dir);

        info!("Cross-toolchain verification passed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lfs_tgt_triple() {
        assert_eq!(LFS_TGT, "x86_64-conary-linux-gnu");
    }

    #[test]
    fn test_cross_tools_order_count() {
        assert_eq!(CROSS_TOOLS_ORDER.len(), 5);
    }

    #[test]
    fn test_new_requires_existing_lfs_root() {
        let work = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let host = Toolchain {
            kind: ToolchainKind::Host,
            path: PathBuf::from("/usr"),
            target: "x86_64-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let result = CrossToolsBuilder::new(
            work.path(),
            Path::new("/nonexistent/lfs/root"),
            config,
            host,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_new_succeeds_with_valid_root() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let host = Toolchain {
            kind: ToolchainKind::Host,
            path: PathBuf::from("/usr"),
            target: "x86_64-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder =
            CrossToolsBuilder::new(work.path(), lfs.path(), config, host);
        assert!(builder.is_ok());
    }

    #[test]
    fn test_build_all_returns_stage1_toolchain() {
        let work = tempfile::tempdir().unwrap();
        let lfs = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let host = Toolchain {
            kind: ToolchainKind::Host,
            path: PathBuf::from("/usr"),
            target: "x86_64-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder =
            CrossToolsBuilder::new(work.path(), lfs.path(), config, host).unwrap();
        let toolchain = builder.build_all().unwrap();

        assert_eq!(toolchain.kind, ToolchainKind::CrossTools);
        assert_eq!(toolchain.target, LFS_TGT);
        assert!(toolchain.path.ends_with("tools"));
    }
}
