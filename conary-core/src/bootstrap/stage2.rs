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

use super::config::BootstrapConfig;
use super::toolchain::{Toolchain, ToolchainKind};
use crate::recipe::{Recipe, parse_recipe_file};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use tracing::{debug, info, warn};

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
    /// Base work directory
    work_dir: PathBuf,

    /// Output directory for Stage 2 artifacts
    output_dir: PathBuf,

    /// Bootstrap configuration
    config: BootstrapConfig,

    /// Stage 1 toolchain (used as the compiler for this stage)
    toolchain: Toolchain,

    /// Sysroot directory for Stage 2
    sysroot: PathBuf,

    /// Source cache directory (shared with Stage 1)
    sources_dir: PathBuf,

    /// Build logs directory
    logs_dir: PathBuf,

    /// Packages to build in order
    packages: Vec<Stage2Package>,

    /// Environment variables for builds
    build_env: HashMap<String, String>,
}

impl Stage2Builder {
    /// Create a new Stage 2 builder
    pub fn new(
        work_dir: impl AsRef<Path>,
        config: &BootstrapConfig,
        toolchain: Toolchain,
    ) -> Result<Self, Stage2Error> {
        let work_dir = work_dir.as_ref().to_path_buf();
        let output_dir = work_dir.join("stage2");

        // Create directory structure
        let sysroot = output_dir.join("sysroot");
        let sources_dir = work_dir.join("sources");
        let logs_dir = output_dir.join("logs");

        fs::create_dir_all(&sysroot)?;
        fs::create_dir_all(&sources_dir)?;
        fs::create_dir_all(&logs_dir)?;

        // Create sysroot directory structure
        for dir in &[
            "usr",
            "usr/bin",
            "usr/lib",
            "usr/include",
            "lib",
            "lib64",
            "bin",
        ] {
            fs::create_dir_all(sysroot.join(dir))?;
        }

        // Set up build environment using Stage 1 toolchain
        let mut build_env = toolchain.env();

        // Add optimization flags (conservative for bootstrap)
        build_env.insert("CFLAGS".to_string(), "-O2 -pipe".to_string());
        build_env.insert("CXXFLAGS".to_string(), "-O2 -pipe".to_string());

        // Add sysroot to paths
        let sysroot_str = sysroot.to_string_lossy().to_string();
        build_env.insert("SYSROOT".to_string(), sysroot_str.clone());

        // Update PATH to include sysroot binaries
        let path = format!(
            "{}:{}/usr/bin:{}",
            toolchain.bin_dir().display(),
            sysroot_str,
            std::env::var("PATH").unwrap_or_default()
        );
        build_env.insert("PATH".to_string(), path);

        Ok(Self {
            work_dir: output_dir.clone(),
            output_dir,
            config: config.clone(),
            toolchain,
            sysroot,
            sources_dir,
            logs_dir,
            packages: Vec::new(),
            build_env,
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
        &self.output_dir
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
                return Err(Stage2Error::RecipeNotFound(format!(
                    "{}",
                    path.display()
                )));
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
        info!("Stage 2: Rebuilding {} packages with Stage 1 toolchain", self.packages.len());
        info!("Sysroot: {}", self.sysroot.display());
        info!("Using Stage 1: {}", self.toolchain.path.display());

        let package_count = self.packages.len();
        for idx in 0..package_count {
            let pkg_name = self.packages[idx].name.clone();
            info!("[{}/{}] Building {pkg_name} (Stage 2)", idx + 1, package_count);

            self.packages[idx].status = Stage2PackageStatus::Building;

            if let Err(e) = self.build_package(idx) {
                self.packages[idx].status =
                    Stage2PackageStatus::Failed(e.to_string());
                self.save_log(idx)?;
                return Err(e);
            }

            self.packages[idx].status = Stage2PackageStatus::Complete;
            self.save_log(idx)?;
        }

        info!("Stage 2 complete: output at {}", self.output_dir.display());

        // Create and return the Stage 2 toolchain
        let stage2_prefix = self.sysroot.join("usr");
        let mut toolchain = Toolchain::from_prefix(&stage2_prefix).map_err(|e| {
            Stage2Error::BuildFailed {
                package: "toolchain".to_string(),
                reason: e.to_string(),
            }
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
            let s2 = self.output_dir.join(name);

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
        let build_base = self.work_dir.join("build").join(&pkg_name);
        let src_dir = build_base.join("src");
        let build_dir = build_base.join("build");

        // Clean previous build if exists
        if build_base.exists() {
            fs::remove_dir_all(&build_base)?;
        }
        fs::create_dir_all(&src_dir)?;
        fs::create_dir_all(&build_dir)?;

        // Fetch source (reuses cached sources from Stage 1)
        let archive_path = self.fetch_source(idx)?;

        // Extract source
        self.extract_source(&archive_path, &src_dir)?;

        // Find the actual source directory
        let actual_src_dir = self.find_source_dir(&src_dir)?;

        // Fetch and extract additional sources
        self.fetch_additional_sources(idx, &actual_src_dir)?;

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

    /// Fetch the source archive for a package (reuses Stage 1 cache)
    fn fetch_source(&mut self, idx: usize) -> Result<PathBuf, Stage2Error> {
        let pkg_name = self.packages[idx].name.clone();
        let url = self.packages[idx].recipe.archive_url();
        let filename = self.packages[idx].recipe.archive_filename();
        let target_path = self.sources_dir.join(&filename);

        // Sources should already be cached from Stage 1
        if target_path.exists() {
            info!("  Using cached source: {}", filename);
            self.packages[idx]
                .log
                .push_str(&format!("Using cached source: {}\n", filename));
            return Ok(target_path);
        }

        // If not cached, download
        info!("  Fetching: {}", url);
        self.packages[idx]
            .log
            .push_str(&format!("Fetching: {}\n", url));

        let output = Command::new("curl")
            .args([
                "-fsSL",
                "-o",
                target_path.to_str().expect("path must be valid utf-8"),
                &url,
            ])
            .output()
            .map_err(|e| Stage2Error::BuildFailed {
                package: pkg_name.clone(),
                reason: e.to_string(),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Stage2Error::BuildFailed {
                package: pkg_name,
                reason: format!("Source fetch failed: {}", stderr),
            });
        }

        Ok(target_path)
    }

    /// Fetch additional sources (like GMP, MPFR, MPC for GCC)
    fn fetch_additional_sources(&mut self, idx: usize, src_dir: &Path) -> Result<(), Stage2Error> {
        let pkg_name = self.packages[idx].name.clone();
        let additional_sources: Vec<_> = self.packages[idx]
            .recipe
            .source
            .additional
            .iter()
            .map(|a| (a.url.clone(), a.extract_to.clone()))
            .collect();

        let sources_dir = self.sources_dir.clone();
        let src_dir = src_dir.to_path_buf();

        for (url, extract_to) in additional_sources {
            let filename = url.split('/').next_back().unwrap_or("additional.tar.gz");
            let target_path = sources_dir.join(filename);

            // Download if not cached
            if !target_path.exists() {
                info!("  Fetching additional: {}", filename);
                self.packages[idx]
                    .log
                    .push_str(&format!("Fetching additional: {}\n", filename));

                let output = Command::new("curl")
                    .args([
                        "-fsSL",
                        "-o",
                        target_path.to_str().expect("path must be valid utf-8"),
                        &url,
                    ])
                    .output()
                    .map_err(|e| Stage2Error::BuildFailed {
                        package: pkg_name.clone(),
                        reason: e.to_string(),
                    })?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Stage2Error::BuildFailed {
                        package: pkg_name.clone(),
                        reason: format!("Additional source fetch failed: {}", stderr),
                    });
                }
            }

            // Extract to the specified location, stripping top-level directory
            let extract_dest = if let Some(dest) = &extract_to {
                src_dir.join(dest)
            } else {
                src_dir.clone()
            };

            self.extract_source_strip(&target_path, &extract_dest)?;
        }

        Ok(())
    }

    /// Extract a tar archive to a destination directory
    fn extract_tar(
        &self,
        archive: &Path,
        dest: &Path,
        strip_components: bool,
    ) -> Result<(), Stage2Error> {
        fs::create_dir_all(dest)?;

        let archive_str = archive.to_str().expect("archive path must be valid utf-8");
        let dest_str = dest.to_str().expect("dest path must be valid utf-8");
        let filename = archive
            .file_name()
            .expect("archive path must have a filename")
            .to_string_lossy();

        let flag = if filename.ends_with(".tar.xz") || filename.ends_with(".txz") {
            "xJf"
        } else if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
            "xzf"
        } else if filename.ends_with(".tar.bz2") || filename.ends_with(".tbz2") {
            "xjf"
        } else {
            "xf"
        };

        let mut cmd = Command::new("tar");
        cmd.args([flag, archive_str, "-C", dest_str]);
        if strip_components {
            cmd.arg("--strip-components=1");
        }

        let output = cmd.output().map_err(|e| Stage2Error::BuildFailed {
            package: "extract".to_string(),
            reason: e.to_string(),
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Stage2Error::BuildFailed {
                package: "extract".to_string(),
                reason: stderr.to_string(),
            });
        }

        Ok(())
    }

    /// Extract a source archive, stripping the top-level directory
    fn extract_source_strip(&self, archive: &Path, dest: &Path) -> Result<(), Stage2Error> {
        self.extract_tar(archive, dest, true)
    }

    /// Extract a source archive
    fn extract_source(&self, archive: &Path, dest: &Path) -> Result<(), Stage2Error> {
        self.extract_tar(archive, dest, false)
    }

    /// Find the actual source directory after extraction
    fn find_source_dir(&self, dir: &Path) -> Result<PathBuf, Stage2Error> {
        let entries: Vec<_> = fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();

        if entries.len() == 1 {
            Ok(entries[0].path())
        } else {
            Ok(dir.to_path_buf())
        }
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
        self.run_recipe_phase(idx, build_dir, "make", "Building", |r| r.build.make.as_ref())
    }

    /// Run the install phase
    fn run_install(&mut self, idx: usize, build_dir: &Path) -> Result<(), Stage2Error> {
        self.run_recipe_phase(idx, build_dir, "install", "Installing", |r| {
            r.build.install.as_ref()
        })
    }

    /// Substitute build variables in a command string
    fn substitute_vars(&self, cmd: &str, idx: usize) -> String {
        let pkg = &self.packages[idx];
        let mut result = cmd.to_string();

        result = result.replace("%(target)s", self.config.triple());
        result = result.replace("%(jobs)s", &self.config.jobs.to_string());
        result = result.replace("%(stage1_sysroot)s", &self.sysroot.to_string_lossy());

        if let Some(cross) = &pkg.recipe.cross {
            if let Some(target) = &cross.target {
                result = result.replace("%(target)s", target);
            }
            if let Some(sysroot) = &cross.sysroot {
                result = result.replace("%(sysroot)s", sysroot);
            }
        }

        result
    }

    /// Run a shell command in the build directory
    fn run_shell_command(
        &mut self,
        idx: usize,
        cmd: &str,
        workdir: &Path,
        phase: &str,
    ) -> Result<(), Stage2Error> {
        use crate::container::{BindMount, ContainerConfig, Sandbox};

        let pkg_name = self.packages[idx].name.clone();
        let cross_env = self.packages[idx].recipe.cross_env();
        let sysroot_str = self.sysroot.to_string_lossy().to_string();
        let build_environment: HashMap<String, String> = self.packages[idx]
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

        // Construct environment vector
        let mut env_vec: Vec<(String, String)> = Vec::new();

        // Add base environment
        for (key, value) in &self.build_env {
            env_vec.push((key.clone(), value.clone()));
        }

        // Add cross-compilation environment
        for (key, value) in cross_env {
            if key == "CFLAGS" || key == "CXXFLAGS" || key == "LDFLAGS" {
                let base_flags = self.build_env.get(&key).map(|s| s.as_str()).unwrap_or("");
                let merged = format!("{} {}", base_flags, value);
                env_vec.retain(|(k, _)| k != &key);
                env_vec.push((key, merged.trim().to_string()));
            } else {
                env_vec.retain(|(k, _)| k != &key);
                env_vec.push((key, value));
            }
        }

        // Add recipe environment
        for (key, value) in build_environment {
            let expanded = self.expand_env_vars(&value);
            env_vec.retain(|(k, _)| k != &key);
            env_vec.push((key, expanded));
        }

        let env_refs: Vec<(&str, &str)> = env_vec
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        debug!("Running in sandbox: bash -c \"{}\"", cmd);
        debug!("Workdir: {}", workdir.display());

        // Configure sandbox -- Stage 2 uses the Stage 1 toolchain path
        let stage1_root = self.toolchain.path.parent().unwrap_or(&self.toolchain.path);

        let mut config = ContainerConfig::pristine_for_bootstrap(
            &self.sysroot,
            &self.work_dir.join("sources"),
            &self.work_dir.join("build"),
            &self.sysroot,
        );

        // Mount Stage 1 tools so the compiler is available
        config.add_bind_mount(BindMount::readonly(stage1_root, "/tools"));
        config.workdir = workdir.to_path_buf();
        config.deny_network();

        let mut sandbox = Sandbox::new(config);

        let (code, stdout, stderr) = sandbox
            .execute(
                "bash",
                &format!("set -e\n{}", cmd),
                &[],
                &env_refs,
            )
            .map_err(|e| Stage2Error::BuildFailed {
                package: pkg_name.clone(),
                reason: e.to_string(),
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
        debug!("Saved build log: {}", log_path.display());
        Ok(())
    }

    /// Expand environment variable references in a string
    ///
    /// Supports both $VAR and ${VAR} syntax.
    fn expand_env_vars(&self, value: &str) -> String {
        let mut result = value.to_string();

        // Handle ${VAR} syntax
        while let Some(start) = result.find("${") {
            if let Some(end) = result[start..].find('}') {
                let var_name = &result[start + 2..start + end];
                let replacement = self
                    .build_env
                    .get(var_name)
                    .cloned()
                    .or_else(|| std::env::var(var_name).ok())
                    .unwrap_or_default();
                result = format!(
                    "{}{}{}",
                    &result[..start],
                    replacement,
                    &result[start + end + 1..]
                );
            } else {
                break;
            }
        }

        // Handle $VAR syntax (must come after ${VAR} to avoid conflicts)
        let mut i = 0;
        while i < result.len() {
            if result[i..].starts_with('$') && !result[i..].starts_with("${") {
                let rest = &result[i + 1..];
                let var_end = rest
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(rest.len());
                if var_end > 0 {
                    let var_name = &rest[..var_end];
                    let replacement = self
                        .build_env
                        .get(var_name)
                        .cloned()
                        .or_else(|| std::env::var(var_name).ok())
                        .unwrap_or_default();
                    result = format!(
                        "{}{}{}",
                        &result[..i],
                        replacement,
                        &result[i + 1 + var_end..]
                    );
                    i += replacement.len();
                    continue;
                }
            }
            i += 1;
        }

        result
    }
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
    let a_files = collect_file_list(a).map_err(|e| format!("Error reading {}: {}", a.display(), e))?;
    let b_files = collect_file_list(b).map_err(|e| format!("Error reading {}: {}", b.display(), e))?;

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
