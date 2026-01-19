// src/bootstrap/stage1.rs

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
use crate::recipe::{parse_recipe_file, Recipe};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use tracing::{debug, info, warn};

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

        // Create sysroot directory structure
        for dir in &["usr", "usr/bin", "usr/lib", "usr/include", "lib", "lib64", "bin"] {
            fs::create_dir_all(sysroot.join(dir))?;
        }

        // Set up build environment
        let mut build_env = stage0.env();

        // Add optimization flags (conservative for bootstrap)
        build_env.insert("CFLAGS".to_string(), "-O2 -pipe".to_string());
        build_env.insert("CXXFLAGS".to_string(), "-O2 -pipe".to_string());

        // Add sysroot to paths
        let sysroot_str = sysroot.to_string_lossy().to_string();
        build_env.insert("SYSROOT".to_string(), sysroot_str.clone());

        // Update PATH to include sysroot binaries
        let path = format!(
            "{}:{}/usr/bin:{}",
            stage0.bin_dir().display(),
            sysroot_str,
            std::env::var("PATH").unwrap_or_default()
        );
        build_env.insert("PATH".to_string(), path);

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
            self.packages[idx].log.push_str(&format!("Using cached source: {}\n", filename));
            return Ok(target_path);
        }

        info!("  Fetching: {}", url);
        self.packages[idx].log.push_str(&format!("Fetching: {}\n", url));

        let output = Command::new("curl")
            .args(["-fsSL", "-o", target_path.to_str().expect("path must be valid utf-8"), &url])
            .output()
            .map_err(|e| Stage1Error::SourceFetchFailed(pkg_name.clone(), e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Stage1Error::SourceFetchFailed(
                pkg_name,
                stderr.to_string(),
            ));
        }

        // Verify checksum
        self.verify_checksum(idx, &target_path)?;

        Ok(target_path)
    }

    /// Fetch additional sources (like GMP, MPFR, MPC for GCC)
    fn fetch_additional_sources(
        &mut self,
        idx: usize,
        src_dir: &Path,
    ) -> Result<(), Stage1Error> {
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

        for (url, _checksum, extract_to) in additional_sources {
            let filename = url.split('/').last().unwrap_or("additional.tar.gz");
            let target_path = sources_dir.join(filename);

            // Download if not cached
            if !target_path.exists() {
                info!("  Fetching additional: {}", filename);
                self.packages[idx].log.push_str(&format!("Fetching additional: {}\n", filename));

                let output = Command::new("curl")
                    .args(["-fsSL", "-o", target_path.to_str().expect("path must be valid utf-8"), &url])
                    .output()
                    .map_err(|e| {
                        Stage1Error::SourceFetchFailed(pkg_name.clone(), e.to_string())
                    })?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Stage1Error::SourceFetchFailed(
                        pkg_name.clone(),
                        stderr.to_string(),
                    ));
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

        // Skip if checksum is a placeholder
        if expected.contains("VERIFY_BEFORE_BUILD") || expected.contains("FIXME") {
            warn!("  Skipping checksum verification for {} (placeholder)", pkg_name);
            return Ok(());
        }

        // Extract algorithm and hash
        let (algo, hash) = expected
            .split_once(':')
            .ok_or_else(|| Stage1Error::SourceFetchFailed(
                pkg_name.clone(),
                "Invalid checksum format".to_string(),
            ))?;

        match algo {
            "sha256" => {
                let output = Command::new("sha256sum")
                    .arg(path)
                    .output()
                    .map_err(|e| {
                        Stage1Error::SourceFetchFailed(pkg_name.clone(), e.to_string())
                    })?;

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

    /// Extract a source archive, stripping the top-level directory
    ///
    /// This is used for in-tree dependencies like GMP, MPFR, MPC for GCC builds.
    fn extract_source_strip(&self, archive: &Path, dest: &Path) -> Result<(), Stage1Error> {
        fs::create_dir_all(dest)?;

        let filename = archive.file_name().expect("archive path must have a filename").to_string_lossy();

        let output = if filename.ends_with(".tar.xz") || filename.ends_with(".txz") {
            Command::new("tar")
                .args(["xJf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8"), "--strip-components=1"])
                .output()
        } else if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
            Command::new("tar")
                .args(["xzf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8"), "--strip-components=1"])
                .output()
        } else if filename.ends_with(".tar.bz2") || filename.ends_with(".tbz2") {
            Command::new("tar")
                .args(["xjf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8"), "--strip-components=1"])
                .output()
        } else {
            // Try generic tar
            Command::new("tar")
                .args(["xf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8"), "--strip-components=1"])
                .output()
        };

        let output = output.map_err(|e| {
            Stage1Error::BuildFailed("extract".to_string(), e.to_string())
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Stage1Error::BuildFailed(
                "extract".to_string(),
                stderr.to_string(),
            ));
        }

        Ok(())
    }

    /// Extract a source archive
    fn extract_source(&self, archive: &Path, dest: &Path) -> Result<(), Stage1Error> {
        fs::create_dir_all(dest)?;

        let filename = archive.file_name().expect("archive path must have a filename").to_string_lossy();

        let output = if filename.ends_with(".tar.xz") || filename.ends_with(".txz") {
            Command::new("tar")
                .args(["xJf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8")])
                .output()
        } else if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
            Command::new("tar")
                .args(["xzf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8")])
                .output()
        } else if filename.ends_with(".tar.bz2") || filename.ends_with(".tbz2") {
            Command::new("tar")
                .args(["xjf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8")])
                .output()
        } else {
            // Try generic tar
            Command::new("tar")
                .args(["xf", archive.to_str().expect("archive path must be valid utf-8"), "-C", dest.to_str().expect("dest path must be valid utf-8")])
                .output()
        };

        let output = output.map_err(|e| {
            Stage1Error::BuildFailed("extract".to_string(), e.to_string())
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Stage1Error::BuildFailed(
                "extract".to_string(),
                stderr.to_string(),
            ));
        }

        Ok(())
    }

    /// Find the actual source directory after extraction
    fn find_source_dir(&self, dir: &Path) -> Result<PathBuf, Stage1Error> {
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

    /// Run the configure phase
    fn run_configure(
        &mut self,
        idx: usize,
        src_dir: &Path,
        build_dir: &Path,
    ) -> Result<(), Stage1Error> {
        self.packages[idx].status = PackageBuildStatus::Configuring;

        // Clone data upfront to avoid borrow conflicts
        let configure_cmd = match &self.packages[idx].recipe.build.configure {
            Some(cmd) if !cmd.is_empty() => cmd.clone(),
            _ => {
                info!("  No configure step");
                return Ok(());
            }
        };
        let has_workdir = self.packages[idx].recipe.build.workdir.is_some();

        // Substitute variables
        let destdir = self.sysroot.to_string_lossy().to_string();
        let mut cmd = self.packages[idx].recipe.substitute(&configure_cmd, &destdir);

        // Substitute cross-compilation variables
        cmd = self.substitute_cross_vars(&cmd, idx);

        info!("  Configuring...");
        self.packages[idx].log.push_str(&format!("=== Configure ===\n{}\n", cmd));

        // Determine working directory - some packages build out-of-tree
        let workdir = if has_workdir {
            build_dir.to_path_buf()
        } else {
            src_dir.to_path_buf()
        };

        self.run_shell_command(idx, &cmd, &workdir, "configure")
    }

    /// Run the make phase
    fn run_make(&mut self, idx: usize, build_dir: &Path) -> Result<(), Stage1Error> {
        self.packages[idx].status = PackageBuildStatus::Building;

        // Clone data upfront
        let make_cmd = match &self.packages[idx].recipe.build.make {
            Some(cmd) => cmd.clone(),
            None => {
                info!("  No make step");
                return Ok(());
            }
        };

        // Substitute variables
        let destdir = self.sysroot.to_string_lossy().to_string();
        let mut cmd = self.packages[idx].recipe.substitute(&make_cmd, &destdir);
        cmd = self.substitute_cross_vars(&cmd, idx);

        info!("  Building...");
        self.packages[idx].log.push_str(&format!("=== Make ===\n{}\n", cmd));

        self.run_shell_command(idx, &cmd, build_dir, "make")
    }

    /// Run the install phase
    fn run_install(&mut self, idx: usize, build_dir: &Path) -> Result<(), Stage1Error> {
        self.packages[idx].status = PackageBuildStatus::Installing;

        // Clone data upfront
        let install_cmd = match &self.packages[idx].recipe.build.install {
            Some(cmd) => cmd.clone(),
            None => {
                info!("  No install step");
                return Ok(());
            }
        };

        // Substitute variables - for Stage 1, DESTDIR is the sysroot
        let destdir = self.sysroot.to_string_lossy().to_string();
        let mut cmd = self.packages[idx].recipe.substitute(&install_cmd, &destdir);
        cmd = self.substitute_cross_vars(&cmd, idx);

        info!("  Installing...");
        self.packages[idx].log.push_str(&format!("=== Install ===\n{}\n", cmd));

        self.run_shell_command(idx, &cmd, build_dir, "install")
    }

    /// Substitute cross-compilation variables in a command
    fn substitute_cross_vars(&self, cmd: &str, idx: usize) -> String {
        let pkg = &self.packages[idx];
        let mut result = cmd.to_string();

        // Standard substitutions from Stage1Builder
        result = result.replace("%(target)s", &self.config.triple());
        result = result.replace("%(jobs)s", &self.config.jobs.to_string());
        // Special variable for Stage1Builder's sysroot (where linux-headers are installed)
        result = result.replace("%(stage1_sysroot)s", &self.sysroot.to_string_lossy());

        // Cross-compilation section variables (recipe values)
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
    ) -> Result<(), Stage1Error> {
        use crate::container::{ContainerConfig, Sandbox, BindMount};

        // Clone all data upfront to avoid borrow conflicts
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
                // Prepend base flags
                let base_flags = self.build_env.get(&key).map(|s| s.as_str()).unwrap_or("");
                let merged = format!("{} {}", base_flags, value);
                // Remove existing if any
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

        // Prepare environment for sandbox (Vec<(&str, &str)>)
        let env_refs: Vec<(&str, &str)> = env_vec.iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        debug!("Running in sandbox: bash -c \"{}\"", cmd);
        debug!("Workdir: {}", workdir.display());

        // Configure sandbox
        // Stage 0 path usually ends in .../bin, we want the root (e.g. /tools)
        let stage0_root = self.stage0.path.parent().unwrap_or(&self.stage0.path);
        
        // We use pristine_for_bootstrap to set up the core mounts
        // sysroot -> /tools/x86_64.../sysroot (target sysroot)
        // stage0_root -> /tools (cross compiler)
        let mut config = ContainerConfig::pristine_for_bootstrap(
             &self.sysroot, // target sysroot (destdir)
             &self.work_dir.join("sources"), // source dir
             &self.work_dir.join("build"), // build dir
             &self.sysroot, // dest dir (same as sysroot)
        );

        // Explicitly mount Stage 0 tools at /tools so the cross-compiler is available
        // This is critical because `gcc` in the recipe refers to /tools/bin/...
        config.add_bind_mount(BindMount::readonly(stage0_root, "/tools"));

        // Set working directory
        config.workdir = workdir.to_path_buf();

        // Enforce network isolation during build
        config.deny_network();

        let mut sandbox = Sandbox::new(config);
        
        let (code, stdout, stderr) = sandbox.execute(
            "bash",
            &format!("set -e\n{}", cmd), // Wrap in script, fail fast
            &[],
            &env_refs
        ).map_err(|e| Stage1Error::BuildFailed(pkg_name.clone(), e.to_string()))?;

        // Log output
        if !stdout.is_empty() {
            self.packages[idx].log.push_str(&format!("stdout:\n{}\n", stdout));
            if verbose { println!("{}", stdout); }
        }
        if !stderr.is_empty() {
            self.packages[idx].log.push_str(&format!("stderr:\n{}\n", stderr));
            if verbose { eprintln!("{}", stderr); }
        }

        if code != 0 {
            return Err(Stage1Error::BuildFailed(
                pkg_name,
                format!("{} phase failed with exit code {}:\n{}", phase, code, stderr),
            ));
        }

        Ok(())
    }

    /// Save a package's build log to disk
    fn save_log(&self, idx: usize) -> Result<(), Stage1Error> {
        let pkg = &self.packages[idx];
        let log_path = self.logs_dir.join(format!("{}.log", pkg.name));
        fs::write(&log_path, &pkg.log)?;
        debug!("Saved build log: {}", log_path.display());
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

    /// Expand environment variable references in a string
    ///
    /// Supports both $VAR and ${VAR} syntax. First checks build_env,
    /// then falls back to the system environment.
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
                result = format!("{}{}{}", &result[..start], replacement, &result[start + end + 1..]);
            } else {
                break;
            }
        }

        // Handle $VAR syntax (must come after ${VAR} to avoid conflicts)
        let mut i = 0;
        while i < result.len() {
            if result[i..].starts_with('$') && !result[i..].starts_with("${") {
                // Find the end of the variable name
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
    fn test_stage1_error_display() {
        let err = Stage1Error::RecipeNotFound("test.toml".to_string());
        assert!(err.to_string().contains("test.toml"));

        let err = Stage1Error::BuildFailed("gcc".to_string(), "compile error".to_string());
        assert!(err.to_string().contains("gcc"));
        assert!(err.to_string().contains("compile error"));
    }
}
