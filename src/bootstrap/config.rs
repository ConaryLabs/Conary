// src/bootstrap/config.rs

//! Bootstrap configuration types

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Target architecture for bootstrap
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TargetArch {
    /// x86_64 / AMD64
    #[default]
    X86_64,
    /// AArch64 / ARM64
    Aarch64,
    /// RISC-V 64-bit
    Riscv64,
}

impl TargetArch {
    /// Get the GNU target triple
    pub fn triple(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64-conary-linux-gnu",
            Self::Aarch64 => "aarch64-conary-linux-gnu",
            Self::Riscv64 => "riscv64-conary-linux-gnu",
        }
    }

    /// Get the architecture name for crosstool-ng
    pub fn ct_ng_arch(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::Riscv64 => "riscv64",
        }
    }

    /// Get the kernel architecture name
    pub fn kernel_arch(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "arm64",
            Self::Riscv64 => "riscv",
        }
    }

    /// Parse from string
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "x86_64" | "amd64" | "x64" => Some(Self::X86_64),
            "aarch64" | "arm64" => Some(Self::Aarch64),
            "riscv64" => Some(Self::Riscv64),
            _ => None,
        }
    }
}

impl std::fmt::Display for TargetArch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::X86_64 => write!(f, "x86_64"),
            Self::Aarch64 => write!(f, "aarch64"),
            Self::Riscv64 => write!(f, "riscv64"),
        }
    }
}

/// Bootstrap configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    /// Target architecture
    pub target_arch: TargetArch,

    /// Where to install the Stage 0 toolchain
    pub tools_prefix: PathBuf,

    /// Where to install the Stage 1 toolchain
    pub stage1_prefix: PathBuf,

    /// Sysroot directory for cross-compilation
    pub sysroot: PathBuf,

    /// Number of parallel jobs for building
    pub jobs: usize,

    /// Kernel version for headers
    pub kernel_version: String,

    /// GCC version
    pub gcc_version: String,

    /// glibc version
    pub glibc_version: String,

    /// binutils version
    pub binutils_version: String,

    /// Path to crosstool-ng config (if using custom)
    pub crosstool_config: Option<PathBuf>,

    /// URL for the Stage 0 seed toolchain
    pub seed_url: Option<String>,

    /// Checksum for the Stage 0 seed toolchain
    pub seed_checksum: Option<String>,

    /// Enable verbose output
    pub verbose: bool,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            target_arch: TargetArch::X86_64,
            tools_prefix: PathBuf::from("/tools"),
            stage1_prefix: PathBuf::from("/conary/stage1"),
            sysroot: PathBuf::from("/conary/sysroot"),
            jobs: num_cpus(),
            kernel_version: "6.18".to_string(),
            gcc_version: "15.2.0".to_string(),
            glibc_version: "2.42".to_string(),
            binutils_version: "2.45.1".to_string(),
            crosstool_config: None,
            seed_url: None,
            seed_checksum: None,
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

    /// Set the tools prefix
    pub fn with_tools_prefix(mut self, path: impl Into<PathBuf>) -> Self {
        self.tools_prefix = path.into();
        self
    }

    /// Set the Stage 1 prefix
    pub fn with_stage1_prefix(mut self, path: impl Into<PathBuf>) -> Self {
        self.stage1_prefix = path.into();
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

    /// Use a custom crosstool-ng config
    pub fn with_crosstool_config(mut self, path: impl Into<PathBuf>) -> Self {
        self.crosstool_config = Some(path.into());
        self
    }

    /// Set the Stage 0 seed toolchain
    pub fn with_seed(mut self, url: impl Into<String>, checksum: impl Into<String>) -> Self {
        self.seed_url = Some(url.into());
        self.seed_checksum = Some(checksum.into());
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
        self.tools_bin()
            .join(format!("{}-{}", self.triple(), tool))
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
    fn test_target_arch_triple() {
        assert_eq!(TargetArch::X86_64.triple(), "x86_64-conary-linux-gnu");
        assert_eq!(TargetArch::Aarch64.triple(), "aarch64-conary-linux-gnu");
        assert_eq!(TargetArch::Riscv64.triple(), "riscv64-conary-linux-gnu");
    }

    #[test]
    fn test_target_arch_parse() {
        assert_eq!(TargetArch::parse("x86_64"), Some(TargetArch::X86_64));
        assert_eq!(TargetArch::parse("amd64"), Some(TargetArch::X86_64));
        assert_eq!(TargetArch::parse("aarch64"), Some(TargetArch::Aarch64));
        assert_eq!(TargetArch::parse("arm64"), Some(TargetArch::Aarch64));
        assert_eq!(TargetArch::parse("riscv64"), Some(TargetArch::Riscv64));
        assert_eq!(TargetArch::parse("unknown"), None);
    }

    #[test]
    fn test_config_defaults() {
        let config = BootstrapConfig::default();
        assert_eq!(config.target_arch, TargetArch::X86_64);
        assert_eq!(config.tools_prefix, PathBuf::from("/tools"));
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
            PathBuf::from("/tools/bin/x86_64-conary-linux-gnu-gcc")
        );
    }
}
