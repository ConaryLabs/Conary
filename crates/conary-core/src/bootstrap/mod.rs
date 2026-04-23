// conary-core/src/bootstrap/mod.rs

//! Bootstrap infrastructure for building Conary from scratch
//!
//! This module provides the tooling to bootstrap a complete Conary system
//! without relying on an existing package manager. The bootstrap process
//! follows a 6-phase approach aligned with Linux From Scratch 13:
//!
//! - **Phase 1 (CrossTools)**: Cross-toolchain (LFS Ch5)
//! - **Phase 2 (TempTools)**: Temporary tools (LFS Ch6-7)
//! - **Phase 3 (FinalSystem)**: Complete system (LFS Ch8)
//! - **Phase 4 (SystemConfig)**: System configuration (LFS Ch9)
//! - **Phase 5 (BootableImage)**: Bootable image (LFS Ch10)
//! - **Phase 6 (Tier2)**: BLFS + Conary self-hosting
//!
//! # Architecture
//!
//! ```text
//! Host System (any Linux with gcc)
//!      │
//!      ▼ (cross-compiles)
//! ┌─────────────────────────────────────────────┐
//! │  Phase 1: Cross-toolchain (LFS Ch5)          │
//! │  Produces: $LFS/tools/                       │
//! │  Cross binutils, cross-GCC, glibc, libstdc++ │
//! └─────────────────────────────────────────────┘
//!      │
//!      ▼ (cross-compiles + chroot builds)
//! ┌─────────────────────────────────────────────┐
//! │  Phase 2: Temporary tools (LFS Ch6-7)        │
//! │  17 cross-compiled + 6 chroot packages       │
//! └─────────────────────────────────────────────┘
//!      │
//!      ▼ (builds inside chroot)
//! ┌─────────────────────────────────────────────┐
//! │  Phase 3: Final system (LFS Ch8)             │
//! │  82 packages -- complete Linux system        │
//! └─────────────────────────────────────────────┘
//!      │
//!      ▼ (configures)
//! ┌─────────────────────────────────────────────┐
//! │  Phase 4: System configuration (LFS Ch9)     │
//! │  Network, fstab, kernel, bootloader          │
//! └─────────────────────────────────────────────┘
//!      │
//!      ▼ (images)
//! ┌─────────────────────────────────────────────┐
//! │  Phase 5: Bootable image (LFS Ch10)          │
//! │  Ready to boot in VM or on hardware          │
//! └─────────────────────────────────────────────┘
//!      │
//!      ▼ (extends)
//! ┌─────────────────────────────────────────────┐
//! │  Phase 6: Tier 2 (BLFS + Conary)             │
//! │  PAM, OpenSSH, curl, Rust, Conary            │
//! └─────────────────────────────────────────────┘
//! ```

pub mod adopt_seed;
mod build_helpers;
mod build_runner;
pub mod chroot_env;
mod config;
mod cross_tools;
mod final_system;
mod guest_profile;
mod image;
mod stages;
mod system_config;
mod temp_tools;
mod tier2;
mod toolchain;

pub use build_runner::{BuildRunnerError, PackageBuildRunner};
pub use config::{BootstrapConfig, TargetArch};
pub use cross_tools::{CrossToolsBuilder, CrossToolsError};
pub use final_system::{FinalSystemBuilder, FinalSystemError, SYSTEM_BUILD_ORDER};
pub use guest_profile::{GuestProfileError, apply_guest_profile};
pub use image::{ImageBuilder, ImageError, ImageFormat, ImageResult, ImageSize, ImageTools};
pub use stages::{BootstrapStage, StageManager, StageState};
pub use system_config::{SystemConfigError, configure_system};
pub use temp_tools::{TempToolsBuilder, TempToolsError};
pub use tier2::{Tier2Builder, Tier2Error};
pub use toolchain::{Toolchain, ToolchainKind};

use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::info;

/// Default paths for bootstrap artifacts
pub const DEFAULT_TOOLS_DIR: &str = "/tools";
pub const DEFAULT_SYSROOT_DIR: &str = "/conary/sysroot";

/// Validate that a recipe field does not contain shell injection characters.
///
/// Rejects backticks, `$()`, semicolons, and pipes that could allow arbitrary
/// command execution when the value is interpolated into a shell script.
///
/// Returns `Ok(())` if the value is safe, or `Err(msg)` describing the
/// first forbidden pattern found.
fn validate_shell_safe(value: &str, field: &str) -> Result<(), String> {
    // Ordered by likely occurrence; we report the first hit.
    if value.contains('`') {
        return Err(format!("{field} contains backtick (shell injection risk)"));
    }
    if value.contains("$(") {
        return Err(format!("{field} contains $() (shell injection risk)"));
    }
    if value.contains(';') {
        return Err(format!("{field} contains semicolon (shell injection risk)"));
    }
    if value.contains('|') {
        return Err(format!("{field} contains pipe (shell injection risk)"));
    }
    Ok(())
}

/// Assemble a build script from recipe fields with variable substitution.
///
/// Used by chroot builds (Phase 2b and 3) where the Kitchen cannot run
/// directly. Each build phase (setup, configure, make, install, post_install)
/// is concatenated into a single `set -e` script.
///
/// Before interpolation, `recipe.package.version` and `recipe.package.name`
/// are validated against a shell-injection denylist (backticks, `$()`,
/// semicolons, pipes). If either field fails validation the offending recipe
/// field is replaced with an `echo` error line and the script is returned as
/// a no-op that exits non-zero so the build fails loudly rather than silently
/// executing injected commands.
pub fn assemble_build_script(recipe: &crate::recipe::Recipe, destdir: &str) -> String {
    // Validate fields that are interpolated as `%(version)s` / `%(name)s`.
    for (field_name, field_value) in [
        ("package.version", recipe.package.version.as_str()),
        ("package.name", recipe.package.name.as_str()),
    ] {
        if let Err(msg) = validate_shell_safe(field_value, field_name) {
            return format!("set -e\necho 'ERROR: {msg}' >&2\nexit 1\n");
        }
    }

    let mut script = String::from("set -e\n");
    for phase in [
        &recipe.build.setup,
        &recipe.build.configure,
        &recipe.build.make,
        &recipe.build.install,
        &recipe.build.post_install,
    ]
    .into_iter()
    .flatten()
    {
        let substituted = recipe.substitute(phase, destdir);
        script.push_str(&substituted);
        script.push('\n');
    }
    script
}

/// Assemble a chroot build script that first enters the staged source tree.
///
/// Chrooted bootstrap phases stage unpacked sources inside the sysroot under a
/// deterministic path, then run the recipe phases from that directory.
pub fn assemble_chroot_build_script(
    recipe: &crate::recipe::Recipe,
    src_dir_in_chroot: &str,
    destdir: &str,
) -> String {
    for (field_name, field_value) in [
        ("package.version", recipe.package.version.as_str()),
        ("package.name", recipe.package.name.as_str()),
    ] {
        if let Err(msg) = validate_shell_safe(field_value, field_name) {
            return format!("set -e\necho 'ERROR: {msg}' >&2\nexit 1\n");
        }
    }

    let mut script = String::from("set -e\n");
    for phase in [
        &recipe.build.setup,
        &recipe.build.configure,
        &recipe.build.make,
        &recipe.build.install,
        &recipe.build.post_install,
    ]
    .into_iter()
    .flatten()
    {
        script.push_str("cd ");
        script.push_str(src_dir_in_chroot);
        script.push('\n');
        script.push_str(&recipe.substitute(phase, destdir));
        script.push('\n');
    }

    script
}

/// Report from a dry-run validation.
#[derive(Debug, Default)]
pub struct DryRunReport {
    /// Number of cross-tools recipes found
    pub cross_tools_count: usize,
    /// Number of system recipes found
    pub system_count: usize,
    /// Number of Tier-2 recipes found
    pub tier2_count: usize,
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

    /// Get the cross-toolchain if it has already been built.
    pub fn get_cross_toolchain(&self) -> Option<Toolchain> {
        self.stages
            .get_artifact_path(BootstrapStage::CrossTools)
            .and_then(|p| Toolchain::from_prefix(&p).ok())
    }

    // -----------------------------------------------------------------
    // 6-phase LFS-aligned pipeline methods
    // -----------------------------------------------------------------

    /// Build Phase 1: Cross-toolchain (LFS Chapter 5).
    ///
    /// Uses the host compiler to build a cross-toolchain targeting `LFS_TGT`.
    /// The output lives under `$LFS/tools/` and is consumed by Phase 2.
    pub fn build_cross_tools(&mut self) -> Result<Toolchain> {
        let host =
            Toolchain::host().map_err(|e| anyhow::anyhow!("Host toolchain not found: {e}"))?;

        let lfs_root = &self.config.lfs_root.clone();
        std::fs::create_dir_all(lfs_root)?;

        let completed = self.stages.completed_packages(BootstrapStage::CrossTools);

        let builder = CrossToolsBuilder::new(&self.work_dir, lfs_root, self.config.clone(), host)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let toolchain = builder
            .build_all(&completed)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        self.stages
            .mark_complete(BootstrapStage::CrossTools, &toolchain.path)?;

        Ok(toolchain)
    }

    /// Build Phase 2: Temporary tools (LFS Chapters 6-7).
    ///
    /// Uses the Phase 1 cross-toolchain to cross-compile utilities, then
    /// sets up a chroot and builds additional packages natively inside it.
    pub fn build_temp_tools(&mut self) -> Result<()> {
        let cross_tc = self.get_cross_toolchain().ok_or_else(|| {
            anyhow::anyhow!("Phase 1 cross-toolchain not found. Run cross-tools first.")
        })?;

        let lfs_root = &self.config.lfs_root.clone();

        let completed = self.stages.completed_packages(BootstrapStage::TempTools);

        let builder =
            TempToolsBuilder::new(&self.work_dir, lfs_root, self.config.clone(), cross_tc)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

        builder
            .build_cross_packages(&completed, &mut self.stages)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // IMPORTANT: chroot_env must stay alive until build_chroot_packages() completes.
        let chroot_env = builder.setup_chroot().map_err(|e| anyhow::anyhow!("{e}"))?;
        builder
            .build_chroot_packages(&completed, &mut self.stages)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        drop(chroot_env);

        self.stages
            .mark_complete(BootstrapStage::TempTools, lfs_root)?;

        Ok(())
    }

    /// Build Phase 3: Final system (LFS Chapter 8).
    ///
    /// Builds all 82 packages of the complete LFS system inside the chroot.
    pub fn build_final_system(&mut self) -> Result<()> {
        let lfs_root = &self.config.lfs_root.clone();

        // Use the system toolchain that is now available inside the chroot
        let toolchain = Toolchain {
            kind: ToolchainKind::System,
            path: lfs_root.join("usr"),
            target: self.config.triple().to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let completed = self.stages.completed_packages(BootstrapStage::FinalSystem);

        let mut builder =
            FinalSystemBuilder::new(&self.work_dir, lfs_root, self.config.clone(), toolchain)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

        // IMPORTANT: chroot_env must stay alive until build_all() completes.
        let chroot_env = builder.setup_chroot().map_err(|e| anyhow::anyhow!("{e}"))?;
        builder
            .build_all(&completed, &mut self.stages)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        drop(chroot_env);

        self.stages
            .mark_complete(BootstrapStage::FinalSystem, lfs_root)?;

        Ok(())
    }

    /// Run Phase 4: System configuration (LFS Chapter 9).
    ///
    /// Configures network, fstab, kernel, and bootloader on the built system.
    pub fn configure_system(&mut self) -> Result<()> {
        let lfs_root = &self.config.lfs_root.clone();

        system_config::configure_system(lfs_root).map_err(|e| anyhow::anyhow!("{e}"))?;

        self.stages
            .mark_complete(BootstrapStage::SystemConfig, lfs_root)?;

        Ok(())
    }

    /// Build Phase 6: Tier-2 packages (BLFS + Conary self-hosting).
    ///
    /// Builds additional packages needed for Conary to manage itself:
    /// PAM, OpenSSH, make-ca, curl, sudo, nano, Rust, and Conary.
    pub fn build_tier2(&mut self) -> Result<()> {
        let lfs_root = &self.config.lfs_root.clone();

        let toolchain = Toolchain {
            kind: ToolchainKind::System,
            path: lfs_root.join("usr"),
            target: self.config.triple().to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let builder = Tier2Builder::new(&self.work_dir, lfs_root, self.config.clone(), toolchain)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        match builder.build_all() {
            Ok(()) => {
                info!("Tier-2 builds complete");
                self.stages.mark_complete(BootstrapStage::Tier2, lfs_root)?;
            }
            Err(e) => return Err(anyhow::anyhow!("{e}")),
        }

        Ok(())
    }

    /// Apply the self-host test guest profile to an already-built sysroot.
    pub fn apply_guest_profile(&self, public_key: &Path) -> Result<()> {
        guest_profile::apply_guest_profile(&self.config.lfs_root, public_key)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Get the LFS root path from configuration.
    pub fn lfs_root(&self) -> &Path {
        &self.config.lfs_root
    }

    /// Validate the full pipeline without building anything.
    pub fn dry_run(&self, recipe_dir: &Path) -> Result<DryRunReport> {
        let mut report = DryRunReport::default();

        // Check cross-tools recipes (Phase 1)
        let cross_tools_dir = recipe_dir.join("cross-tools");
        if cross_tools_dir.exists() {
            for name in &[
                "linux-headers",
                "binutils-pass1",
                "gcc-pass1",
                "glibc",
                "libstdcxx",
            ] {
                let path = cross_tools_dir.join(format!("{name}.toml"));
                if path.exists() {
                    match crate::recipe::parse_recipe_file(&path) {
                        Ok(recipe) => {
                            report.cross_tools_count += 1;
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
                        .push(format!("Missing cross-tools recipe: {name}"));
                }
            }
        } else {
            report
                .warnings
                .push("cross-tools recipe directory not found".to_string());
        }

        // Check system recipes and graph resolution (Phase 3)
        let system_dir = recipe_dir.join("system");
        if system_dir.exists() {
            let mut graph = crate::recipe::RecipeGraph::new();
            for entry in std::fs::read_dir(&system_dir)? {
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
                            report.system_count += 1;
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
                    .push(format!("Dependency cycle in system recipes: {e}")),
            }
        } else {
            report
                .warnings
                .push("system recipe directory not found".to_string());
        }

        // Check Tier-2 recipes
        let tier2_dir = recipe_dir.join("tier2");
        if tier2_dir.exists() {
            for entry in std::fs::read_dir(&tier2_dir)? {
                let path = entry?.path();
                if path.extension().is_some_and(|e| e == "toml") {
                    match crate::recipe::parse_recipe_file(&path) {
                        Ok(_) => report.tier2_count += 1,
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

    /// Get the base system sysroot path if built.
    ///
    /// Returns the recorded `FinalSystem` artifact path when available.
    ///
    /// Completed bootstrap runs may have been built with a custom `--lfs-root`,
    /// so image generation must prefer the checkpointed artifact path from the
    /// work directory over the config default.
    pub fn get_sysroot(&self) -> Option<PathBuf> {
        self.stages
            .get_artifact_path(BootstrapStage::FinalSystem)
            .or_else(|| {
                self.stages
                    .is_complete(BootstrapStage::FinalSystem)
                    .then(|| self.config.lfs_root.clone())
            })
    }

    /// Build a bootable image from the base system
    pub fn build_image(
        &mut self,
        output: impl AsRef<Path>,
        format: ImageFormat,
        size: ImageSize,
    ) -> Result<ImageResult> {
        // Get sysroot path
        let sysroot = self.get_sysroot().ok_or_else(|| {
            anyhow::anyhow!("Base system not found. Run 'bootstrap system' first.")
        })?;

        let mut builder = ImageBuilder::new(
            &self.work_dir,
            &self.config,
            &sysroot,
            output.as_ref(),
            format,
            size,
        )?;

        let result = match format {
            ImageFormat::Erofs => builder.build()?,
            _ => builder.build_tier1_image()?,
        };

        self.stages
            .mark_complete(BootstrapStage::BootableImage, &result.path)?;

        Ok(result)
    }
}

/// Prerequisites for bootstrap
#[derive(Debug)]
pub struct Prerequisites {
    pub make: Option<String>,
    pub gcc: Option<String>,
    pub git: Option<String>,
}

impl Prerequisites {
    /// Check for required tools
    pub fn check() -> Result<Self> {
        Ok(Self {
            make: Self::find_version("make", &["--version"]),
            gcc: Self::find_version("gcc", &["--version"]),
            git: Self::find_version("git", &["--version"]),
        })
    }

    /// Check if all required prerequisites are met
    pub fn all_present(&self) -> bool {
        self.make.is_some() && self.gcc.is_some() && self.git.is_some()
    }

    /// Get list of missing prerequisites
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
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
    use crate::recipe::parser::parse_recipe_file;
    use std::path::Path;

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
    fn test_get_sysroot_prefers_recorded_final_system_artifact_path() {
        let temp = tempfile::tempdir().unwrap();
        let mut bootstrap = Bootstrap::new(temp.path()).unwrap();
        let custom_sysroot = temp.path().join("custom-lfs-root");
        std::fs::create_dir_all(&custom_sysroot).unwrap();

        bootstrap
            .stages
            .mark_complete(BootstrapStage::FinalSystem, &custom_sysroot)
            .unwrap();

        assert_eq!(bootstrap.get_sysroot(), Some(custom_sysroot));
    }

    #[test]
    fn test_dry_run_with_recipes() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let bootstrap = Bootstrap::with_config(dir.path(), config).unwrap();

        // The dry_run test requires a complete, cycle-free set of recipes.
        // Skip when recipes are unavailable (CI) or the graph is incomplete.
        if !std::path::Path::new("recipes/cross-tools").exists() {
            eprintln!("Skipping: recipes/cross-tools not found in cwd");
            return;
        }

        let recipe_dir = std::path::Path::new("recipes");
        let report = bootstrap.dry_run(recipe_dir).unwrap();
        assert_eq!(
            report.cross_tools_count, 5,
            "Expected 5 cross-tools recipes"
        );
        assert!(
            report.system_count >= 10,
            "Expected at least 10 system recipes"
        );
        assert!(report.graph_resolved, "Graph should resolve");
    }

    #[test]
    fn test_dry_run_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let bootstrap = Bootstrap::with_config(dir.path(), config).unwrap();

        let recipe_dir = dir.path().join("nonexistent_recipes");
        let report = bootstrap.dry_run(&recipe_dir).unwrap();

        // With no recipe dirs, we should get warnings but no errors
        assert_eq!(report.cross_tools_count, 0);
        assert_eq!(report.system_count, 0);
        assert_eq!(report.tier2_count, 0);
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

    #[test]
    fn test_assemble_chroot_build_script_changes_into_staged_source_dir() {
        let recipe_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes/temp-tools/gettext.toml");
        let recipe = parse_recipe_file(&recipe_path).unwrap();

        let script = assemble_chroot_build_script(
            &recipe,
            "/var/tmp/conary-bootstrap/temp-tools/gettext/src",
            "/",
        );

        assert!(
            script.starts_with("set -e\ncd /var/tmp/conary-bootstrap/temp-tools/gettext/src\n")
        );
        assert!(script.contains("./configure --disable-shared"));
    }

    #[test]
    fn test_assemble_chroot_build_script_resets_to_source_root_between_phases() {
        let recipe_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes/system/glibc.toml");
        let recipe = parse_recipe_file(&recipe_path).unwrap();

        let source_root = "/var/tmp/conary-bootstrap/final-system/glibc/src";
        let script = assemble_chroot_build_script(&recipe, source_root, "/");

        assert_eq!(script.matches(&format!("cd {source_root}\n")).count(), 3);
        assert!(script.contains(&format!("cd {source_root}\nmkdir -v build\ncd build")));
        assert!(script.contains(&format!("cd {source_root}\ncd build\nmake -j")));
    }
}
