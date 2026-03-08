// conary-core/src/bootstrap/base.rs

//! Base system builder
//!
//! Builds the complete base system using the Stage 1 toolchain.
//! This includes all packages needed for a bootable, self-hosting system.
//!
//! # Build Phases
//!
//! The base system is built in phases following dependency order:
//!
//! 1. **Libraries** - Core libraries (zlib, openssl, ncurses, etc.)
//! 2. **Dev Tools** - Build tools (make, autoconf, perl, python, etc.)
//! 3. **Core System** - Kernel, init, shell, utilities
//! 4. **Userland** - Text tools, networking, editors
//! 5. **Boot** - Bootloader and EFI tools
//!
//! After completion, the target root contains a complete bootable system.

use super::build_helpers;
use super::config::BootstrapConfig;
use super::toolchain::Toolchain;
use crate::container::{ContainerConfig, Sandbox};
use crate::recipe::{Recipe, RecipeGraph, parse_recipe_file};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use thiserror::Error;
use tracing::{debug, info, warn};

/// Errors that can occur during base system build
#[derive(Debug, Error)]
pub enum BaseError {
    #[error("Stage 1 toolchain not found at {0}")]
    Stage1NotFound(PathBuf),

    #[error("Recipe not found: {0}")]
    RecipeNotFound(String),

    #[error("Recipe parse error: {0}")]
    RecipeParseFailed(String),

    #[error("Source fetch failed for {0}: {1}")]
    SourceFetchFailed(String, String),

    #[error("Build failed for {0}: {1}")]
    BuildFailed(String, String),

    #[error("Failed to create directory: {0}")]
    DirectoryCreation(#[from] std::io::Error),

    #[error("Phase {0} failed: {1}")]
    PhaseFailed(String, String),

    #[error("I/O error: {0}")]
    IoError(String),

    #[error("Dependency cycle detected: {0}")]
    DependencyCycle(String),
}

/// Build phase for base system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseBuildPhase {
    /// Core libraries (compression, crypto, terminal)
    Libraries,
    /// Development tools (compilers, build systems)
    DevTools,
    /// Core system (kernel, init, shell)
    CoreSystem,
    /// Userland tools (text, archive, net)
    Userland,
    /// Boot tools (grub, efi)
    Boot,
}

impl BaseBuildPhase {
    /// Get phase name
    pub fn name(&self) -> &'static str {
        match self {
            Self::Libraries => "libraries",
            Self::DevTools => "dev-tools",
            Self::CoreSystem => "core-system",
            Self::Userland => "userland",
            Self::Boot => "boot",
        }
    }

    /// Get all phases in order
    pub fn all() -> &'static [BaseBuildPhase] {
        &[
            Self::Libraries,
            Self::DevTools,
            Self::CoreSystem,
            Self::Userland,
            Self::Boot,
        ]
    }
}

impl std::fmt::Display for BaseBuildPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Status of a package build
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseBuildStatus {
    Pending,
    Building,
    Complete,
    Failed(String),
    Skipped(String),
}

/// A package in the base system build
#[derive(Debug)]
pub struct BasePackage {
    /// Package name
    pub name: String,
    /// Recipe category/directory
    pub category: String,
    /// Build phase
    pub phase: BaseBuildPhase,
    /// Recipe (loaded lazily)
    pub recipe: Option<Recipe>,
    /// Build status
    pub status: BaseBuildStatus,
    /// Build log
    pub log: String,
}

/// Builder for base system
pub struct BaseBuilder {
    /// Base work directory
    work_dir: PathBuf,

    /// Bootstrap configuration
    config: BootstrapConfig,

    /// Stage 1 toolchain
    toolchain: Toolchain,

    /// Target root filesystem
    target_root: PathBuf,

    /// Source cache directory
    sources_dir: PathBuf,

    /// Build logs directory
    logs_dir: PathBuf,

    /// Recipe base directory
    recipe_dir: PathBuf,

    /// Packages to build
    packages: Vec<BasePackage>,

    /// Recipes loaded from directory (name, recipe) for graph-based ordering
    recipes: Vec<(String, Recipe)>,

    /// Environment variables for builds
    build_env: HashMap<String, String>,

    /// Current phase
    current_phase: Option<BaseBuildPhase>,
}

impl BaseBuilder {
    /// Packages for each build phase (name, category)
    const LIBRARY_PACKAGES: &'static [(&'static str, &'static str)] = &[
        ("zlib", "libs"),
        ("xz", "libs"),
        ("zstd", "libs"),
        ("ncurses", "libs"),
        ("readline", "libs"),
        ("openssl", "libs"),
        ("libcap", "libs"),
        ("libmnl", "libs"),
        ("elfutils", "libs"),
        ("kmod", "libs"),
        ("dbus", "libs"),
        ("linux-pam", "libs"),
    ];

    const DEV_PACKAGES: &'static [(&'static str, &'static str)] = &[
        ("make", "dev"),
        ("m4", "dev"),
        ("autoconf", "dev"),
        ("automake", "dev"),
        ("libtool", "dev"),
        ("pkgconf", "dev"),
        ("bison", "dev"),
        ("flex", "dev"),
        ("gettext", "dev"),
        ("perl", "dev"),
        ("python", "dev"),
        ("cmake", "dev"),
        ("ninja", "dev"),
        ("meson", "dev"),
    ];

    const CORE_PACKAGES: &'static [(&'static str, &'static str)] = &[
        ("linux", "base"),
        ("coreutils", "base"),
        ("bash", "base"),
        ("util-linux", "base"),
        ("systemd", "base"),
        ("iproute2", "base"),
        ("openssh", "base"),
    ];

    const USERLAND_PACKAGES: &'static [(&'static str, &'static str)] = &[
        // Text processing
        ("grep", "text"),
        ("sed", "text"),
        ("gawk", "text"),
        ("less", "text"),
        ("diffutils", "text"),
        ("patch", "text"),
        ("findutils", "text"),
        ("file", "text"),
        // Archive
        ("tar", "archive"),
        ("gzip", "archive"),
        ("bzip2", "archive"),
        ("cpio", "archive"),
        // Networking
        ("ca-certificates", "net"),
        ("curl", "net"),
        ("wget2", "net"),
        // VCS
        ("git", "vcs"),
        // System utilities
        ("procps-ng", "sys"),
        ("psmisc", "sys"),
        ("shadow", "sys"),
        ("sudo", "sys"),
        // Editors
        ("vim", "editors"),
        ("nano", "editors"),
    ];

    const BOOT_PACKAGES: &'static [(&'static str, &'static str)] = &[
        ("popt", "boot"),
        ("efivar", "boot"),
        ("efibootmgr", "boot"),
        ("dosfstools", "boot"),
        ("grub", "boot"),
    ];

    /// Create a new base system builder
    pub fn new(
        work_dir: impl AsRef<Path>,
        config: &BootstrapConfig,
        toolchain: Toolchain,
        target_root: impl AsRef<Path>,
        recipe_dir: impl AsRef<Path>,
    ) -> Result<Self, BaseError> {
        let work_dir = work_dir.as_ref().to_path_buf();
        let base_dir = work_dir.join("base");
        let target_root = target_root.as_ref().to_path_buf();
        let recipe_dir = recipe_dir.as_ref().to_path_buf();

        // Create directory structure
        let sources_dir = work_dir.join("sources");
        let logs_dir = base_dir.join("logs");

        fs::create_dir_all(&base_dir)?;
        fs::create_dir_all(&sources_dir)?;
        fs::create_dir_all(&logs_dir)?;

        // Create target root filesystem structure
        Self::create_root_structure(&target_root)?;

        // Set up build environment
        let mut build_env = toolchain.env();

        // Standard build flags with target sysroot paths
        let include_path = format!("-I{}/usr/include", target_root.display());
        let lib_path = format!(
            "-L{}/usr/lib -L{}/lib -Wl,-rpath-link,{}/usr/lib",
            target_root.display(),
            target_root.display(),
            target_root.display()
        );
        build_env.insert("CFLAGS".to_string(), format!("-O2 -pipe {}", include_path));
        build_env.insert(
            "CXXFLAGS".to_string(),
            format!("-O2 -pipe {}", include_path),
        );
        build_env.insert("LDFLAGS".to_string(), lib_path.clone());

        // Also set LIBRARY_PATH for static linking and tools that don't use LDFLAGS
        build_env.insert(
            "LIBRARY_PATH".to_string(),
            format!(
                "{}/usr/lib:{}/lib",
                target_root.display(),
                target_root.display()
            ),
        );

        // Set C_INCLUDE_PATH and CPLUS_INCLUDE_PATH for compilers
        let include_dir = format!("{}/usr/include", target_root.display());
        build_env.insert("C_INCLUDE_PATH".to_string(), include_dir.clone());
        build_env.insert("CPLUS_INCLUDE_PATH".to_string(), include_dir);

        // PKG_CONFIG paths
        let pkg_config_path = format!(
            "{}/usr/lib/pkgconfig:{}/usr/share/pkgconfig",
            target_root.display(),
            target_root.display()
        );
        build_env.insert("PKG_CONFIG_PATH".to_string(), pkg_config_path);

        // Allow configure scripts to run as root (needed for bootstrap)
        build_env.insert("FORCE_UNSAFE_CONFIGURE".to_string(), "1".to_string());

        Ok(Self {
            work_dir: base_dir,
            config: config.clone(),
            toolchain,
            target_root,
            sources_dir,
            logs_dir,
            recipe_dir,
            packages: Vec::new(),
            recipes: Vec::new(),
            build_env,
            current_phase: None,
        })
    }

    /// Create the root filesystem directory structure
    fn create_root_structure(root: &Path) -> Result<(), BaseError> {
        let dirs = [
            "bin",
            "boot",
            "dev",
            "etc",
            "home",
            "lib",
            "lib64",
            "media",
            "mnt",
            "opt",
            "proc",
            "root",
            "run",
            "sbin",
            "srv",
            "sys",
            "tmp",
            "usr",
            "var",
            "usr/bin",
            "usr/include",
            "usr/lib",
            "usr/lib64",
            "usr/libexec",
            "usr/sbin",
            "usr/share",
            "usr/src",
            "usr/share/man",
            "usr/share/doc",
            "usr/share/info",
            "var/cache",
            "var/lib",
            "var/lock",
            "var/log",
            "var/mail",
            "var/opt",
            "var/run",
            "var/spool",
            "var/tmp",
            "etc/sysconfig",
            "etc/profile.d",
        ];

        for dir in &dirs {
            fs::create_dir_all(root.join(dir))?;
        }

        // Set proper permissions on /tmp
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tmp = root.join("tmp");
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o1777))?;
        }

        Ok(())
    }

    fn phase_packages(phase: BaseBuildPhase) -> &'static [(&'static str, &'static str)] {
        match phase {
            BaseBuildPhase::Libraries => Self::LIBRARY_PACKAGES,
            BaseBuildPhase::DevTools => Self::DEV_PACKAGES,
            BaseBuildPhase::CoreSystem => Self::CORE_PACKAGES,
            BaseBuildPhase::Userland => Self::USERLAND_PACKAGES,
            BaseBuildPhase::Boot => Self::BOOT_PACKAGES,
        }
    }

    /// Initialize packages for all phases
    pub fn init_packages(&mut self) -> Result<(), BaseError> {
        self.packages.clear();

        for &phase in BaseBuildPhase::all() {
            for (name, category) in Self::phase_packages(phase) {
                self.packages.push(BasePackage {
                    name: (*name).to_string(),
                    category: (*category).to_string(),
                    phase,
                    recipe: None,
                    status: BaseBuildStatus::Pending,
                    log: String::new(),
                });
            }
        }

        info!(
            "Initialized {} packages for base system build",
            self.packages.len()
        );
        Ok(())
    }

    /// Load all recipes from a directory and resolve build order via dependency graph.
    ///
    /// Scans `recipe_dir` for `.toml` files, parses each as a Recipe, adds them
    /// to a `RecipeGraph`, and returns the topologically sorted build order.
    /// This replaces the hardcoded phase-based ordering with graph-based ordering
    /// derived from actual recipe dependencies.
    pub fn load_recipes_from_dir(&mut self, recipe_dir: &Path) -> Result<Vec<String>, BaseError> {
        let mut graph = RecipeGraph::new();
        self.recipes.clear();

        for entry in std::fs::read_dir(recipe_dir)
            .map_err(|e| BaseError::IoError(format!("Cannot read recipe dir: {e}")))?
        {
            let entry = entry.map_err(|e| BaseError::IoError(e.to_string()))?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                let recipe = parse_recipe_file(&path).map_err(|e| {
                    BaseError::RecipeParseFailed(format!("{}: {e}", path.display()))
                })?;
                graph.add_from_recipe(&recipe);
                self.recipes.push((recipe.package.name.clone(), recipe));
            }
        }

        let order = graph
            .topological_sort()
            .map_err(|e| BaseError::DependencyCycle(format!("{e}")))?;

        info!(
            "Loaded {} base recipes, resolved build order ({} packages)",
            self.recipes.len(),
            order.len()
        );
        Ok(order)
    }

    /// Classify a package into a build phase for progress reporting.
    ///
    /// This preserves the phase classification from the hardcoded package lists
    /// for use in progress reporting and UI, even when using graph-based ordering.
    pub fn package_phase(name: &str) -> BaseBuildPhase {
        match name {
            "zlib" | "xz" | "zstd" | "ncurses" | "readline" | "openssl" | "libcap" | "libmnl"
            | "elfutils" | "kmod" | "dbus" | "linux-pam" => BaseBuildPhase::Libraries,
            "make" | "m4" | "autoconf" | "automake" | "libtool" | "pkgconf" | "bison" | "flex"
            | "gettext" | "perl" | "python" | "cmake" | "ninja" | "meson" => {
                BaseBuildPhase::DevTools
            }
            "linux" | "coreutils" | "bash" | "util-linux" | "systemd" => {
                BaseBuildPhase::CoreSystem
            }
            "grub" | "dosfstools" | "efivar" | "efibootmgr" | "popt" | "dracut" => {
                BaseBuildPhase::Boot
            }
            _ => BaseBuildPhase::Userland,
        }
    }

    /// Initialize packages from graph-based build order.
    ///
    /// Uses the topologically sorted order from `load_recipes_from_dir` to
    /// populate the package list with proper phase classification.
    pub fn init_packages_from_order(&mut self, order: &[String]) {
        self.packages.clear();

        for name in order {
            let phase = Self::package_phase(name);
            let recipe = self
                .recipes
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, r)| r.clone());

            self.packages.push(BasePackage {
                name: name.clone(),
                category: phase.name().to_string(),
                phase,
                recipe,
                status: BaseBuildStatus::Pending,
                log: String::new(),
            });
        }

        info!(
            "Initialized {} packages from graph-resolved order",
            self.packages.len()
        );
    }

    /// Load recipe for a package
    fn load_recipe(&self, pkg: &BasePackage) -> Result<Recipe, BaseError> {
        let recipe_path = self
            .recipe_dir
            .join(&pkg.category)
            .join(format!("{}.toml", pkg.name));

        if !recipe_path.exists() {
            return Err(BaseError::RecipeNotFound(format!(
                "{}",
                recipe_path.display()
            )));
        }

        parse_recipe_file(&recipe_path).map_err(|e| BaseError::RecipeParseFailed(e.to_string()))
    }

    /// Build all packages
    pub fn build(&mut self) -> Result<(), BaseError> {
        info!("Building base system...");
        info!("Target root: {}", self.target_root.display());
        info!("Using toolchain: {}", self.toolchain.path.display());
        info!("Total packages: {}", self.packages.len());

        for phase in BaseBuildPhase::all() {
            self.current_phase = Some(*phase);
            self.build_phase(*phase)?;
        }

        info!("Base system build complete!");
        Ok(())
    }

    /// Build a specific phase
    pub fn build_phase(&mut self, phase: BaseBuildPhase) -> Result<(), BaseError> {
        info!("\n=== Building phase: {} ===", phase);

        let package_indices: Vec<usize> = self
            .packages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.phase == phase)
            .map(|(i, _)| i)
            .collect();

        let phase_count = package_indices.len();

        for (num, idx) in package_indices.iter().enumerate() {
            let pkg_name = self.packages[*idx].name.clone();
            let pkg_category = self.packages[*idx].category.clone();

            info!("[{}/{}] Building {}...", num + 1, phase_count, pkg_name);

            // Load recipe if not already loaded
            if self.packages[*idx].recipe.is_none() {
                match self.load_recipe(&self.packages[*idx]) {
                    Ok(recipe) => {
                        self.packages[*idx].recipe = Some(recipe);
                    }
                    Err(e) => {
                        warn!("  Skipping {}: {}", pkg_name, e);
                        self.packages[*idx].status = BaseBuildStatus::Skipped(e.to_string());
                        continue;
                    }
                }
            }

            match self.build_package(*idx) {
                Ok(()) => {
                    self.packages[*idx].status = BaseBuildStatus::Complete;
                    info!("  [OK] {} built successfully", pkg_name);
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    self.packages[*idx].status = BaseBuildStatus::Failed(err_msg.clone());
                    self.save_log(*idx)?;

                    // Check if this is a critical package
                    if Self::is_critical(&pkg_name, &pkg_category) {
                        return Err(BaseError::PhaseFailed(
                            phase.to_string(),
                            format!("Critical package {} failed: {}", pkg_name, err_msg),
                        ));
                    } else {
                        warn!("  [WARN] {} failed (non-critical): {}", pkg_name, err_msg);
                    }
                }
            }

            self.save_log(*idx)?;
        }

        Ok(())
    }

    /// Check if a package is critical (failure should stop the build)
    fn is_critical(name: &str, _category: &str) -> bool {
        // These packages are essential - build cannot continue without them
        matches!(
            name,
            "zlib"
                | "ncurses"
                | "readline"
                | "openssl"
                | "make"
                | "bash"
                | "coreutils"
                | "linux"
                | "util-linux"
                | "grep"
                | "sed"
                | "tar"
                | "gzip"
        )
    }

    /// Build a single package
    fn build_package(&mut self, idx: usize) -> Result<(), BaseError> {
        let pkg_name = self.packages[idx].name.clone();
        self.packages[idx].status = BaseBuildStatus::Building;

        // Create build directory
        let build_base = self.work_dir.join("build").join(&pkg_name);
        let src_dir = build_base.join("src");
        let build_dir = build_base.join("build");

        // Clean previous build
        if build_base.exists() {
            fs::remove_dir_all(&build_base)?;
        }
        fs::create_dir_all(&src_dir)?;
        fs::create_dir_all(&build_dir)?;

        // Fetch and extract source
        let archive_path = self.fetch_source(idx)?;
        self.extract_source(&archive_path, &src_dir)?;
        let actual_src_dir = self.find_source_dir(&src_dir)?;

        // Fetch additional sources
        self.fetch_additional_sources(idx, &actual_src_dir)?;

        // Determine working directory
        let has_workdir = self.packages[idx]
            .recipe
            .as_ref()
            .and_then(|r| r.build.workdir.as_ref())
            .is_some();
        let workdir = if has_workdir {
            &build_dir
        } else {
            &actual_src_dir
        };

        // Run build phases
        self.run_setup(idx, &actual_src_dir, &build_dir)?;
        self.run_configure(idx, workdir)?;
        self.run_make(idx, workdir)?;
        self.run_install(idx, workdir)?;

        Ok(())
    }

    /// Fetch source archive
    fn fetch_source(&mut self, idx: usize) -> Result<PathBuf, BaseError> {
        let recipe = self.packages[idx]
            .recipe
            .as_ref()
            .expect("recipe must be loaded before build");
        let pkg_name = self.packages[idx].name.clone();
        let url = recipe.archive_url();
        let filename = recipe.archive_filename();
        let target_path = self.sources_dir.join(&filename);

        if target_path.exists() {
            info!("  Using cached source: {}", filename);
            self.packages[idx]
                .log
                .push_str(&format!("Using cached: {}\n", filename));
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
            .map_err(|e| BaseError::SourceFetchFailed(pkg_name.clone(), e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BaseError::SourceFetchFailed(pkg_name, stderr.to_string()));
        }

        // Verify checksum
        self.verify_checksum(idx, &target_path)?;

        Ok(target_path)
    }

    /// Fetch additional sources
    fn fetch_additional_sources(&mut self, idx: usize, src_dir: &Path) -> Result<(), BaseError> {
        let recipe = self.packages[idx]
            .recipe
            .as_ref()
            .expect("recipe must be loaded before build");
        let pkg_name = self.packages[idx].name.clone();

        let additional: Vec<_> = recipe
            .source
            .additional
            .iter()
            .map(|a| (a.url.clone(), a.extract_to.clone()))
            .collect();

        for (url, extract_to) in additional {
            let filename = url.split('/').next_back().unwrap_or("additional.tar.gz");
            let target_path = self.sources_dir.join(filename);

            if !target_path.exists() {
                info!("  Fetching additional: {}", filename);
                self.packages[idx]
                    .log
                    .push_str(&format!("Fetching: {}\n", filename));

                let output = Command::new("curl")
                    .args([
                        "-fsSL",
                        "-o",
                        target_path.to_str().expect("path must be valid utf-8"),
                        &url,
                    ])
                    .output()
                    .map_err(|e| BaseError::SourceFetchFailed(pkg_name.clone(), e.to_string()))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(BaseError::SourceFetchFailed(
                        pkg_name.clone(),
                        stderr.to_string(),
                    ));
                }
            }

            let extract_dest = extract_to
                .map(|d| src_dir.join(d))
                .unwrap_or_else(|| src_dir.to_path_buf());

            self.extract_source(&target_path, &extract_dest)?;
        }

        Ok(())
    }

    /// Verify checksum
    fn verify_checksum(&self, idx: usize, path: &Path) -> Result<(), BaseError> {
        let recipe = self.packages[idx]
            .recipe
            .as_ref()
            .expect("recipe must be loaded before build");
        let pkg_name = &self.packages[idx].name;
        let expected = &recipe.source.checksum;

        // Reject placeholder checksums unless skip_verify is enabled
        if expected.contains("VERIFY_BEFORE_BUILD") || expected.contains("FIXME") {
            if self.config.skip_verify {
                warn!(
                    "  Skipping placeholder checksum (--skip-verify enabled): {}",
                    expected
                );
                return Ok(());
            }
            return Err(BaseError::SourceFetchFailed(
                pkg_name.clone(),
                format!(
                    "Recipe has placeholder checksum '{}' -- provide a real SHA-256 or use --skip-verify",
                    expected
                ),
            ));
        }

        let (algo, hash) = expected.split_once(':').ok_or_else(|| {
            BaseError::SourceFetchFailed(pkg_name.clone(), "Invalid checksum format".to_string())
        })?;

        if algo == "sha256" {
            let output = Command::new("sha256sum")
                .arg(path)
                .output()
                .map_err(|e| BaseError::SourceFetchFailed(pkg_name.clone(), e.to_string()))?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let computed = stdout.split_whitespace().next().unwrap_or("");

            if computed != hash {
                return Err(BaseError::SourceFetchFailed(
                    pkg_name.clone(),
                    format!("Checksum mismatch: expected {}, got {}", hash, computed),
                ));
            }
        }

        Ok(())
    }

    fn extract_source(&self, archive: &Path, dest: &Path) -> Result<(), BaseError> {
        build_helpers::extract_tar(archive, dest, false)
            .map_err(|e| BaseError::BuildFailed("extract".to_string(), e))
    }

    /// Find actual source directory
    fn find_source_dir(&self, dir: &Path) -> Result<PathBuf, BaseError> {
        build_helpers::find_source_dir(dir).map_err(BaseError::DirectoryCreation)
    }

    /// Run setup phase
    fn run_setup(&mut self, idx: usize, src_dir: &Path, build_dir: &Path) -> Result<(), BaseError> {
        let recipe = self.packages[idx]
            .recipe
            .as_ref()
            .expect("recipe must be loaded before build");

        if let Some(setup) = &recipe.build.setup {
            let setup = setup.clone();
            let destdir = self.target_root.to_string_lossy().to_string();
            let cmd = recipe.substitute(&setup, &destdir);

            info!("  Running setup...");
            self.packages[idx]
                .log
                .push_str(&format!("=== Setup ===\n{}\n", cmd));

            // Setup typically creates build directory and cd's into it
            self.run_shell_command(idx, &cmd, src_dir, "setup")?;
        }

        // Ensure build directory exists
        fs::create_dir_all(build_dir)?;

        Ok(())
    }

    fn run_recipe_phase(
        &mut self,
        idx: usize,
        workdir: &Path,
        phase_name: &str,
        get_cmd: fn(&Recipe) -> Option<&String>,
    ) -> Result<(), BaseError> {
        let recipe = self.packages[idx]
            .recipe
            .as_ref()
            .expect("recipe must be loaded before build");

        let raw_cmd = match get_cmd(recipe) {
            Some(cmd) if !cmd.is_empty() => cmd.clone(),
            _ => return Ok(()),
        };

        let destdir = self.target_root.to_string_lossy().to_string();
        let mut cmd = recipe.substitute(&raw_cmd, &destdir);
        cmd = self.substitute_vars(&cmd);

        info!("  {}...", phase_name);
        self.packages[idx]
            .log
            .push_str(&format!("=== {} ===\n{}\n", phase_name, cmd));

        self.run_shell_command(idx, &cmd, workdir, phase_name)
    }

    fn run_configure(&mut self, idx: usize, workdir: &Path) -> Result<(), BaseError> {
        self.run_recipe_phase(idx, workdir, "configure", |r| r.build.configure.as_ref())
    }

    fn run_make(&mut self, idx: usize, workdir: &Path) -> Result<(), BaseError> {
        self.run_recipe_phase(idx, workdir, "make", |r| r.build.make.as_ref())
    }

    fn run_install(&mut self, idx: usize, workdir: &Path) -> Result<(), BaseError> {
        self.run_recipe_phase(idx, workdir, "install", |r| r.build.install.as_ref())
    }

    /// Substitute build variables
    fn substitute_vars(&self, cmd: &str) -> String {
        let mut result = cmd.to_string();

        result = result.replace("%(jobs)s", &self.config.jobs.to_string());
        result = result.replace("%(destdir)s", &self.target_root.to_string_lossy());

        result
    }

    /// Run a shell command, with optional sandbox isolation.
    ///
    /// On Linux, attempts to run the command inside a pristine container
    /// (using `ContainerConfig::pristine_for_bootstrap`) to prevent host
    /// toolchain contamination. Falls back to direct execution if sandboxing
    /// is unavailable.
    fn run_shell_command(
        &mut self,
        idx: usize,
        cmd: &str,
        workdir: &Path,
        phase: &str,
    ) -> Result<(), BaseError> {
        let pkg_name = self.packages[idx].name.clone();
        let recipe = self.packages[idx]
            .recipe
            .as_ref()
            .expect("recipe must be loaded before build");

        // Get environment from recipe
        let recipe_env: HashMap<String, String> = recipe
            .build
            .environment
            .iter()
            .map(|(k, v)| {
                let value = recipe.substitute(v, &self.target_root.to_string_lossy());
                (k.clone(), value)
            })
            .collect();

        let verbose = self.config.verbose;

        debug!("Running: bash -c \"{}\"", cmd);
        debug!("Workdir: {}", workdir.display());

        // Merge all environment variables for both sandbox and direct execution
        let mut all_env: Vec<(&str, String)> = Vec::new();
        for (key, value) in &self.build_env {
            all_env.push((key.as_str(), value.clone()));
        }
        for (key, value) in &recipe_env {
            all_env.push((key.as_str(), value.clone()));
        }

        // Try sandboxed execution on Linux
        #[cfg(target_os = "linux")]
        let sandbox_result = {
            let build_dir = self.work_dir.join("build").join(&pkg_name);
            let container_config = ContainerConfig::pristine_for_bootstrap(
                &self.toolchain.path,
                workdir,
                &build_dir,
                &self.target_root,
            );
            let mut sandbox = Sandbox::new(container_config);

            let env_refs: Vec<(&str, &str)> =
                all_env.iter().map(|(k, v)| (*k, v.as_str())).collect();

            sandbox.execute("bash", cmd, &[], &env_refs)
        };

        #[cfg(not(target_os = "linux"))]
        let sandbox_result: std::result::Result<(i32, String, String), crate::error::Error> =
            Err(crate::error::Error::ScriptletError(
                "Sandbox not available on non-Linux".to_string(),
            ));

        match sandbox_result {
            Ok((code, stdout, stderr)) => {
                if !stdout.is_empty() {
                    self.packages[idx]
                        .log
                        .push_str(&format!("stdout:\n{}\n", stdout));
                }
                if !stderr.is_empty() {
                    self.packages[idx]
                        .log
                        .push_str(&format!("stderr:\n{}\n", stderr));
                }

                if code != 0 {
                    return Err(BaseError::BuildFailed(
                        pkg_name,
                        format!("{} phase failed (sandboxed, exit {}):\n{}", phase, code, stderr),
                    ));
                }

                debug!("  [sandbox] {} completed successfully", phase);
                Ok(())
            }
            Err(sandbox_err) => {
                debug!(
                    "Sandbox unavailable ({}), falling back to direct execution",
                    sandbox_err
                );

                // Fall back to direct execution
                let mut command = Command::new("bash");
                command.arg("-c").arg(cmd).current_dir(workdir);

                for (key, value) in &self.build_env {
                    command.env(key, value);
                }
                for (key, value) in &recipe_env {
                    command.env(key, value);
                }

                if verbose {
                    command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
                } else {
                    command.stdout(Stdio::piped()).stderr(Stdio::piped());
                }

                let output = command
                    .output()
                    .map_err(|e| BaseError::BuildFailed(pkg_name.clone(), e.to_string()))?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if !stdout.is_empty() {
                    self.packages[idx]
                        .log
                        .push_str(&format!("stdout:\n{}\n", stdout));
                }
                if !stderr.is_empty() {
                    self.packages[idx]
                        .log
                        .push_str(&format!("stderr:\n{}\n", stderr));
                }

                if !output.status.success() {
                    return Err(BaseError::BuildFailed(
                        pkg_name,
                        format!("{} phase failed:\n{}", phase, stderr),
                    ));
                }

                Ok(())
            }
        }
    }

    /// Save package build log
    fn save_log(&self, idx: usize) -> Result<(), BaseError> {
        let pkg = &self.packages[idx];
        let log_path = self.logs_dir.join(format!("{}.log", pkg.name));
        fs::write(&log_path, &pkg.log)?;
        debug!("Saved build log: {}", log_path.display());
        Ok(())
    }

    /// Get build summary
    pub fn summary(&self) -> BuildSummary {
        let mut complete = 0;
        let mut failed = 0;
        let mut skipped = 0;
        let mut pending = 0;

        for pkg in &self.packages {
            match &pkg.status {
                BaseBuildStatus::Complete => complete += 1,
                BaseBuildStatus::Failed(_) => failed += 1,
                BaseBuildStatus::Skipped(_) => skipped += 1,
                BaseBuildStatus::Pending | BaseBuildStatus::Building => pending += 1,
            }
        }

        BuildSummary {
            total: self.packages.len(),
            complete,
            failed,
            skipped,
            pending,
        }
    }

    /// Get target root path
    pub fn target_root(&self) -> &Path {
        &self.target_root
    }
}

/// Build summary statistics
#[derive(Debug)]
pub struct BuildSummary {
    pub total: usize,
    pub complete: usize,
    pub failed: usize,
    pub skipped: usize,
    pub pending: usize,
}

impl std::fmt::Display for BuildSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Total: {}, Complete: {}, Failed: {}, Skipped: {}, Pending: {}",
            self.total, self.complete, self.failed, self.skipped, self.pending
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_phase_order() {
        let phases = BaseBuildPhase::all();
        assert_eq!(phases.len(), 5);
        assert_eq!(phases[0], BaseBuildPhase::Libraries);
        assert_eq!(phases[4], BaseBuildPhase::Boot);
    }

    #[test]
    fn test_build_phase_name() {
        assert_eq!(BaseBuildPhase::Libraries.name(), "libraries");
        assert_eq!(BaseBuildPhase::Boot.name(), "boot");
    }

    #[test]
    fn test_package_counts() {
        let lib_count = BaseBuilder::LIBRARY_PACKAGES.len();
        let dev_count = BaseBuilder::DEV_PACKAGES.len();
        let core_count = BaseBuilder::CORE_PACKAGES.len();
        let user_count = BaseBuilder::USERLAND_PACKAGES.len();
        let boot_count = BaseBuilder::BOOT_PACKAGES.len();

        let total = lib_count + dev_count + core_count + user_count + boot_count;
        assert_eq!(total, 60); // 12 + 14 + 7 + 22 + 5
    }

    #[test]
    fn test_is_critical() {
        assert!(BaseBuilder::is_critical("zlib", "libs"));
        assert!(BaseBuilder::is_critical("bash", "base"));
        assert!(BaseBuilder::is_critical("linux", "base"));
        assert!(!BaseBuilder::is_critical("vim", "editors"));
        assert!(!BaseBuilder::is_critical("git", "vcs"));
    }

    #[test]
    fn test_package_phase_classification() {
        assert_eq!(
            BaseBuilder::package_phase("zlib"),
            BaseBuildPhase::Libraries
        );
        assert_eq!(
            BaseBuilder::package_phase("openssl"),
            BaseBuildPhase::Libraries
        );
        assert_eq!(
            BaseBuilder::package_phase("make"),
            BaseBuildPhase::DevTools
        );
        assert_eq!(
            BaseBuilder::package_phase("cmake"),
            BaseBuildPhase::DevTools
        );
        assert_eq!(
            BaseBuilder::package_phase("bash"),
            BaseBuildPhase::CoreSystem
        );
        assert_eq!(
            BaseBuilder::package_phase("systemd"),
            BaseBuildPhase::CoreSystem
        );
        assert_eq!(BaseBuilder::package_phase("grub"), BaseBuildPhase::Boot);
        assert_eq!(
            BaseBuilder::package_phase("vim"),
            BaseBuildPhase::Userland
        );
        assert_eq!(
            BaseBuilder::package_phase("unknown-pkg"),
            BaseBuildPhase::Userland
        );
    }

    #[test]
    fn test_load_recipes_from_dir() {
        use crate::bootstrap::toolchain::ToolchainKind;

        let recipe_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("recipes/base");
        if !recipe_dir.exists() {
            eprintln!("Skipping test_load_recipes_from_dir: recipes/base not found");
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        std::fs::create_dir_all(&target).unwrap();

        let tools_path = temp.path().join("tools");
        std::fs::create_dir_all(&tools_path).unwrap();

        let config = BootstrapConfig::new().with_skip_verify(true);
        let toolchain = Toolchain {
            kind: ToolchainKind::Stage1,
            path: tools_path,
            target: "x86_64-conary-linux-gnu".to_string(),
            gcc_version: None,
            glibc_version: None,
            binutils_version: None,
            is_static: false,
        };

        let mut builder = BaseBuilder::new(
            temp.path().join("work"),
            &config,
            toolchain,
            &target,
            &recipe_dir,
        )
        .unwrap();

        let order = builder.load_recipes_from_dir(&recipe_dir).unwrap();
        assert!(!order.is_empty(), "Should load at least one recipe");

        // Verify graph ordering: zlib should come before packages that depend on it
        if let (Some(zlib_pos), Some(tar_pos)) = (
            order.iter().position(|n| n == "zlib"),
            order.iter().position(|n| n == "tar"),
        ) {
            assert!(
                zlib_pos < tar_pos,
                "zlib should come before tar in build order"
            );
        }

        // Test init_packages_from_order
        builder.init_packages_from_order(&order);
        assert_eq!(builder.packages.len(), order.len());
    }

    #[test]
    fn test_build_summary_display() {
        let summary = BuildSummary {
            total: 52,
            complete: 40,
            failed: 2,
            skipped: 5,
            pending: 5,
        };

        let s = summary.to_string();
        assert!(s.contains("52"));
        assert!(s.contains("40"));
    }
}
