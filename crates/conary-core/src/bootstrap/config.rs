// conary-core/src/bootstrap/config.rs

//! Bootstrap configuration types

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use crate::image::arch::TargetArch;

/// Bootstrap configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    /// Target architecture
    pub target_arch: TargetArch,

    /// LFS root directory ($LFS) -- the target filesystem root
    pub lfs_root: PathBuf,

    /// Where to install the cross-tools (Phase 1 output: $LFS/tools)
    pub tools_prefix: PathBuf,

    /// Number of parallel jobs for building
    pub jobs: usize,

    /// Enable verbose output
    pub verbose: bool,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            target_arch: TargetArch::X86_64,
            lfs_root: PathBuf::from("/mnt/lfs"),
            tools_prefix: PathBuf::from("/mnt/lfs/tools"),
            jobs: num_cpus(),
            verbose: false,
        }
    }
}

impl BootstrapConfig {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the target architecture
    pub fn with_target(mut self, arch: TargetArch) -> Self {
        self.target_arch = arch;
        self
    }

    /// Set the LFS root directory ($LFS)
    pub fn with_lfs_root(mut self, path: impl Into<PathBuf>) -> Self {
        let root: PathBuf = path.into();
        self.tools_prefix = root.join("tools");
        self.lfs_root = root;
        self
    }

    /// Set the tools prefix
    pub fn with_tools_prefix(mut self, path: impl Into<PathBuf>) -> Self {
        self.tools_prefix = path.into();
        self
    }

    /// Set number of parallel jobs
    pub fn with_jobs(mut self, jobs: usize) -> Self {
        self.jobs = jobs;
        self
    }

    /// Enable verbose output
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Get the target triple
    pub fn triple(&self) -> &'static str {
        self.target_arch.triple()
    }

    /// Get the bin directory for the toolchain
    pub fn tools_bin(&self) -> PathBuf {
        self.tools_prefix.join("bin")
    }

    /// Get the path to a toolchain binary (e.g., "gcc" -> "/tools/bin/x86_64-conary-linux-gnu-gcc")
    pub fn tool_path(&self, tool: &str) -> PathBuf {
        self.tools_bin().join(format!("{}-{}", self.triple(), tool))
    }
}

/// Get number of CPUs for parallel builds
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = BootstrapConfig::default();
        assert_eq!(config.target_arch, TargetArch::X86_64);
        assert_eq!(config.lfs_root, PathBuf::from("/mnt/lfs"));
        assert_eq!(config.tools_prefix, PathBuf::from("/mnt/lfs/tools"));
        assert!(config.jobs > 0);
    }

    #[test]
    fn test_config_builder() {
        let config = BootstrapConfig::new()
            .with_target(TargetArch::Aarch64)
            .with_tools_prefix("/opt/cross")
            .with_jobs(8)
            .with_verbose(true);

        assert_eq!(config.target_arch, TargetArch::Aarch64);
        assert_eq!(config.tools_prefix, PathBuf::from("/opt/cross"));
        assert_eq!(config.jobs, 8);
        assert!(config.verbose);
    }

    #[test]
    fn test_tool_path() {
        let config = BootstrapConfig::default();
        assert_eq!(
            config.tool_path("gcc"),
            PathBuf::from("/mnt/lfs/tools/bin/x86_64-conary-linux-gnu-gcc")
        );
    }

    #[test]
    fn test_with_lfs_root() {
        let config = BootstrapConfig::new().with_lfs_root("/custom/lfs");
        assert_eq!(config.lfs_root, PathBuf::from("/custom/lfs"));
        assert_eq!(config.tools_prefix, PathBuf::from("/custom/lfs/tools"));
    }
}
