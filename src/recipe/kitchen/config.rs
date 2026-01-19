// src/recipe/kitchen/config.rs

//! Configuration types for the Kitchen build system

use crate::recipe::format::BuildStage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Configuration for a specific bootstrap stage
///
/// This specifies the sysroot, toolchain paths, and environment
/// for a particular bootstrap stage.
#[derive(Debug, Clone)]
pub struct StageConfig {
    /// The bootstrap stage this config is for
    pub stage: BuildStage,
    /// Root directory containing the stage's libraries and headers
    pub sysroot: PathBuf,
    /// Directory containing the toolchain binaries (compilers, linkers)
    pub tools_dir: Option<PathBuf>,
    /// Tool name prefix (e.g., "x86_64-conary-linux-gnu")
    pub tool_prefix: Option<String>,
    /// Target triple for cross-compilation
    pub target_triple: Option<String>,
}

impl StageConfig {
    /// Create a new stage configuration
    pub fn new(stage: BuildStage, sysroot: PathBuf) -> Self {
        Self {
            stage,
            sysroot,
            tools_dir: None,
            tool_prefix: None,
            target_triple: None,
        }
    }

    /// Set the tools directory
    pub fn with_tools_dir(mut self, dir: PathBuf) -> Self {
        self.tools_dir = Some(dir);
        self
    }

    /// Set the tool prefix
    pub fn with_tool_prefix(mut self, prefix: String) -> Self {
        self.tool_prefix = Some(prefix);
        self
    }

    /// Set the target triple
    pub fn with_target(mut self, target: String) -> Self {
        self.target_triple = Some(target);
        self
    }

    /// Get environment variables for this stage
    pub fn env_vars(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();

        // Set sysroot
        env.push(("SYSROOT".to_string(), self.sysroot.to_string_lossy().to_string()));
        let sysroot_flag = format!("--sysroot={}", self.sysroot.display());

        // Set tool prefix for cross-compilation
        if let Some(prefix) = &self.tool_prefix {
            let tools_path = self.tools_dir.as_ref()
                .map(|d| format!("{}/", d.display()))
                .unwrap_or_default();

            env.push(("CC".to_string(), format!("{}{}-gcc", tools_path, prefix)));
            env.push(("CXX".to_string(), format!("{}{}-g++", tools_path, prefix)));
            env.push(("AR".to_string(), format!("{}{}-ar", tools_path, prefix)));
            env.push(("LD".to_string(), format!("{}{}-ld", tools_path, prefix)));
            env.push(("RANLIB".to_string(), format!("{}{}-ranlib", tools_path, prefix)));
            env.push(("NM".to_string(), format!("{}{}-nm", tools_path, prefix)));
            env.push(("STRIP".to_string(), format!("{}{}-strip", tools_path, prefix)));
            env.push(("CROSS_COMPILE".to_string(), format!("{}-", prefix)));
        }

        // Set target
        if let Some(target) = &self.target_triple {
            env.push(("TARGET".to_string(), target.clone()));
        }

        // Set CFLAGS/LDFLAGS with sysroot
        env.push(("CFLAGS".to_string(), sysroot_flag.clone()));
        env.push(("CXXFLAGS".to_string(), sysroot_flag.clone()));
        env.push(("LDFLAGS".to_string(), sysroot_flag));

        // Set stage marker
        env.push(("CONARY_STAGE".to_string(), self.stage.as_str().to_string()));

        env
    }
}

/// Stage configuration registry
///
/// Holds configurations for all bootstrap stages. Used by the Kitchen
/// to select the appropriate configuration for each recipe's stage.
#[derive(Debug, Default)]
pub struct StageRegistry {
    stages: HashMap<BuildStage, StageConfig>,
}

impl StageRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a stage configuration
    pub fn register(&mut self, config: StageConfig) {
        self.stages.insert(config.stage, config);
    }

    /// Get the configuration for a stage
    pub fn get(&self, stage: BuildStage) -> Option<&StageConfig> {
        self.stages.get(&stage)
    }

    /// Check if a stage is registered
    pub fn has_stage(&self, stage: BuildStage) -> bool {
        self.stages.contains_key(&stage)
    }

    /// Create a typical bootstrap registry with standard stage paths
    ///
    /// This sets up stages under a base directory:
    /// - stage0: `/base/stage0` (cross-tools from host)
    /// - stage1: `/base/stage1` (native but using stage0 tools)
    /// - stage2: `/base/stage2` (fully self-hosted)
    pub fn bootstrap_standard(base: &Path, target: &str) -> Self {
        let mut registry = Self::new();

        // Stage 0: Cross-compiled from host
        let stage0 = StageConfig::new(BuildStage::Stage0, base.join("stage0"))
            .with_tools_dir(base.join("cross-tools/bin"))
            .with_tool_prefix(target.to_string())
            .with_target(target.to_string());
        registry.register(stage0);

        // Stage 1: Built with stage0 tools
        let stage1 = StageConfig::new(BuildStage::Stage1, base.join("stage1"))
            .with_tools_dir(base.join("stage0/usr/bin"))
            .with_target(target.to_string());
        registry.register(stage1);

        // Stage 2: Fully self-hosted
        let stage2 = StageConfig::new(BuildStage::Stage2, base.join("stage2"))
            .with_tools_dir(base.join("stage1/usr/bin"));
        registry.register(stage2);

        registry
    }
}

/// Configuration for the Kitchen
#[derive(Debug, Clone)]
pub struct KitchenConfig {
    /// Directory for downloaded sources
    pub source_cache: PathBuf,
    /// Timeout for build operations
    pub timeout: Duration,
    /// Number of parallel jobs
    pub jobs: u32,
    /// Enable network access during build (not recommended)
    pub allow_network: bool,
    /// Keep build directory after completion (for debugging)
    pub keep_builddir: bool,
    /// Enable container isolation for builds (requires root or user namespaces)
    pub use_isolation: bool,
    /// Memory limit for isolated builds (bytes, 0 = no limit)
    pub memory_limit: u64,
    /// CPU time limit for isolated builds (seconds, 0 = no limit)
    pub cpu_time_limit: u64,
    /// Enable pristine mode - no host system mounts (for bootstrap builds)
    ///
    /// When enabled, the build container has NO access to host /usr, /lib, etc.
    /// You must provide a sysroot containing the toolchain to use.
    pub pristine_mode: bool,
    /// Sysroot path for pristine builds (e.g., /opt/stage0)
    ///
    /// Only used when pristine_mode is true. This directory should contain
    /// the cross-compiler and libraries needed for the build.
    pub sysroot: Option<PathBuf>,
    /// Auto-install makedepends before building
    ///
    /// When true, the Kitchen will check and install makedepends before
    /// starting the build, and optionally clean them up afterward.
    pub auto_makedepends: bool,
    /// Clean up makedepends after build completes
    ///
    /// Only meaningful if auto_makedepends is true.
    pub cleanup_makedepends: bool,
}

impl Default for KitchenConfig {
    fn default() -> Self {
        let jobs = std::thread::available_parallelism()
            .map(|p| p.get() as u32)
            .unwrap_or(4);

        Self {
            source_cache: PathBuf::from("/var/cache/conary/sources"),
            timeout: Duration::from_secs(3600), // 1 hour
            jobs,
            allow_network: false,
            keep_builddir: false,
            use_isolation: true, // On by default for security and reproducibility
            memory_limit: 4 * 1024 * 1024 * 1024, // 4 GB for builds
            cpu_time_limit: 0, // No CPU time limit (builds can be long)
            pristine_mode: false,
            sysroot: None,
            auto_makedepends: false, // Off by default, requires resolver
            cleanup_makedepends: true, // Clean up by default when auto is enabled
        }
    }
}

impl KitchenConfig {
    /// Create a configuration for bootstrap builds
    ///
    /// Enables pristine mode with the specified sysroot. This ensures
    /// builds don't depend on host system libraries/tools.
    pub fn for_bootstrap(sysroot: &Path) -> Self {
        Self {
            use_isolation: true,
            pristine_mode: true,
            sysroot: Some(sysroot.to_path_buf()),
            // In bootstrap mode, we typically don't auto-install makedepends
            // since the sysroot should already be configured with the toolchain
            auto_makedepends: false,
            cleanup_makedepends: false,
            ..Self::default()
        }
    }

    /// Create a configuration with makedepends auto-resolution
    ///
    /// This is useful for building packages in a normal environment
    /// where missing build dependencies should be automatically installed.
    pub fn with_auto_makedepends(cleanup: bool) -> Self {
        Self {
            auto_makedepends: true,
            cleanup_makedepends: cleanup,
            ..Self::default()
        }
    }
}

/// Result of cooking a recipe
#[derive(Debug)]
pub struct CookResult {
    /// Path to the built CCS package
    pub package_path: PathBuf,
    /// Build log
    pub log: String,
    /// Warnings generated during build
    pub warnings: Vec<String>,
    /// Makedepends resolution result (if auto_makedepends was enabled)
    pub makedepends: Option<super::makedepends::MakedependsResult>,
    /// Whether this result came from cache
    pub from_cache: bool,
    /// Cache key used (if caching was enabled)
    pub cache_key: Option<String>,
    /// Provenance data captured during the build
    pub provenance: Option<crate::ccs::manifest::ManifestProvenance>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kitchen_config_default() {
        let config = KitchenConfig::default();
        assert!(config.jobs > 0);
        assert!(!config.allow_network);
        assert!(!config.keep_builddir);
        assert!(!config.auto_makedepends);
        assert!(config.cleanup_makedepends);
        // Isolation should be ON by default for security
        assert!(config.use_isolation);
    }

    #[test]
    fn test_kitchen_config_for_bootstrap() {
        let config = KitchenConfig::for_bootstrap(Path::new("/opt/stage0"));
        assert!(config.use_isolation);
        assert!(config.pristine_mode);
        assert_eq!(config.sysroot, Some(PathBuf::from("/opt/stage0")));
        assert!(!config.auto_makedepends);
        assert!(!config.cleanup_makedepends);
    }

    #[test]
    fn test_kitchen_config_with_auto_makedepends() {
        let config = KitchenConfig::with_auto_makedepends(true);
        assert!(config.auto_makedepends);
        assert!(config.cleanup_makedepends);

        let config_no_cleanup = KitchenConfig::with_auto_makedepends(false);
        assert!(config_no_cleanup.auto_makedepends);
        assert!(!config_no_cleanup.cleanup_makedepends);
    }
}
