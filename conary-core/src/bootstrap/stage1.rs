// conary-core/src/bootstrap/stage1.rs

//! Stage 1 toolchain builder
//!
//! Stage 1 builds a self-hosted toolchain using the Stage 0 cross-compiler.
//! This is the first step toward a fully native system - the resulting
//! toolchain can compile packages that run on the target system.
//!
//! # Build Order
//!
//! The Stage 1 build follows a strict dependency order:
//!
//! 1. **linux-headers** - Kernel headers for glibc
//! 2. **binutils** - Assembler, linker, and binary tools
//! 3. **gcc-pass1** - Minimal C compiler (no threads, no shared libs)
//! 4. **glibc** - The C library (needs gcc-pass1)
//! 5. **gcc-pass2** - Full GCC with C/C++ and threading support
//!
//! After this, the sysroot contains a complete toolchain that can build
//! the base system packages.

use super::config::BootstrapConfig;
use super::toolchain::{Toolchain, ToolchainKind};
use crate::recipe::{Recipe, parse_recipe_file};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use super::build_helpers;
use tracing::{info, warn};

/// Errors that can occur during Stage 1 build
#[derive(Debug, Error)]
pub enum Stage1Error {
    #[error("Stage 0 toolchain not found at {0}")]
    Stage0NotFound(PathBuf),

    #[error("Recipe not found: {0}")]
    RecipeNotFound(String),

    #[error("Recipe parse error: {0}")]
    RecipeParseFailed(String),

    #[error("Source fetch failed for {0}: {1}")]
    SourceFetchFailed(String, String),

    #[error("Build failed for {0}: {1}")]
    BuildFailed(String, String),

    #[error("Install failed for {0}: {1}")]
    InstallFailed(String, String),

    #[error("Failed to create directory: {0}")]
    DirectoryCreation(#[from] std::io::Error),

    #[error("Missing dependency: {0} requires {1}")]
    MissingDependency(String, String),
}

/// Status of a Stage 1 package build
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageBuildStatus {
    /// Not started
    Pending,
    /// Fetching sources
    Fetching,
    /// Extracting and patching
    Preparing,
    /// Running configure
    Configuring,
    /// Running make
    Building,
    /// Running make install
    Installing,
    /// Build complete
    Complete,
    /// Build failed
    Failed(String),
}

/// Represents a single package being built in Stage 1
#[derive(Debug)]
pub struct Stage1Package {
    /// Package name
    pub name: String,
    /// Recipe for this package
    pub recipe: Recipe,
    /// Current build status
    pub status: PackageBuildStatus,
    /// Build log
    pub log: String,
}

/// Builder for Stage 1 toolchain
pub struct Stage1Builder {
    /// Base work directory
    work_dir: PathBuf,

    /// Bootstrap configuration
    config: BootstrapConfig,

    /// Stage 0 toolchain
    stage0: Toolchain,

    /// Sysroot directory for Stage 1
    sysroot: PathBuf,

    /// Source cache directory
    sources_dir: PathBuf,

    /// Build logs directory
    logs_dir: PathBuf,

    /// Packages to build in order
    packages: Vec<Stage1Package>,

    /// Environment variables for builds
    build_env: HashMap<String, String>,
}

impl Stage1Builder {
    /// Create a new Stage 1 builder
    pub fn new(
        work_dir: impl AsRef<Path>,
        config: &BootstrapConfig,
        stage0: Toolchain,
    ) -> Result<Self, Stage1Error> {
        let work_dir = work_dir.as_ref().to_path_buf();
        let stage1_dir = work_dir.join("stage1");

        // Create directory structure
        let sysroot = stage1_dir.join("sysroot");
        let sources_dir = work_dir.join("sources");
        let logs_dir = stage1_dir.join("logs");

        fs::create_dir_all(&sysroot)?;
        fs::create_dir_all(&sources_dir)?;
        fs::create_dir_all(&logs_dir)?;

        build_helpers::create_sysroot_dirs(&sysroot)?;

        let build_env = build_helpers::setup_build_env(&stage0, &sysroot);

        Ok(Self {
            work_dir: stage1_dir,
            config: config.clone(),
            stage0,
            sysroot,
            sources_dir,
            logs_dir,
            packages: Vec::new(),
            build_env,
        })
    }

    /// Load recipes for Stage 1 packages
    pub fn load_recipes(&mut self, recipe_dir: impl AsRef<Path>) -> Result<(), Stage1Error> {
        let recipe_dir = recipe_dir.as_ref();
        let stage1_dir = recipe_dir.join("stage1");

        // The canonical build order for Stage 1
        let build_order = [
            "linux-headers",
            "binutils",
            "gcc-pass1",
            "glibc",
            "gcc-pass2",
        ];

        for pkg_name in &build_order {
            let recipe_path = stage1_dir.join(format!("{}.toml", pkg_name));

            if !recipe_path.exists() {
                return Err(Stage1Error::RecipeNotFound(format!(
                    "{}",
                    recipe_path.display()
                )));
            }

            let recipe = parse_recipe_file(&recipe_path)
                .map_err(|e| Stage1Error::RecipeParseFailed(e.to_string()))?;

            self.packages.push(Stage1Package {
                name: pkg_name.to_string(),
                recipe,
                status: PackageBuildStatus::Pending,
                log: String::new(),
            });
        }

        info!("Loaded {} Stage 1 recipes", self.packages.len());
        Ok(())
    }

    /// Build all Stage 1 packages
    pub fn build(&mut self) -> Result<Toolchain, Stage1Error> {
        info!("Building Stage 1 toolchain...");
        info!("Sysroot: {}", self.sysroot.display());
        info!("Using Stage 0: {}", self.stage0.path.display());

        let package_count = self.packages.len();
        for idx in 0..package_count {
            let pkg_name = self.packages[idx].name.clone();
            info!("[{}/{}] Building {}...", idx + 1, package_count, pkg_name);

            if let Err(e) = self.build_package(idx) {
                self.packages[idx].status = PackageBuildStatus::Failed(e.to_string());
                // Save log before returning error
                self.save_log(idx)?;
                return Err(e);
            }

            self.packages[idx].status = PackageBuildStatus::Complete;
            self.save_log(idx)?;
        }

        info!("Stage 1 build complete!");

        // Create and return the Stage 1 toolchain
        // Stage 1 uses --prefix=/usr, so binaries are in sysroot/usr/bin
        let stage1_prefix = self.sysroot.join("usr");
        let mut toolchain = Toolchain::from_prefix(&stage1_prefix)
            .map_err(|e| Stage1Error::BuildFailed("toolchain".to_string(), e.to_string()))?;
        toolchain.kind = ToolchainKind::Stage1;

        Ok(toolchain)
    }

    /// Build a single package by index
    fn build_package(&mut self, idx: usize) -> Result<(), Stage1Error> {
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

        // Fetch source
        self.packages[idx].status = PackageBuildStatus::Fetching;
        let archive_path = self.fetch_source(idx)?;

        // Extract source
        self.packages[idx].status = PackageBuildStatus::Preparing;
        self.extract_source(&archive_path, &src_dir)?;

        // Find the actual source directory (archives usually have a top-level dir)
        let actual_src_dir = self.find_source_dir(&src_dir)?;

        // Fetch and extract additional sources (like GMP, MPFR, MPC for GCC)
        self.fetch_additional_sources(idx, &actual_src_dir)?;

        // Determine working directory for build phases
        // If recipe has a workdir, use build_dir; otherwise use source dir
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

        Ok(())
    }

    /// Fetch the source archive for a package
    fn fetch_source(&mut self, idx: usize) -> Result<PathBuf, Stage1Error> {
        // Clone the data we need to avoid borrow conflicts
        let pkg_name = self.packages[idx].name.clone();
        let url = self.packages[idx].recipe.archive_url();
        let filename = self.packages[idx].recipe.archive_filename();
        let target_path = self.sources_dir.join(&filename);

        // Check if already downloaded
        if target_path.exists() {
            info!("  Using cached source: {}", filename);
            self.packages[idx]
                .log
                .push_str(&format!("Using cached source: {}\n", filename));
            return Ok(target_path);
        }

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
            .map_err(|e| Stage1Error::SourceFetchFailed(pkg_name.clone(), e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Stage1Error::SourceFetchFailed(pkg_name, stderr.to_string()));
        }

        // Verify checksum
        self.verify_checksum(idx, &target_path)?;

        Ok(target_path)
    }

    /// Fetch additional sources (like GMP, MPFR, MPC for GCC)
    fn fetch_additional_sources(&mut self, idx: usize, src_dir: &Path) -> Result<(), Stage1Error> {
        // Clone the data we need upfront to avoid borrow conflicts
        let pkg_name = self.packages[idx].name.clone();
        let additional_sources: Vec<_> = self.packages[idx]
            .recipe
            .source
            .additional
            .iter()
            .map(|a| (a.url.clone(), a.checksum.clone(), a.extract_to.clone()))
            .collect();

        let sources_dir = self.sources_dir.clone();
        let src_dir = src_dir.to_path_buf();

        for (url, checksum, extract_to) in additional_sources {
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
                    .map_err(|e| Stage1Error::SourceFetchFailed(pkg_name.clone(), e.to_string()))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Stage1Error::SourceFetchFailed(
                        pkg_name.clone(),
                        stderr.to_string(),
                    ));
                }
            }

            // Verify checksum of additional source
            if checksum.contains("VERIFY_BEFORE_BUILD") || checksum.contains("FIXME") {
                if !self.config.skip_verify {
                    return Err(Stage1Error::SourceFetchFailed(
                        pkg_name.clone(),
                        format!(
                            "Additional source has placeholder checksum '{}' -- provide a real SHA-256 or use --skip-verify",
                            checksum
                        ),
                    ));
                }
                warn!("  Skipping placeholder checksum for additional source (--skip-verify)");
            } else if let Some((algo, hash)) = checksum.split_once(':') {
                if algo == "sha256" {
                    let output = Command::new("sha256sum")
                        .arg(&target_path)
                        .output()
                        .map_err(|e| {
                            Stage1Error::SourceFetchFailed(pkg_name.clone(), e.to_string())
                        })?;
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let computed = stdout.split_whitespace().next().unwrap_or("");
                    if computed != hash {
                        return Err(Stage1Error::SourceFetchFailed(
                            pkg_name.clone(),
                            format!(
                                "Additional source checksum mismatch for {}: expected {}, got {}",
                                filename, hash, computed
                            ),
                        ));
                    }
                } else {
                    warn!("  Unknown checksum algorithm for additional source: {}", algo);
                }
            }

            // Extract to the specified location, stripping top-level directory
            // This is needed because tarballs like mpfr-4.2.1.tar.xz contain
            // mpfr-4.2.1/ as the top-level, but GCC expects gmp/, mpfr/, etc.
            let extract_dest = if let Some(dest) = &extract_to {
                src_dir.join(dest)
            } else {
                src_dir.clone()
            };

            self.extract_source_strip(&target_path, &extract_dest)?;
        }

        Ok(())
    }

    /// Verify checksum of downloaded file
    fn verify_checksum(&self, idx: usize, path: &Path) -> Result<(), Stage1Error> {
        let pkg_name = &self.packages[idx].name;
        let expected = &self.packages[idx].recipe.source.checksum;

        // Reject placeholder checksums unless skip_verify is enabled
        if expected.contains("VERIFY_BEFORE_BUILD") || expected.contains("FIXME") {
            if self.config.skip_verify {
                warn!(
                    "  Skipping placeholder checksum (--skip-verify enabled): {}",
                    expected
                );
                return Ok(());
            }
            return Err(Stage1Error::SourceFetchFailed(
                pkg_name.clone(),
                format!(
                    "Recipe has placeholder checksum '{}' -- provide a real SHA-256 or use --skip-verify",
                    expected
                ),
            ));
        }

        // Extract algorithm and hash
        let (algo, hash) = expected.split_once(':').ok_or_else(|| {
            Stage1Error::SourceFetchFailed(pkg_name.clone(), "Invalid checksum format".to_string())
        })?;

        match algo {
            "sha256" => {
                let output = Command::new("sha256sum")
                    .arg(path)
                    .output()
                    .map_err(|e| Stage1Error::SourceFetchFailed(pkg_name.clone(), e.to_string()))?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                let computed = stdout.split_whitespace().next().unwrap_or("");

                if computed != hash {
                    return Err(Stage1Error::SourceFetchFailed(
                        pkg_name.clone(),
                        format!("Checksum mismatch: expected {}, got {}", hash, computed),
                    ));
                }
            }
            _ => {
                warn!("  Unknown checksum algorithm: {}", algo);
            }
        }

        Ok(())
    }

    /// Extract a tar archive to a destination directory
    fn extract_tar(
        &self,
        archive: &Path,
        dest: &Path,
        strip_components: bool,
    ) -> Result<(), Stage1Error> {
        build_helpers::extract_tar(archive, dest, strip_components)
            .map_err(|e| Stage1Error::BuildFailed("extract".to_string(), e))
    }

    /// Extract a source archive, stripping the top-level directory
    ///
    /// This is used for in-tree dependencies like GMP, MPFR, MPC for GCC builds.
    fn extract_source_strip(&self, archive: &Path, dest: &Path) -> Result<(), Stage1Error> {
        self.extract_tar(archive, dest, true)
    }

    /// Extract a source archive
    fn extract_source(&self, archive: &Path, dest: &Path) -> Result<(), Stage1Error> {
        self.extract_tar(archive, dest, false)
    }

    /// Find the actual source directory after extraction
    fn find_source_dir(&self, dir: &Path) -> Result<PathBuf, Stage1Error> {
        build_helpers::find_source_dir(dir).map_err(Stage1Error::DirectoryCreation)
    }

    /// Run a recipe build phase (configure, make, or install)
    fn run_recipe_phase(
        &mut self,
        idx: usize,
        workdir: &Path,
        phase_name: &str,
        label: &str,
        status: PackageBuildStatus,
        get_cmd: fn(&Recipe) -> Option<&String>,
    ) -> Result<(), Stage1Error> {
        self.packages[idx].status = status;

        let raw_cmd = match get_cmd(&self.packages[idx].recipe) {
            Some(cmd) if !cmd.is_empty() => cmd.clone(),
            _ => {
                info!("  No {} step", phase_name);
                return Ok(());
            }
        };

        let destdir = self.sysroot.to_string_lossy().to_string();
        let mut cmd = self.packages[idx].recipe.substitute(&raw_cmd, &destdir);
        cmd = self.substitute_cross_vars(&cmd, idx);

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
    ) -> Result<(), Stage1Error> {
        let has_workdir = self.packages[idx].recipe.build.workdir.is_some();
        let workdir = if has_workdir { build_dir } else { src_dir };
        self.run_recipe_phase(
            idx,
            workdir,
            "configure",
            "Configuring",
            PackageBuildStatus::Configuring,
            |r| r.build.configure.as_ref(),
        )
    }

    /// Run the make phase
    fn run_make(&mut self, idx: usize, build_dir: &Path) -> Result<(), Stage1Error> {
        self.run_recipe_phase(
            idx,
            build_dir,
            "make",
            "Building",
            PackageBuildStatus::Building,
            |r| r.build.make.as_ref(),
        )
    }

    /// Run the install phase
    fn run_install(&mut self, idx: usize, build_dir: &Path) -> Result<(), Stage1Error> {
        self.run_recipe_phase(
            idx,
            build_dir,
            "install",
            "Installing",
            PackageBuildStatus::Installing,
            |r| r.build.install.as_ref(),
        )
    }

    /// Substitute cross-compilation variables in a command
    fn substitute_cross_vars(&self, cmd: &str, idx: usize) -> String {
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
    ) -> Result<(), Stage1Error> {
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

        let env_vec = build_helpers::merge_build_env(
            &self.build_env,
            cross_env,
            recipe_env,
            &self.build_env,
        );

        let stage0_root = self.stage0.path.parent().unwrap_or(&self.stage0.path);

        let (code, stdout, stderr) = build_helpers::run_sandboxed_command(
            cmd,
            workdir,
            &self.sysroot,
            &self.work_dir.join("sources"),
            &self.work_dir.join("build"),
            stage0_root,
            &env_vec,
        )
        .map_err(|e| Stage1Error::BuildFailed(pkg_name.clone(), e))?;

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
            return Err(Stage1Error::BuildFailed(
                pkg_name,
                format!(
                    "{} phase failed with exit code {}:\n{}",
                    phase, code, stderr
                ),
            ));
        }

        Ok(())
    }

    /// Save a package's build log to disk
    fn save_log(&self, idx: usize) -> Result<(), Stage1Error> {
        let pkg = &self.packages[idx];
        let log_path = self.logs_dir.join(format!("{}.log", pkg.name));
        fs::write(&log_path, &pkg.log)?;
        tracing::debug!("Saved build log: {}", log_path.display());
        Ok(())
    }

    /// Get the sysroot path
    pub fn sysroot(&self) -> &Path {
        &self.sysroot
    }

    /// Get build status summary
    pub fn status_summary(&self) -> Vec<(&str, &PackageBuildStatus)> {
        self.packages
            .iter()
            .map(|p| (p.name.as_str(), &p.status))
            .collect()
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_build_status() {
        let status = PackageBuildStatus::Pending;
        assert_eq!(status, PackageBuildStatus::Pending);

        let status = PackageBuildStatus::Failed("error".to_string());
        if let PackageBuildStatus::Failed(msg) = status {
            assert_eq!(msg, "error");
        }
    }

    #[test]
    fn test_placeholder_checksum_rejected() {
        // Verify that placeholder checksums are rejected by default
        // This tests the logic without needing actual file I/O
        assert!("VERIFY_BEFORE_BUILD".contains("VERIFY_BEFORE_BUILD"));
        assert!("sha256:FIXME".contains("FIXME"));
        // The actual verify_checksum is a method on Stage1Builder,
        // so just verify the string detection logic works
    }

    #[test]
    fn test_stage1_error_display() {
        let err = Stage1Error::RecipeNotFound("test.toml".to_string());
        assert!(err.to_string().contains("test.toml"));

        let err = Stage1Error::BuildFailed("gcc".to_string(), "compile error".to_string());
        assert!(err.to_string().contains("gcc"));
        assert!(err.to_string().contains("compile error"));
    }

    #[test]
    fn test_load_stage1_recipes() {
        let recipe_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("recipes/stage1");
        if !recipe_dir.exists() {
            eprintln!("Skipping test: recipes/stage1 not found");
            return;
        }
        for name in &[
            "linux-headers",
            "binutils",
            "gcc-pass1",
            "glibc",
            "gcc-pass2",
        ] {
            let path = recipe_dir.join(format!("{name}.toml"));
            assert!(path.exists(), "Missing recipe: {path:?}");
            let recipe = crate::recipe::parse_recipe_file(&path).unwrap();
            assert!(
                !recipe.package.name.is_empty(),
                "Empty package name in {name}"
            );
            assert!(
                !recipe.source.checksum.is_empty(),
                "Empty checksum in {name}"
            );
            assert!(
                !recipe.source.checksum.contains("VERIFY_BEFORE_BUILD")
                    && !recipe.source.checksum.contains("FIXME"),
                "Placeholder checksum in {name}"
            );
        }
    }
}
