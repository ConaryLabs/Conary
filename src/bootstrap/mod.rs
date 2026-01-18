// src/bootstrap/mod.rs

//! Bootstrap infrastructure for building Conary from scratch
//!
//! This module provides the tooling to bootstrap a complete Conary system
//! without relying on an existing package manager. The bootstrap process
//! follows a staged approach:
//!
//! - **Stage 0**: Cross-compilation toolchain (built with crosstool-ng)
//! - **Stage 1**: Self-hosted toolchain (built with Stage 0)
//! - **Stage 2**: Fully native toolchain (optional rebuild for purity)
//! - **Final**: Production packages built with Stage 1/2 toolchain
//!
//! # Architecture
//!
//! ```text
//! Host System (any Linux)
//!      │
//!      ▼
//! ┌─────────────────────────────────────────────┐
//! │  Stage 0: crosstool-ng                       │
//! │  Produces: /tools/x86_64-conary-linux-gnu/   │
//! │  Static cross-compiler (gcc, glibc, binutils)│
//! └─────────────────────────────────────────────┘
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

mod config;
mod stage0;
mod stage1;
mod stages;
mod toolchain;

pub use config::{BootstrapConfig, TargetArch};
pub use stage0::{Stage0Builder, Stage0Error, Stage0Status};
pub use stage1::{PackageBuildStatus, Stage1Builder, Stage1Error, Stage1Package};
pub use stages::{BootstrapStage, StageManager, StageState};
pub use toolchain::{Toolchain, ToolchainKind};

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Default paths for bootstrap artifacts
pub const DEFAULT_TOOLS_DIR: &str = "/tools";
pub const DEFAULT_STAGE1_DIR: &str = "/conary/stage1";
pub const DEFAULT_SYSROOT_DIR: &str = "/conary/sysroot";

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

    /// Check if crosstool-ng is available
    pub fn check_prerequisites(&self) -> Result<Prerequisites> {
        Prerequisites::check()
    }

    /// Build Stage 0 toolchain using crosstool-ng
    pub fn build_stage0(&mut self) -> Result<Toolchain> {
        let mut builder = Stage0Builder::new(&self.work_dir, &self.config)?;
        let toolchain = builder.build()?;

        self.stages
            .mark_complete(BootstrapStage::Stage0, &toolchain.path)?;

        Ok(toolchain)
    }

    /// Get the Stage 0 toolchain if it's already built
    pub fn get_stage0_toolchain(&self) -> Option<Toolchain> {
        self.stages
            .get_artifact_path(BootstrapStage::Stage0)
            .and_then(|p| Toolchain::from_prefix(&p).ok())
    }

    /// Build Stage 1 toolchain using Stage 0
    pub fn build_stage1(&mut self, recipe_dir: impl AsRef<Path>) -> Result<Toolchain> {
        // Get Stage 0 toolchain
        let stage0 = self
            .get_stage0_toolchain()
            .ok_or_else(|| anyhow::anyhow!("Stage 0 toolchain not found. Run stage0 first."))?;

        let mut builder = Stage1Builder::new(&self.work_dir, &self.config, stage0)?;
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

    /// Resume bootstrap from last checkpoint
    pub fn resume(&mut self) -> Result<BootstrapStage> {
        self.stages.current_stage()
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
}
