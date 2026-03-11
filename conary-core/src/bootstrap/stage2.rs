// conary-core/src/bootstrap/stage2.rs

//! Stage 2 toolchain builder (reproducibility rebuild)
//!
//! Rebuilds the Stage 1 packages using the Stage 1 compiler instead of the
//! Stage 0 cross-compiler. This verifies that the toolchain can reproduce
//! itself and doesn't depend on the host system.
//!
//! # Purpose
//!
//! Stage 2 exists to prove that the Stage 1 toolchain is self-hosting:
//! if you can rebuild the same 5 packages with the Stage 1 compiler and
//! get identical (or near-identical) output, the bootstrap chain is sound.
//!
//! # Build Order
//!
//! Same as Stage 1:
//!
//! 1. **linux-headers** - Kernel headers for glibc
//! 2. **binutils** - Assembler, linker, and binary tools
//! 3. **gcc-pass1** - Minimal C compiler (no threads, no shared libs)
//! 4. **glibc** - The C library (needs gcc-pass1)
//! 5. **gcc-pass2** - Full GCC with C/C++ and threading support

use super::build_helpers;
use super::build_runner::PackageBuildRunner;
use super::config::BootstrapConfig;
use super::toolchain::{Toolchain, ToolchainKind};
use crate::recipe::{Recipe, parse_recipe_file};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{info, warn};

/// Errors that can occur during Stage 2 build
#[derive(Debug, Error)]
pub enum Stage2Error {
    #[error("Stage 1 toolchain not found")]
    NoStage1Toolchain,

    #[error("Recipe not found: {0}")]
    RecipeNotFound(String),

    #[error("Recipe parse error: {0}")]
    RecipeParseFailed(String),

    #[error("Build failed for {package}: {reason}")]
    BuildFailed { package: String, reason: String },

    #[error("Reproducibility mismatch for {package}: stage1={stage1_hash}, stage2={stage2_hash}")]
    ReproducibilityMismatch {
        package: String,
        stage1_hash: String,
        stage2_hash: String,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("Build runner error: {0}")]
    Runner(super::build_runner::BuildRunnerError),
}

/// Packages rebuilt in Stage 2 (same as Stage 1)
const STAGE2_PACKAGES: &[&str] = &[
    "linux-headers",
    "binutils",
    "gcc-pass1",
    "glibc",
    "gcc-pass2",
];

/// Status of a Stage 2 package build
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage2PackageStatus {
    /// Not started
    Pending,
    /// Building
    Building,
    /// Build complete
    Complete,
    /// Build failed
    Failed(String),
}

/// A single package being built in Stage 2
#[derive(Debug)]
pub struct Stage2Package {
    /// Package name
    pub name: String,
    /// Recipe for this package
    pub recipe: Recipe,
    /// Current build status
    pub status: Stage2PackageStatus,
    /// Build log
    pub log: String,
}

/// Builder for Stage 2 toolchain (reproducibility rebuild)
pub struct Stage2Builder {
    /// Work directory (also serves as the output directory for Stage 2 artifacts)
    work_dir: PathBuf,

    /// Bootstrap configuration
    config: BootstrapConfig,

    /// Stage 1 toolchain (used as the compiler for this stage)
    toolchain: Toolchain,

    /// Sysroot directory for Stage 2
    sysroot: PathBuf,

    /// Build logs directory
    logs_dir: PathBuf,

    /// Packages to build in order
    packages: Vec<Stage2Package>,

    /// Environment variables for builds
    build_env: HashMap<String, String>,

    /// Shared build runner for source fetching and extraction
    runner: PackageBuildRunner,
}

impl Stage2Builder {
    /// Create a new Stage 2 builder
    pub fn new(
        work_dir: impl AsRef<Path>,
        config: &BootstrapConfig,
        toolchain: Toolchain,
    ) -> Result<Self, Stage2Error> {
        let work_dir = work_dir.as_ref().join("stage2");

        // Create directory structure
        let sysroot = work_dir.join("sysroot");
        let sources_dir = work_dir.parent().unwrap_or(&work_dir).join("sources");
        let logs_dir = work_dir.join("logs");

        fs::create_dir_all(&sysroot)?;
        fs::create_dir_all(&sources_dir)?;
        fs::create_dir_all(&logs_dir)?;

        build_helpers::create_sysroot_dirs(&sysroot)?;

        let build_env = build_helpers::setup_build_env(&toolchain, &sysroot);
        let runner = PackageBuildRunner::new(&sources_dir, config);

        Ok(Self {
            work_dir,
            config: config.clone(),
            toolchain,
            sysroot,
            logs_dir,
            packages: Vec::new(),
            build_env,
            runner,
        })
    }

    /// The packages rebuilt in Stage 2.
    pub fn package_names() -> &'static [&'static str] {
        STAGE2_PACKAGES
    }

    /// The input toolchain (should be Stage 1).
    pub fn toolchain(&self) -> &Toolchain {
        &self.toolchain
    }

    /// Get the sysroot path
    pub fn sysroot(&self) -> &Path {
        &self.sysroot
    }

    /// Get the output directory
    pub fn output_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Load recipes from the recipe directory.
    ///
    /// Expects recipes to be under `recipe_dir/stage1/` (same recipes as Stage 1).
    pub fn load_recipes(&mut self, recipe_dir: &Path) -> Result<(), Stage2Error> {
        let stage1_dir = recipe_dir.join("stage1");
        self.packages.clear();

        for name in STAGE2_PACKAGES {
            let path = stage1_dir.join(format!("{name}.toml"));
            if !path.exists() {
                return Err(Stage2Error::RecipeNotFound(path.display().to_string()));
            }
            let recipe = parse_recipe_file(&path)
                .map_err(|e| Stage2Error::RecipeParseFailed(format!("{name}: {e}")))?;
            self.packages.push(Stage2Package {
                name: name.to_string(),
                recipe,
                status: Stage2PackageStatus::Pending,
                log: String::new(),
            });
        }

        info!("Loaded {} Stage 2 recipes", self.packages.len());
        Ok(())
    }

    /// Build all Stage 2 packages.
    ///
    /// Returns a toolchain pointing at the Stage 2 sysroot.
    pub fn build(&mut self) -> Result<Toolchain, Stage2Error> {
        info!(
            "Stage 2: Rebuilding {} packages with Stage 1 toolchain",
            self.packages.len()
        );
        info!("Sysroot: {}", self.sysroot.display());
        info!("Using Stage 1: {}", self.toolchain.path.display());

        let package_count = self.packages.len();
        for idx in 0..package_count {
            let pkg_name = self.packages[idx].name.clone();
            info!(
                "[{}/{}] Building {pkg_name} (Stage 2)",
                idx + 1,
                package_count
            );

            self.packages[idx].status = Stage2PackageStatus::Building;

            if let Err(e) = self.build_package(idx) {
                self.packages[idx].status = Stage2PackageStatus::Failed(e.to_string());
                self.save_log(idx)?;
                return Err(e);
            }

            self.packages[idx].status = Stage2PackageStatus::Complete;
            self.save_log(idx)?;
        }

        info!("Stage 2 complete: output at {}", self.work_dir.display());

        // Create and return the Stage 2 toolchain
        let stage2_prefix = self.sysroot.join("usr");
        let mut toolchain =
            Toolchain::from_prefix(&stage2_prefix).map_err(|e| Stage2Error::BuildFailed {
                package: "toolchain".to_string(),
                reason: e.to_string(),
            })?;
        toolchain.kind = ToolchainKind::Stage2;

        Ok(toolchain)
    }

    /// Validate that the Stage 1 toolchain is usable.
    pub fn validate_toolchain(&self) -> Result<(), Stage2Error> {
        if !self.toolchain.bin_dir().exists() {
            return Err(Stage2Error::NoStage1Toolchain);
        }
        Ok(())
    }

    /// Compare Stage 1 and Stage 2 output for reproducibility.
    ///
    /// Returns a list of mismatched packages (empty = fully reproducible).
    pub fn verify_reproducibility(&self, stage1_dir: &Path) -> Vec<String> {
        let mut mismatches = Vec::new();

        for name in STAGE2_PACKAGES {
            let s1 = stage1_dir.join(name);
            let s2 = self.work_dir.join(name);

            if s1.exists()
                && s2.exists()
                && let Err(e) = compare_dirs(&s1, &s2)
            {
                warn!("Reproducibility mismatch for {name}: {e}");
                mismatches.push(name.to_string());
            }
        }

        mismatches
    }

    /// Get build status summary
    pub fn status_summary(&self) -> Vec<(&str, &Stage2PackageStatus)> {
        self.packages
            .iter()
            .map(|p| (p.name.as_str(), &p.status))
            .collect()
    }

    /// Build a single package by index
    fn build_package(&mut self, idx: usize) -> Result<(), Stage2Error> {
        let pkg_name = self.packages[idx].name.clone();

        // Create build directory
        let (src_dir, build_dir) = self
            .runner
            .prepare_build_dirs(&self.work_dir, &pkg_name)
            .map_err(Stage2Error::Runner)?;

        // Fetch source (reuses cached sources from Stage 1)
        let archive_path = self
            .runner
            .fetch_source(&pkg_name, &self.packages[idx].recipe)
            .map_err(|e| Stage2Error::BuildFailed {
                package: pkg_name.clone(),
                reason: e.to_string(),
            })?;

        // Extract source
        self.runner
            .extract_source(&archive_path, &src_dir)
            .map_err(Stage2Error::Runner)?;

        // Find the actual source directory
        let actual_src_dir = self
            .runner
            .find_source_dir(&src_dir)
            .map_err(Stage2Error::Runner)?;

        // Fetch and extract additional sources
        self.runner
            .fetch_additional_sources(&pkg_name, &self.packages[idx].recipe, &actual_src_dir)
            .map_err(|e| Stage2Error::BuildFailed {
                package: pkg_name.clone(),
                reason: e.to_string(),
            })?;

        // Determine working directory for build phases
        let has_workdir = self.packages[idx].recipe.build.workdir.is_some();
        let work_dir = if has_workdir {
            build_dir.clone()
        } else {
            actual_src_dir.clone()
        };

        // Run build phases
        self.run_configure(idx, &actual_src_dir, &build_dir)?;
        self.run_make(idx, &work_dir)?;
        self.run_install(idx, &work_dir)?;

        info!("  Package {pkg_name} built successfully (Stage 2)");
        Ok(())
    }

    /// Run a recipe build phase (configure, make, or install)
    fn run_recipe_phase(
        &mut self,
        idx: usize,
        workdir: &Path,
        phase_name: &str,
        label: &str,
        get_cmd: fn(&Recipe) -> Option<&String>,
    ) -> Result<(), Stage2Error> {
        let raw_cmd = match get_cmd(&self.packages[idx].recipe) {
            Some(cmd) if !cmd.is_empty() => cmd.clone(),
            _ => {
                info!("  No {} step", phase_name);
                return Ok(());
            }
        };

        let destdir = self.sysroot.to_string_lossy().to_string();
        let mut cmd = self.packages[idx].recipe.substitute(&raw_cmd, &destdir);
        cmd = self.substitute_vars(&cmd, idx);

        info!("  {}...", label);
        self.packages[idx]
            .log
            .push_str(&format!("=== {} ===\n{}\n", label, cmd));

        self.run_shell_command(idx, &cmd, workdir, phase_name)
    }

    /// Run the configure phase
    fn run_configure(
        &mut self,
        idx: usize,
        src_dir: &Path,
        build_dir: &Path,
    ) -> Result<(), Stage2Error> {
        let has_workdir = self.packages[idx].recipe.build.workdir.is_some();
        let workdir = if has_workdir { build_dir } else { src_dir };
        self.run_recipe_phase(idx, workdir, "configure", "Configuring", |r| {
            r.build.configure.as_ref()
        })
    }

    /// Run the make phase
    fn run_make(&mut self, idx: usize, build_dir: &Path) -> Result<(), Stage2Error> {
        self.run_recipe_phase(idx, build_dir, "make", "Building", |r| {
            r.build.make.as_ref()
        })
    }

    /// Run the install phase
    fn run_install(&mut self, idx: usize, build_dir: &Path) -> Result<(), Stage2Error> {
        self.run_recipe_phase(idx, build_dir, "install", "Installing", |r| {
            r.build.install.as_ref()
        })
    }

    /// Substitute build variables in a command string
    fn substitute_vars(&self, cmd: &str, idx: usize) -> String {
        build_helpers::substitute_build_vars(
            cmd,
            self.config.triple(),
            self.config.jobs,
            &self.sysroot,
            &self.packages[idx].recipe,
        )
    }

    /// Run a shell command in the build directory
    fn run_shell_command(
        &mut self,
        idx: usize,
        cmd: &str,
        workdir: &Path,
        phase: &str,
    ) -> Result<(), Stage2Error> {
        let pkg_name = self.packages[idx].name.clone();
        let cross_env = self.packages[idx].recipe.cross_env();
        let sysroot_str = self.sysroot.to_string_lossy().to_string();
        let recipe_env: HashMap<String, String> = self.packages[idx]
            .recipe
            .build
            .environment
            .iter()
            .map(|(k, v)| {
                let value = self.packages[idx].recipe.substitute(v, &sysroot_str);
                (k.clone(), value)
            })
            .collect();
        let verbose = self.config.verbose;

        let env_vec =
            build_helpers::merge_build_env(&self.build_env, cross_env, recipe_env, &self.build_env);

        let stage1_root = self.toolchain.path.parent().unwrap_or(&self.toolchain.path);

        let (code, stdout, stderr) = build_helpers::run_sandboxed_command(
            cmd,
            workdir,
            &self.sysroot,
            &self.work_dir.join("sources"),
            &self.work_dir.join("build"),
            stage1_root,
            &env_vec,
        )
        .map_err(|e| Stage2Error::BuildFailed {
            package: pkg_name.clone(),
            reason: e,
        })?;

        // Log output
        if !stdout.is_empty() {
            self.packages[idx]
                .log
                .push_str(&format!("stdout:\n{}\n", stdout));
            if verbose {
                println!("{}", stdout);
            }
        }
        if !stderr.is_empty() {
            self.packages[idx]
                .log
                .push_str(&format!("stderr:\n{}\n", stderr));
            if verbose {
                eprintln!("{}", stderr);
            }
        }

        if code != 0 {
            return Err(Stage2Error::BuildFailed {
                package: pkg_name,
                reason: format!(
                    "{} phase failed with exit code {}:\n{}",
                    phase, code, stderr
                ),
            });
        }

        Ok(())
    }

    /// Save a package's build log to disk
    fn save_log(&self, idx: usize) -> Result<(), Stage2Error> {
        let pkg = &self.packages[idx];
        let log_path = self.logs_dir.join(format!("{}.log", pkg.name));
        fs::write(&log_path, &pkg.log)?;
        Ok(())
    }
}

/// Compute SHA-256 hash of a file's contents
fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    let content = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Ok(hex::encode(hasher.finalize()))
}

/// Best-effort directory comparison for reproducibility checking.
///
/// Walks both directory trees and compares file sizes. Full content
/// hashing is a follow-up (timestamps and build paths make bit-exact
/// comparison difficult without canonicalisation).
fn compare_dirs(a: &Path, b: &Path) -> Result<(), String> {
    if !a.exists() || !b.exists() {
        return Err(format!(
            "Missing directory: {} or {}",
            a.display(),
            b.display()
        ));
    }

    // Collect sorted file lists from both directories
    let a_files =
        collect_file_list(a).map_err(|e| format!("Error reading {}: {}", a.display(), e))?;
    let b_files =
        collect_file_list(b).map_err(|e| format!("Error reading {}: {}", b.display(), e))?;

    // Compare file counts
    if a_files.len() != b_files.len() {
        return Err(format!(
            "File count mismatch: {} has {} files, {} has {} files",
            a.display(),
            a_files.len(),
            b.display(),
            b_files.len()
        ));
    }

    // Compare relative paths
    let a_relative: Vec<_> = a_files
        .iter()
        .filter_map(|p| p.strip_prefix(a).ok().map(|r| r.to_path_buf()))
        .collect();
    let b_relative: Vec<_> = b_files
        .iter()
        .filter_map(|p| p.strip_prefix(b).ok().map(|r| r.to_path_buf()))
        .collect();

    if a_relative != b_relative {
        return Err("File tree structure differs".to_string());
    }

    // Compare file contents by SHA-256 hash
    for (a_path, rel_path) in a_files.iter().zip(a_relative.iter()) {
        let b_path = b.join(rel_path);
        let a_hash = hash_file(a_path)
            .map_err(|e| format!("Failed to hash {}: {}", a_path.display(), e))?;
        let b_hash = hash_file(&b_path)
            .map_err(|e| format!("Failed to hash {}: {}", b_path.display(), e))?;
        if a_hash != b_hash {
            return Err(format!(
                "Content mismatch for {}: {} vs {}",
                rel_path.display(),
                a_hash,
                b_hash
            ));
        }
    }

    Ok(())
}

/// Recursively collect all files in a directory, sorted
fn collect_file_list(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_file_list(&path)?);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage2_package_names() {
        let names = Stage2Builder::package_names();
        assert_eq!(names.len(), 5);
        assert_eq!(names[0], "linux-headers");
        assert_eq!(names[4], "gcc-pass2");
    }

    #[test]
    fn test_stage2_error_display() {
        let err = Stage2Error::NoStage1Toolchain;
        assert!(err.to_string().contains("Stage 1"));

        let err = Stage2Error::RecipeNotFound("test.toml".to_string());
        assert!(err.to_string().contains("test.toml"));

        let err = Stage2Error::BuildFailed {
            package: "gcc".to_string(),
            reason: "compile error".to_string(),
        };
        assert!(err.to_string().contains("gcc"));
        assert!(err.to_string().contains("compile error"));

        let err = Stage2Error::ReproducibilityMismatch {
            package: "binutils".to_string(),
            stage1_hash: "aaa".to_string(),
            stage2_hash: "bbb".to_string(),
        };
        assert!(err.to_string().contains("binutils"));
        assert!(err.to_string().contains("aaa"));
        assert!(err.to_string().contains("bbb"));
    }

    #[test]
    fn test_stage2_validate_missing_toolchain() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::Stage1,
            path: dir.path().join("nonexistent"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };
        let builder = Stage2Builder::new(dir.path(), &config, tc).unwrap();
        assert!(builder.validate_toolchain().is_err());
    }

    #[test]
    fn test_stage2_output_dir() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::Stage1,
            path: dir.path().join("stage1"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };
        let builder = Stage2Builder::new(dir.path(), &config, tc).unwrap();
        assert!(builder.output_dir().ends_with("stage2"));
    }

    #[test]
    fn test_stage2_uses_stage1_toolchain() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::Stage1,
            path: dir.path().join("stage1"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };
        let builder = Stage2Builder::new(dir.path(), &config, tc).unwrap();
        assert_eq!(builder.toolchain().kind, ToolchainKind::Stage1);
    }

    #[test]
    fn test_reproducibility_empty_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::Stage1,
            path: dir.path().join("stage1"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };
        let builder = Stage2Builder::new(dir.path(), &config, tc).unwrap();
        // No output dirs exist, so no mismatches reported
        let mismatches = builder.verify_reproducibility(dir.path());
        assert!(mismatches.is_empty());
    }

    #[test]
    fn test_reproducibility_matching_dirs() {
        let dir = tempfile::tempdir().unwrap();

        // Create matching directory structures
        let s1 = dir.path().join("s1").join("binutils");
        let s2 = dir.path().join("s2").join("binutils");
        fs::create_dir_all(s1.join("bin")).unwrap();
        fs::create_dir_all(s2.join("bin")).unwrap();
        fs::write(s1.join("bin/ld"), "contents").unwrap();
        fs::write(s2.join("bin/ld"), "contents").unwrap();

        let result = compare_dirs(&s1, &s2);
        assert!(result.is_ok());
    }

    #[test]
    fn test_reproducibility_mismatching_dirs() {
        let dir = tempfile::tempdir().unwrap();

        // Create mismatching directory structures
        let s1 = dir.path().join("s1");
        let s2 = dir.path().join("s2");
        fs::create_dir_all(&s1).unwrap();
        fs::create_dir_all(&s2).unwrap();
        fs::write(s1.join("file_a"), "a").unwrap();
        fs::write(s2.join("file_b"), "b").unwrap();

        let result = compare_dirs(&s1, &s2);
        assert!(result.is_err());
    }

    #[test]
    fn test_reproducibility_content_mismatch() {
        let dir = tempfile::tempdir().unwrap();

        // Create directories with same structure but different content
        let s1 = dir.path().join("s1");
        let s2 = dir.path().join("s2");
        fs::create_dir_all(&s1).unwrap();
        fs::create_dir_all(&s2).unwrap();
        fs::write(s1.join("file"), "content_a").unwrap();
        fs::write(s2.join("file"), "content_b").unwrap();

        let result = compare_dirs(&s1, &s2);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Content mismatch"));
    }

    #[test]
    fn test_stage2_package_status() {
        let status = Stage2PackageStatus::Pending;
        assert_eq!(status, Stage2PackageStatus::Pending);

        let status = Stage2PackageStatus::Failed("error".to_string());
        if let Stage2PackageStatus::Failed(msg) = status {
            assert_eq!(msg, "error");
        }
    }

    #[test]
    fn test_stage2_load_recipes() {
        let dir = tempfile::tempdir().unwrap();
        let config = BootstrapConfig::new();
        let tc = Toolchain {
            kind: ToolchainKind::Stage1,
            path: dir.path().join("stage1"),
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };
        let mut builder = Stage2Builder::new(dir.path(), &config, tc).unwrap();

        let recipe_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("recipes");
        if !recipe_dir.join("stage1").exists() {
            eprintln!("Skipping: recipes/stage1 not found");
            return;
        }
        assert!(builder.load_recipes(&recipe_dir).is_ok());
        assert_eq!(builder.packages.len(), 5);
    }
}
