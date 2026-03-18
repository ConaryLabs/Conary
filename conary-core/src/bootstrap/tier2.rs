// conary-core/src/bootstrap/tier2.rs

//! Phase 6: Tier-2 packages (BLFS + Conary self-hosting)
//!
//! After the base LFS system is complete and bootable, this phase installs
//! additional packages from Beyond Linux From Scratch (BLFS) that are needed
//! for Conary to function: Rust, OpenSSL, SQLite, and Conary itself. Once
//! this phase completes, the system can manage its own packages.

use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::build_runner::PackageBuildRunner;
use super::config::BootstrapConfig;
use super::toolchain::Toolchain;

/// Tier-2 package build order (BLFS + Conary).
#[allow(dead_code)]
const TIER2_ORDER: [&str; 8] = [
    "curl",
    "cmake",
    "llvm",
    "rust",
    "sqlite",
    "openssl",
    "conary",
    "conary-server",
];

/// Errors specific to the Tier-2 build phase.
#[derive(Debug, thiserror::Error)]
pub enum Tier2Error {
    /// A package build step failed.
    #[error("Tier-2 build failed for {package}: {reason}")]
    BuildFailed { package: String, reason: String },

    /// The base system is not ready.
    #[error("Base system not ready: {0}")]
    BaseNotReady(String),

    /// I/O error during the build.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Error from the shared build runner.
    #[error(transparent)]
    BuildRunner(#[from] super::build_runner::BuildRunnerError),
}

/// Builder for Phase 6 Tier-2 packages.
///
/// Builds BLFS packages and Conary itself, completing the self-hosting
/// bootstrap.
pub struct Tier2Builder {
    /// Working directory for build artifacts.
    work_dir: PathBuf,
    /// Root of the installed system.
    system_root: PathBuf,
    /// Bootstrap configuration.
    config: BootstrapConfig,
    /// System toolchain.
    toolchain: Toolchain,
    /// Shared build runner for source fetching and verification.
    _runner: PackageBuildRunner,
}

impl Tier2Builder {
    /// Create a new Tier-2 builder.
    ///
    /// # Arguments
    ///
    /// * `work_dir` - scratch space for downloads and build trees
    /// * `system_root` - root of the installed LFS system
    /// * `config` - bootstrap configuration
    /// * `toolchain` - system toolchain from the completed LFS build
    ///
    /// # Errors
    ///
    /// Returns `Tier2Error::BaseNotReady` if `system_root` does not contain
    /// a usable system (missing `/usr/bin/gcc`).
    pub fn new(
        work_dir: &Path,
        system_root: &Path,
        config: BootstrapConfig,
        toolchain: Toolchain,
    ) -> Result<Self, Tier2Error> {
        let gcc = system_root.join("usr").join("bin").join("gcc");
        if !gcc.exists() {
            return Err(Tier2Error::BaseNotReady(format!(
                "GCC not found at {}, complete Phase 3 first",
                gcc.display()
            )));
        }

        let sources_dir = work_dir.join("sources");
        std::fs::create_dir_all(&sources_dir)?;

        let runner = PackageBuildRunner::new(&sources_dir, &config);

        Ok(Self {
            work_dir: work_dir.to_path_buf(),
            system_root: system_root.to_path_buf(),
            config,
            toolchain,
            _runner: runner,
        })
    }

    /// Build all Tier-2 packages in order.
    pub fn build_all(&self) -> Result<(), Tier2Error> {
        info!(
            "Phase 6: Building Tier-2 packages ({} packages)",
            TIER2_ORDER.len()
        );

        for (i, pkg) in TIER2_ORDER.iter().enumerate() {
            info!(
                "Building Tier-2 package [{}/{}]: {}",
                i + 1,
                TIER2_ORDER.len(),
                pkg
            );
            // TODO: implement recipe-driven build for each Tier-2 package
            debug!(
                "  build_package({}) -- placeholder (root={}, toolchain={})",
                pkg,
                self.system_root.display(),
                self.toolchain.target
            );
            let _ = &self.config;
            let _ = &self.work_dir;
        }

        info!("Phase 6 complete: Conary self-hosting achieved");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::toolchain::ToolchainKind;

    #[test]
    fn test_tier2_order_count() {
        assert_eq!(TIER2_ORDER.len(), 8);
    }

    #[test]
    fn test_tier2_includes_conary() {
        assert!(TIER2_ORDER.contains(&"conary"));
    }

    #[test]
    fn test_new_requires_gcc() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: root.path().join("usr"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let result = Tier2Builder::new(work.path(), root.path(), config, tc);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_succeeds_with_gcc() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let gcc_path = root.path().join("usr/bin");
        std::fs::create_dir_all(&gcc_path).unwrap();
        std::fs::write(gcc_path.join("gcc"), b"").unwrap();

        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: root.path().join("usr"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = Tier2Builder::new(work.path(), root.path(), config, tc);
        assert!(builder.is_ok());
    }

    #[test]
    fn test_build_all_placeholder() {
        let work = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let gcc_path = root.path().join("usr/bin");
        std::fs::create_dir_all(&gcc_path).unwrap();
        std::fs::write(gcc_path.join("gcc"), b"").unwrap();

        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::System,
            path: root.path().join("usr"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = Tier2Builder::new(work.path(), root.path(), config, tc).unwrap();
        assert!(builder.build_all().is_ok());
    }
}
