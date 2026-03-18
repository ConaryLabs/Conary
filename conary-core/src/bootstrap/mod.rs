// conary-core/src/bootstrap/mod.rs

//! Bootstrap infrastructure for building Conary from scratch
//!
//! This module provides the tooling to bootstrap a complete Conary system
//! without relying on an existing package manager. The bootstrap process
//! follows a staged approach aligned with Linux From Scratch 13:
//!
//! - **Stage 1**: Self-hosted toolchain (cross-compiled from host)
//! - **Base System**: Core packages (kernel, glibc, coreutils, networking)
//! - **Conary**: Self-hosting (Rust + Conary built in-sysroot)
//! - **Image**: Bootable image generation
//!
//! # Architecture
//!
//! ```text
//! Host System (any Linux with gcc)
//!      │
//!      ▼ (cross-compiles)
//! ┌─────────────────────────────────────────────┐
//! │  Stage 1: Self-hosted toolchain              │
//! │  Produces: /conary/stage1/                   │
//! │  Native gcc, glibc, binutils                 │
//! └─────────────────────────────────────────────┘
//!      │
//!      ▼ (builds)
//! ┌─────────────────────────────────────────────┐
//! │  Base System                                 │
//! │  Kernel, systemd, coreutils, networking...   │
//! └─────────────────────────────────────────────┘
//!      │
//!      ▼
//! ┌─────────────────────────────────────────────┐
//! │  Bootable Image                              │
//! │  Ready to boot in VM or on hardware          │
//! └─────────────────────────────────────────────┘
//! ```

mod base;
mod build_helpers;
mod build_runner;
mod conary_stage;
mod config;
mod cross_tools;
mod final_system;
mod image;
pub(crate) mod repart;
mod stage1;
mod stages;
mod system_config;
mod temp_tools;
mod tier2;
mod toolchain;

pub use base::{
    BaseBuildPhase, BaseBuildStatus, BaseBuilder, BaseError, BasePackage, BuildSummary,
};
pub use build_runner::{BuildRunnerError, PackageBuildRunner};
pub use conary_stage::{ConaryStageBuilder, ConaryStageError};
pub use config::{BootstrapConfig, TargetArch};
pub use cross_tools::{CrossToolsBuilder, CrossToolsError};
pub use final_system::{FinalSystemBuilder, FinalSystemError, SYSTEM_BUILD_ORDER};
pub use image::{ImageBuilder, ImageError, ImageFormat, ImageResult, ImageSize, ImageTools};
pub use stage1::{PackageBuildStatus, Stage1Builder, Stage1Error, Stage1Package};
pub use stages::{BootstrapStage, StageManager, StageState};
pub use system_config::{SystemConfigError, configure_system};
pub use temp_tools::{TempToolsBuilder, TempToolsError};
pub use tier2::{Tier2Builder, Tier2Error};
pub use toolchain::{Toolchain, ToolchainKind};

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Default paths for bootstrap artifacts
pub const DEFAULT_TOOLS_DIR: &str = "/tools";
pub const DEFAULT_STAGE1_DIR: &str = "/conary/stage1";
pub const DEFAULT_SYSROOT_DIR: &str = "/conary/sysroot";

/// Report from a dry-run validation.
#[derive(Debug, Default)]
pub struct DryRunReport {
    /// Number of Stage 1 recipes found
    pub stage1_count: usize,
    /// Number of base system recipes found
    pub base_count: usize,
    /// Number of Conary recipes found
    pub conary_count: usize,
    /// Whether the dependency graph resolved without cycles
    pub graph_resolved: bool,
    /// Number of placeholder checksums found
    pub placeholder_count: usize,
    /// Errors found during validation
    pub errors: Vec<String>,
    /// Warnings found during validation
    pub warnings: Vec<String>,
}

impl DryRunReport {
    /// Returns `true` if no errors and no placeholder checksums were found.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty() && self.placeholder_count == 0
    }
}

/// Bootstrap orchestrator that coordinates the entire bootstrap process
pub struct Bootstrap {
    /// Configuration for this bootstrap
    config: BootstrapConfig,

    /// Stage manager for tracking progress
    stages: StageManager,

    /// Base directory for bootstrap work
    work_dir: PathBuf,
}

impl Bootstrap {
    /// Create a new bootstrap orchestrator
    pub fn new(work_dir: impl AsRef<Path>) -> Result<Self> {
        let work_dir = work_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&work_dir)?;

        Ok(Self {
            config: BootstrapConfig::default(),
            stages: StageManager::new(&work_dir)?,
            work_dir,
        })
    }

    /// Create with custom configuration
    pub fn with_config(work_dir: impl AsRef<Path>, config: BootstrapConfig) -> Result<Self> {
        let work_dir = work_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&work_dir)?;

        Ok(Self {
            config,
            stages: StageManager::new(&work_dir)?,
            work_dir,
        })
    }

    /// Get the work directory
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Get the configuration
    pub fn config(&self) -> &BootstrapConfig {
        &self.config
    }

    /// Get the stage manager
    pub fn stages(&self) -> &StageManager {
        &self.stages
    }

    /// Get the stage manager (mutable)
    pub fn stages_mut(&mut self) -> &mut StageManager {
        &mut self.stages
    }

    /// Check if prerequisites are available
    pub fn check_prerequisites(&self) -> Result<Prerequisites> {
        Prerequisites::check()
    }

    /// Build Stage 1 toolchain using the host cross-compiler
    pub fn build_stage1(&mut self, recipe_dir: impl AsRef<Path>) -> Result<Toolchain> {
        let host = Toolchain::host()
            .map_err(|e| anyhow::anyhow!("Host toolchain not found: {e}"))?;

        let mut builder = Stage1Builder::new(&self.work_dir, &self.config, host)?;
        builder.load_recipes(recipe_dir)?;

        let toolchain = builder.build()?;

        self.stages
            .mark_complete(BootstrapStage::Stage1, &toolchain.path)?;

        Ok(toolchain)
    }

    /// Get the Stage 1 toolchain if it's already built
    pub fn get_stage1_toolchain(&self) -> Option<Toolchain> {
        self.stages
            .get_artifact_path(BootstrapStage::Stage1)
            .and_then(|p| Toolchain::from_prefix(&p).ok())
    }

    /// Build base system using Stage 1 toolchain
    pub fn build_base(
        &mut self,
        recipe_dir: impl AsRef<Path>,
        target_root: impl AsRef<Path>,
    ) -> Result<BuildSummary> {
        // Get Stage 1 toolchain
        let stage1 = self
            .get_stage1_toolchain()
            .ok_or_else(|| anyhow::anyhow!("Stage 1 toolchain not found. Run stage1 first."))?;

        let mut builder = BaseBuilder::new(
            &self.work_dir,
            &self.config,
            stage1,
            target_root.as_ref(),
            recipe_dir.as_ref(),
        )?;

        builder.init_packages()?;
        builder.build()?;

        self.stages
            .mark_complete(BootstrapStage::BaseSystem, builder.target_root())?;

        Ok(builder.summary())
    }

    /// Build the Conary self-hosting stage (Rust + Conary).
    ///
    /// Downloads a Rust bootstrap compiler, builds Rust from source, then
    /// compiles Conary itself. After this stage the bootstrapped system can
    /// manage its own packages.
    pub fn build_conary_stage(&mut self) -> Result<()> {
        let sysroot = self
            .get_sysroot()
            .ok_or_else(|| anyhow::anyhow!("Base system not found. Run base first."))?;

        let builder = ConaryStageBuilder::new(
            self.work_dir.join("conary-stage"),
            self.config.clone(),
            sysroot.clone(),
        );

        builder.build()?;

        self.stages
            .mark_complete(BootstrapStage::Conary, &sysroot)?;

        Ok(())
    }

    /// Validate the full pipeline without building anything.
    pub fn dry_run(&self, recipe_dir: &Path) -> Result<DryRunReport> {
        let mut report = DryRunReport::default();

        // Check Stage 1 recipes
        let stage1_dir = recipe_dir.join("stage1");
        if stage1_dir.exists() {
            for name in &[
                "linux-headers",
                "binutils",
                "gcc-pass1",
                "glibc",
                "gcc-pass2",
            ] {
                let path = stage1_dir.join(format!("{name}.toml"));
                if path.exists() {
                    match crate::recipe::parse_recipe_file(&path) {
                        Ok(recipe) => {
                            report.stage1_count += 1;
                            if recipe.source.checksum.contains("VERIFY_BEFORE_BUILD")
                                || recipe.source.checksum.contains("FIXME")
                            {
                                report.placeholder_count += 1;
                                report
                                    .errors
                                    .push(format!("Placeholder checksum in {name}"));
                            }
                        }
                        Err(e) => report.errors.push(format!("Failed to parse {name}: {e}")),
                    }
                } else {
                    report
                        .errors
                        .push(format!("Missing Stage 1 recipe: {name}"));
                }
            }
        } else {
            report
                .warnings
                .push("Stage 1 recipe directory not found".to_string());
        }

        // Check Base recipes and graph resolution
        let base_dir = recipe_dir.join("base");
        if base_dir.exists() {
            let mut graph = crate::recipe::RecipeGraph::new();
            for entry in std::fs::read_dir(&base_dir)? {
                let path = entry?.path();
                if path.extension().is_some_and(|e| e == "toml") {
                    match crate::recipe::parse_recipe_file(&path) {
                        Ok(recipe) => {
                            if recipe.source.checksum.contains("VERIFY_BEFORE_BUILD")
                                || recipe.source.checksum.contains("FIXME")
                            {
                                report.placeholder_count += 1;
                            }
                            graph.add_from_recipe(&recipe);
                            report.base_count += 1;
                        }
                        Err(e) => report
                            .errors
                            .push(format!("Failed to parse {}: {e}", path.display())),
                    }
                }
            }
            match graph.topological_sort() {
                Ok(_) => report.graph_resolved = true,
                Err(e) => report
                    .errors
                    .push(format!("Dependency cycle in base recipes: {e}")),
            }
        } else {
            report
                .warnings
                .push("Base recipe directory not found".to_string());
        }

        // Check Conary recipes
        let conary_dir = recipe_dir.join("conary");
        if conary_dir.exists() {
            for entry in std::fs::read_dir(&conary_dir)? {
                let path = entry?.path();
                if path.extension().is_some_and(|e| e == "toml") {
                    match crate::recipe::parse_recipe_file(&path) {
                        Ok(_) => report.conary_count += 1,
                        Err(e) => report
                            .errors
                            .push(format!("Failed to parse {}: {e}", path.display())),
                    }
                }
            }
        }

        Ok(report)
    }

    /// Resume bootstrap from last checkpoint
    ///
    /// Returns `None` when all stages are complete.
    pub fn resume(&mut self) -> Result<Option<BootstrapStage>> {
        self.stages.current_stage()
    }

    /// Get the base system sysroot path if built
    pub fn get_sysroot(&self) -> Option<PathBuf> {
        self.stages.get_artifact_path(BootstrapStage::BaseSystem)
    }

    /// Build a bootable image from the base system
    pub fn build_image(
        &mut self,
        output: impl AsRef<Path>,
        format: ImageFormat,
        size: ImageSize,
    ) -> Result<ImageResult> {
        // Get sysroot path
        let sysroot = self
            .get_sysroot()
            .ok_or_else(|| anyhow::anyhow!("Base system not found. Run base first."))?;

        let mut builder = ImageBuilder::new(
            &self.work_dir,
            &self.config,
            &sysroot,
            output.as_ref(),
            format,
            size,
        )?;

        let result = builder.build()?;

        self.stages
            .mark_complete(BootstrapStage::Image, &result.path)?;

        Ok(result)
    }
}

/// Prerequisites for bootstrap
#[derive(Debug)]
pub struct Prerequisites {
    pub crosstool_ng: Option<String>,
    pub make: Option<String>,
    pub gcc: Option<String>,
    pub git: Option<String>,
}

impl Prerequisites {
    /// Check for required tools
    pub fn check() -> Result<Self> {
        Ok(Self {
            crosstool_ng: Self::find_version("ct-ng", &["version"]),
            make: Self::find_version("make", &["--version"]),
            gcc: Self::find_version("gcc", &["--version"]),
            git: Self::find_version("git", &["--version"]),
        })
    }

    /// Check if all required prerequisites are met
    pub fn all_present(&self) -> bool {
        self.crosstool_ng.is_some()
            && self.make.is_some()
            && self.gcc.is_some()
            && self.git.is_some()
    }

    /// Get list of missing prerequisites
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.crosstool_ng.is_none() {
            missing.push("crosstool-ng (ct-ng)");
        }
        if self.make.is_none() {
            missing.push("make");
        }
        if self.gcc.is_none() {
            missing.push("gcc");
        }
        if self.git.is_none() {
            missing.push("git");
        }
        missing
    }

    fn find_version(cmd: &str, args: &[&str]) -> Option<String> {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout)
                        .ok()
                        .and_then(|s| s.lines().next().map(String::from))
                } else {
                    None
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prerequisites_check() {
        let prereqs = Prerequisites::check().unwrap();
        // At minimum, make and gcc should be present on any dev system
        assert!(prereqs.make.is_some(), "make should be installed");
        assert!(prereqs.gcc.is_some(), "gcc should be installed");
    }

    #[test]
    fn test_bootstrap_new() {
        let temp = tempfile::tempdir().unwrap();
        let bootstrap = Bootstrap::new(temp.path()).unwrap();
        assert!(bootstrap.work_dir().exists());
    }

    #[test]
    fn test_dry_run_with_recipes() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let bootstrap = Bootstrap::with_config(dir.path(), config).unwrap();

        let recipe_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("recipes");
        if !recipe_dir.exists() {
            eprintln!("Skipping: recipes not found");
            return;
        }

        let report = bootstrap.dry_run(&recipe_dir).unwrap();
        assert_eq!(report.stage1_count, 5, "Expected 5 Stage 1 recipes");
        assert!(report.base_count >= 10, "Expected at least 10 base recipes");
        assert!(report.graph_resolved, "Graph should resolve");
        assert_eq!(
            report.placeholder_count, 0,
            "No placeholder checksums allowed in stage1"
        );
    }

    #[test]
    fn test_dry_run_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let bootstrap = Bootstrap::with_config(dir.path(), config).unwrap();

        let recipe_dir = dir.path().join("nonexistent_recipes");
        let report = bootstrap.dry_run(&recipe_dir).unwrap();

        // With no recipe dirs, we should get warnings but no errors
        assert_eq!(report.stage1_count, 0);
        assert_eq!(report.base_count, 0);
        assert_eq!(report.conary_count, 0);
        assert!(!report.warnings.is_empty(), "Should have warnings");
    }

    #[test]
    fn test_dry_run_report_is_ok() {
        let report = DryRunReport::default();
        assert!(report.is_ok(), "Empty report should be ok");

        let report_with_error = DryRunReport {
            errors: vec!["test error".to_string()],
            ..Default::default()
        };
        assert!(
            !report_with_error.is_ok(),
            "Report with error should not be ok"
        );

        let report_with_placeholder = DryRunReport {
            placeholder_count: 1,
            ..Default::default()
        };
        assert!(
            !report_with_placeholder.is_ok(),
            "Report with placeholder should not be ok"
        );
    }
}
