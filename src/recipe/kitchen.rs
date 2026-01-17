// src/recipe/kitchen.rs

//! Kitchen: the isolated build environment for cooking recipes
//!
//! The Kitchen provides a sandboxed environment for building packages
//! from source recipes. It handles:
//! - Fetching source archives and patches
//! - Extracting and patching sources
//! - Running build commands in isolation
//! - Packaging the result as CCS

use crate::ccs::builder::{write_ccs_package, CcsBuilder};
use crate::ccs::manifest::{CcsManifest, PackageDep};
use crate::container::{BindMount, ContainerConfig, Sandbox};
use crate::error::{Error, Result};
use crate::hash::{hash_bytes, HashAlgorithm};
use crate::recipe::cache::{BuildCache, ToolchainInfo};
use crate::recipe::format::Recipe;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// Trait for resolving and installing makedepends before building
///
/// This allows the Kitchen to remain decoupled from the package installation
/// logic while still being able to ensure build dependencies are available.
pub trait MakedependsResolver: Send + Sync {
    /// Check which makedepends are missing
    ///
    /// Returns a list of package names that are not currently installed.
    fn check_missing(&self, deps: &[&str]) -> Result<Vec<String>>;

    /// Install the specified makedepends
    ///
    /// Should install the packages and return the list of packages that
    /// were actually installed (for later cleanup).
    fn install(&self, deps: &[String]) -> Result<Vec<String>>;

    /// Uninstall packages that were installed as makedepends
    ///
    /// Called after build completes to clean up temporary dependencies.
    /// Only removes packages that were installed by this build.
    fn cleanup(&self, installed: &[String]) -> Result<()>;
}

/// A no-op resolver that assumes all dependencies are satisfied
///
/// Use this when you want to skip makedepends resolution entirely
/// (e.g., in a pre-configured build container).
pub struct NoopResolver;

impl MakedependsResolver for NoopResolver {
    fn check_missing(&self, _deps: &[&str]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn install(&self, _deps: &[String]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn cleanup(&self, _installed: &[String]) -> Result<()> {
        Ok(())
    }
}

/// Result of makedepends resolution
#[derive(Debug, Default, Clone)]
pub struct MakedependsResult {
    /// Packages that were already installed
    pub already_installed: Vec<String>,
    /// Packages that were installed for this build
    pub newly_installed: Vec<String>,
    /// Packages that could not be resolved
    pub unresolved: Vec<String>,
}

/// Configuration for a specific bootstrap stage
///
/// This specifies the sysroot, toolchain paths, and environment
/// for a particular bootstrap stage.
#[derive(Debug, Clone)]
pub struct StageConfig {
    /// The bootstrap stage this config is for
    pub stage: super::format::BuildStage,
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
    pub fn new(stage: super::format::BuildStage, sysroot: PathBuf) -> Self {
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
    stages: std::collections::HashMap<super::format::BuildStage, StageConfig>,
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
    pub fn get(&self, stage: super::format::BuildStage) -> Option<&StageConfig> {
        self.stages.get(&stage)
    }

    /// Check if a stage is registered
    pub fn has_stage(&self, stage: super::format::BuildStage) -> bool {
        self.stages.contains_key(&stage)
    }

    /// Create a typical bootstrap registry with standard stage paths
    ///
    /// This sets up stages under a base directory:
    /// - stage0: `/base/stage0` (cross-tools from host)
    /// - stage1: `/base/stage1` (native but using stage0 tools)
    /// - stage2: `/base/stage2` (fully self-hosted)
    pub fn bootstrap_standard(base: &Path, target: &str) -> Self {
        use super::format::BuildStage;

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
            use_isolation: false, // Off by default, requires root or user namespaces
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
    pub makedepends: Option<MakedependsResult>,
    /// Whether this result came from cache
    pub from_cache: bool,
    /// Cache key used (if caching was enabled)
    pub cache_key: Option<String>,
}

/// The Kitchen: where recipes are cooked
pub struct Kitchen {
    config: KitchenConfig,
    /// Optional resolver for makedepends
    resolver: Option<Arc<dyn MakedependsResolver>>,
}

impl Kitchen {
    /// Create a new Kitchen with the given configuration
    pub fn new(config: KitchenConfig) -> Self {
        Self {
            config,
            resolver: None,
        }
    }

    /// Create a new Kitchen with a makedepends resolver
    pub fn with_resolver(config: KitchenConfig, resolver: Arc<dyn MakedependsResolver>) -> Self {
        Self {
            config,
            resolver: Some(resolver),
        }
    }

    /// Create a Kitchen with default configuration
    pub fn with_defaults() -> Self {
        Self::new(KitchenConfig::default())
    }

    /// Set the makedepends resolver
    pub fn set_resolver(&mut self, resolver: Arc<dyn MakedependsResolver>) {
        self.resolver = Some(resolver);
    }

    /// Resolve makedepends for a recipe
    ///
    /// Checks which makedepends are missing and installs them if a resolver
    /// is configured and auto_makedepends is enabled.
    ///
    /// Returns the resolution result with lists of installed and missing packages.
    pub fn resolve_makedepends(&self, recipe: &Recipe) -> Result<MakedependsResult> {
        let makedepends: Vec<&str> = recipe.build.makedepends.iter().map(|s| s.as_str()).collect();

        if makedepends.is_empty() {
            debug!("No makedepends specified in recipe");
            return Ok(MakedependsResult::default());
        }

        info!(
            "Checking makedepends: {}",
            makedepends.join(", ")
        );

        let resolver = match &self.resolver {
            Some(r) => r,
            None => {
                // No resolver configured - just return with all deps as "already installed"
                // (the caller is expected to have ensured they're available)
                debug!("No makedepends resolver configured, assuming all deps are available");
                return Ok(MakedependsResult {
                    already_installed: makedepends.iter().map(|s| s.to_string()).collect(),
                    newly_installed: Vec::new(),
                    unresolved: Vec::new(),
                });
            }
        };

        // Check which deps are missing
        let missing = resolver.check_missing(&makedepends)?;

        if missing.is_empty() {
            info!("All makedepends are already installed");
            return Ok(MakedependsResult {
                already_installed: makedepends.iter().map(|s| s.to_string()).collect(),
                newly_installed: Vec::new(),
                unresolved: Vec::new(),
            });
        }

        // Determine which are already installed
        let missing_set: std::collections::HashSet<&str> =
            missing.iter().map(|s| s.as_str()).collect();
        let already_installed: Vec<String> = makedepends
            .iter()
            .filter(|d| !missing_set.contains(*d))
            .map(|s| s.to_string())
            .collect();

        info!(
            "Installing missing makedepends: {}",
            missing.join(", ")
        );

        // Install missing deps
        let installed = resolver.install(&missing)?;

        // Any that weren't installed are unresolved
        let installed_set: std::collections::HashSet<&str> =
            installed.iter().map(|s| s.as_str()).collect();
        let unresolved: Vec<String> = missing
            .iter()
            .filter(|d| !installed_set.contains(d.as_str()))
            .cloned()
            .collect();

        if !unresolved.is_empty() {
            warn!(
                "Could not resolve makedepends: {}",
                unresolved.join(", ")
            );
        }

        Ok(MakedependsResult {
            already_installed,
            newly_installed: installed,
            unresolved,
        })
    }

    /// Clean up makedepends that were installed for a build
    fn cleanup_makedepends(&self, result: &MakedependsResult) -> Result<()> {
        if result.newly_installed.is_empty() {
            return Ok(());
        }

        let resolver = match &self.resolver {
            Some(r) => r,
            None => return Ok(()),
        };

        info!(
            "Cleaning up makedepends: {}",
            result.newly_installed.join(", ")
        );

        resolver.cleanup(&result.newly_installed)
    }

    /// Cook a recipe and produce a CCS package
    ///
    /// This is the main entry point for building from source.
    ///
    /// The cooking process follows these phases:
    /// 1. **Makedepends**: Resolve and install build dependencies (if enabled)
    /// 2. **Prep**: Fetch source archives and patches
    /// 3. **Unpack**: Extract sources and apply patches
    /// 4. **Simmer**: Run configure/make/install
    /// 5. **Plate**: Package result as CCS
    /// 6. **Cleanup**: Remove temporarily installed makedepends (if enabled)
    pub fn cook(&self, recipe: &Recipe, output_dir: &Path) -> Result<CookResult> {
        info!(
            "Cooking {} version {}",
            recipe.package.name, recipe.package.version
        );

        // Phase 0: Resolve makedepends (if enabled)
        let makedepends_result = if self.config.auto_makedepends {
            info!("Resolving makedepends...");
            let result = self.resolve_makedepends(recipe)?;

            // Fail if there are unresolved dependencies
            if !result.unresolved.is_empty() {
                return Err(Error::ResolutionError(format!(
                    "Unresolved makedepends: {}",
                    result.unresolved.join(", ")
                )));
            }

            Some(result)
        } else {
            None
        };

        // Wrap the build in a closure so we can ensure cleanup happens
        let build_result = (|| {
            let mut cook = Cook::new(self, recipe)?;

            // Phase 1: Prep - fetch ingredients
            info!("Prep: fetching ingredients...");
            cook.prep()?;

            // Phase 2: Unpack and patch
            info!("Unpacking and patching sources...");
            cook.unpack()?;
            cook.patch()?;

            // Phase 3: Simmer - run the build
            info!("Simmering: running build...");
            cook.simmer()?;

            // Phase 4: Plate - package the result
            info!("Plating: creating CCS package...");
            let package_path = cook.plate(output_dir)?;

            Ok(CookResult {
                package_path,
                log: cook.log,
                warnings: cook.warnings,
                makedepends: makedepends_result.clone(),
                from_cache: false,
                cache_key: None,
            })
        })();

        // Phase 5: Cleanup makedepends (if enabled and configured)
        if self.config.cleanup_makedepends
            && let Some(ref result) = makedepends_result
            && let Err(e) = self.cleanup_makedepends(result)
        {
            warn!("Failed to clean up makedepends: {}", e);
            // Don't fail the build just because cleanup failed
        }

        build_result
    }

    /// Cook a recipe with build artifact caching
    ///
    /// This checks the cache first before building. If a cached artifact exists
    /// with matching recipe and toolchain hash, it's used directly.
    ///
    /// The caching is based on:
    /// - Recipe content (name, version, sources, patches, build config)
    /// - Toolchain info (compiler version, target, stage)
    ///
    /// # Arguments
    ///
    /// * `recipe` - The recipe to cook
    /// * `output_dir` - Where to place the final CCS package
    /// * `cache` - The build cache to use
    /// * `toolchain` - Information about the current toolchain
    pub fn cook_cached(
        &self,
        recipe: &Recipe,
        output_dir: &Path,
        cache: &BuildCache,
        toolchain: &ToolchainInfo,
    ) -> Result<CookResult> {
        let cache_key = cache.cache_key(recipe, toolchain);

        // Check cache first
        if let Some(entry) = cache.get_by_key(&cache_key)? {
            info!(
                "Using cached build for {}-{} (key: {})",
                recipe.package.name,
                recipe.package.version,
                &cache_key[..16]
            );

            // Copy cached package to output dir
            let output_name = format!(
                "{}-{}-{}.ccs",
                recipe.package.name, recipe.package.version, recipe.package.release
            );
            let output_path = output_dir.join(&output_name);
            cache.copy_to(&entry, &output_path)?;

            return Ok(CookResult {
                package_path: output_path,
                log: format!("Cache hit: {}", entry.cache_key),
                warnings: Vec::new(),
                makedepends: None,
                from_cache: true,
                cache_key: Some(cache_key),
            });
        }

        debug!(
            "Cache miss for {}-{}, building from source",
            recipe.package.name, recipe.package.version
        );

        // No cache hit, do a full build
        let mut result = self.cook(recipe, output_dir)?;

        // Store in cache for next time
        match cache.put(recipe, toolchain, &result.package_path) {
            Ok(entry) => {
                result.cache_key = Some(entry.cache_key);
                info!(
                    "Cached build artifact for {}-{}",
                    recipe.package.name, recipe.package.version
                );
            }
            Err(e) => {
                warn!("Failed to cache build artifact: {}", e);
                // Don't fail the build just because caching failed
            }
        }

        Ok(result)
    }

    /// Cook multiple recipes in order with caching
    ///
    /// This is useful for cooking a dependency chain where later recipes
    /// may depend on earlier ones. Uses the cache to avoid rebuilding
    /// unchanged packages.
    pub fn cook_batch(
        &self,
        recipes: &[&Recipe],
        output_dir: &Path,
        cache: &BuildCache,
        toolchain: &ToolchainInfo,
    ) -> Result<Vec<CookResult>> {
        let mut results = Vec::with_capacity(recipes.len());

        for recipe in recipes {
            info!(
                "Cooking {}/{}: {}-{}",
                results.len() + 1,
                recipes.len(),
                recipe.package.name,
                recipe.package.version
            );

            let result = self.cook_cached(recipe, output_dir, cache, toolchain)?;

            if result.from_cache {
                info!("  -> cache hit");
            } else {
                info!("  -> built from source");
            }

            results.push(result);
        }

        Ok(results)
    }

    /// Fetch a source archive (with caching)
    fn fetch_source(&self, url: &str, checksum: &str) -> Result<PathBuf> {
        // Create cache directory if needed
        fs::create_dir_all(&self.config.source_cache)?;

        // Use checksum as cache key
        let cache_key = checksum.replace(':', "_");
        let cached_path = self.config.source_cache.join(&cache_key);

        // Check if already cached
        if cached_path.exists() {
            debug!("Using cached source: {}", cached_path.display());
            // Verify checksum
            if verify_file_checksum(&cached_path, checksum)? {
                return Ok(cached_path);
            }
            warn!("Cached file checksum mismatch, re-downloading");
            fs::remove_file(&cached_path)?;
        }

        // Download the source
        info!("Downloading: {}", url);
        let temp_path = self.config.source_cache.join(format!("{}.tmp", cache_key));

        download_file(url, &temp_path)?;

        // Verify checksum
        if !verify_file_checksum(&temp_path, checksum)? {
            fs::remove_file(&temp_path)?;
            return Err(Error::ChecksumMismatch {
                expected: checksum.to_string(),
                actual: "mismatch".to_string(),
            });
        }

        // Move to final location
        fs::rename(&temp_path, &cached_path)?;
        Ok(cached_path)
    }
}

/// A single cook operation
pub struct Cook<'a> {
    kitchen: &'a Kitchen,
    recipe: &'a Recipe,
    /// Temporary build directory
    build_dir: TempDir,
    /// Source directory within build_dir
    source_dir: PathBuf,
    /// Destination directory (where files get installed)
    dest_dir: PathBuf,
    /// Build log accumulator
    log: String,
    /// Warnings
    warnings: Vec<String>,
}

impl<'a> Cook<'a> {
    fn new(kitchen: &'a Kitchen, recipe: &'a Recipe) -> Result<Self> {
        let build_dir = TempDir::new()
            .map_err(|e| Error::IoError(format!("Failed to create build directory: {}", e)))?;

        let source_dir = build_dir.path().join("source");
        let dest_dir = build_dir.path().join("destdir");

        fs::create_dir_all(&source_dir)?;
        fs::create_dir_all(&dest_dir)?;

        Ok(Self {
            kitchen,
            recipe,
            build_dir,
            source_dir,
            dest_dir,
            log: String::new(),
            warnings: Vec::new(),
        })
    }

    /// Phase 1: Prep - fetch all sources
    fn prep(&mut self) -> Result<()> {
        // Fetch main source archive
        let archive_url = self.recipe.archive_url();
        let archive_path = self.kitchen.fetch_source(&archive_url, &self.recipe.source.checksum)?;

        // Copy to build directory
        let local_archive = self.build_dir.path().join(self.recipe.archive_filename());
        fs::copy(&archive_path, &local_archive)?;

        self.log_line(&format!("Fetched source: {}", archive_url));

        // Fetch additional sources
        for additional in &self.recipe.source.additional {
            let path = self.kitchen.fetch_source(&additional.url, &additional.checksum)?;
            let filename = additional
                .url
                .split('/')
                .last()
                .unwrap_or("additional.tar.gz");
            let local_path = self.build_dir.path().join(filename);
            fs::copy(&path, &local_path)?;
            self.log_line(&format!("Fetched additional source: {}", additional.url));
        }

        // Fetch patches
        if let Some(patches) = &self.recipe.patches {
            for patch in &patches.files {
                if patch.file.starts_with("http://") || patch.file.starts_with("https://") {
                    let checksum = patch.checksum.as_deref().unwrap_or("sha256:0");
                    let path = self.kitchen.fetch_source(&patch.file, checksum)?;
                    let filename = patch.file.split('/').last().unwrap_or("patch.diff");
                    let local_path = self.build_dir.path().join("patches").join(filename);
                    fs::create_dir_all(local_path.parent().unwrap())?;
                    fs::copy(&path, &local_path)?;
                    self.log_line(&format!("Fetched patch: {}", patch.file));
                }
            }
        }

        Ok(())
    }

    /// Phase 2a: Unpack sources
    fn unpack(&mut self) -> Result<()> {
        let archive_path = self.build_dir.path().join(self.recipe.archive_filename());

        // Detect archive type and extract
        extract_archive(&archive_path, &self.source_dir)?;
        self.log_line(&format!(
            "Extracted source to {}",
            self.source_dir.display()
        ));

        // Find the actual source directory (often archives have a top-level dir)
        let entries: Vec<_> = fs::read_dir(&self.source_dir)?
            .filter_map(|e| e.ok())
            .collect();

        if entries.len() == 1 && entries[0].file_type().map(|t| t.is_dir()).unwrap_or(false) {
            // Single directory - this is the actual source
            self.source_dir = entries[0].path();
            debug!("Source directory: {}", self.source_dir.display());
        }

        // Override with explicit extract_dir if specified
        if let Some(extract_dir) = &self.recipe.source.extract_dir {
            self.source_dir = self.build_dir.path().join("source").join(extract_dir);
        }

        Ok(())
    }

    /// Phase 2b: Apply patches
    fn patch(&mut self) -> Result<()> {
        let patches = match &self.recipe.patches {
            Some(p) => &p.files,
            None => return Ok(()),
        };

        for patch_info in patches {
            let patch_path = if patch_info.file.starts_with("http://")
                || patch_info.file.starts_with("https://")
            {
                let filename = patch_info.file.split('/').last().unwrap_or("patch.diff");
                self.build_dir.path().join("patches").join(filename)
            } else {
                PathBuf::from(&patch_info.file)
            };

            if !patch_path.exists() {
                return Err(Error::NotFound(format!(
                    "Patch file not found: {}",
                    patch_path.display()
                )));
            }

            info!("Applying patch: {}", patch_info.file);
            apply_patch(&self.source_dir, &patch_path, patch_info.strip)?;
            self.log_line(&format!("Applied patch: {}", patch_info.file));
        }

        Ok(())
    }

    /// Phase 3: Simmer - run the build
    fn simmer(&mut self) -> Result<()> {
        let build = &self.recipe.build;

        // Determine working directory
        let workdir = if let Some(wd) = &build.workdir {
            self.source_dir.join(wd)
        } else {
            self.source_dir.clone()
        };

        // Set up environment
        let mut env: Vec<(&str, String)> = vec![
            ("DESTDIR", self.dest_dir.to_string_lossy().to_string()),
            (
                "MAKEFLAGS",
                format!("-j{}", build.jobs.unwrap_or(self.kitchen.config.jobs)),
            ),
        ];

        for (key, value) in &build.environment {
            env.push((key, value.clone()));
        }

        // Run setup if specified
        if let Some(setup) = &build.setup {
            self.run_build_step("setup", setup, &workdir, &env)?;
        }

        // Run configure
        if let Some(configure) = &build.configure {
            let cmd = self.recipe.substitute(configure, &self.dest_dir.to_string_lossy());
            self.run_build_step("configure", &cmd, &workdir, &env)?;
        }

        // Run make
        if let Some(make) = &build.make {
            let cmd = self.recipe.substitute(make, &self.dest_dir.to_string_lossy());
            self.run_build_step("make", &cmd, &workdir, &env)?;
        }

        // Run check if specified
        if let Some(check) = &build.check {
            match self.run_build_step("check", check, &workdir, &env) {
                Ok(_) => {}
                Err(e) => {
                    self.warnings.push(format!("Tests failed: {}", e));
                }
            }
        }

        // Run install
        if let Some(install) = &build.install {
            let cmd = self.recipe.substitute(install, &self.dest_dir.to_string_lossy());
            self.run_build_step("install", &cmd, &workdir, &env)?;
        }

        // Run post_install if specified
        if let Some(post_install) = &build.post_install {
            self.run_build_step("post_install", post_install, &workdir, &env)?;
        }

        Ok(())
    }

    /// Run a build step
    fn run_build_step(
        &mut self,
        phase: &str,
        command: &str,
        workdir: &Path,
        env: &[(&str, String)],
    ) -> Result<()> {
        info!("Running {} phase", phase);
        debug!("Command: {}", command);

        if self.kitchen.config.use_isolation {
            self.run_build_step_isolated(phase, command, workdir, env)
        } else {
            self.run_build_step_direct(phase, command, workdir, env)
        }
    }

    /// Run a build step with container isolation
    fn run_build_step_isolated(
        &mut self,
        phase: &str,
        command: &str,
        workdir: &Path,
        env: &[(&str, String)],
    ) -> Result<()> {
        // Configure container based on pristine mode
        let mut container_config = if self.kitchen.config.pristine_mode {
            // Pristine mode: no host system mounts
            // This is critical for bootstrap builds to avoid toolchain contamination
            let config = if let Some(sysroot) = &self.kitchen.config.sysroot {
                ContainerConfig::pristine_for_bootstrap(
                    sysroot,
                    &self.source_dir,
                    self.build_dir.path(),
                    &self.dest_dir,
                )
            } else {
                ContainerConfig::pristine()
            };
            info!(
                "Using pristine container (no host mounts) for {} phase",
                phase
            );
            config
        } else {
            // Normal mode: mount host system directories
            ContainerConfig::default()
        };

        // Set resource limits from kitchen config
        container_config.memory_limit = self.kitchen.config.memory_limit;
        container_config.cpu_time_limit = self.kitchen.config.cpu_time_limit;
        container_config.timeout = self.kitchen.config.timeout;
        container_config.hostname = "conary-build".to_string();
        container_config.workdir = workdir.to_path_buf();

        // For non-pristine mode, set up bind mounts manually
        if !self.kitchen.config.pristine_mode {
            // Clear default mounts and add build-specific ones
            container_config.bind_mounts.clear();

            // Essential system directories (read-only)
            for path in &["/usr", "/lib", "/lib64", "/bin", "/sbin"] {
                if Path::new(path).exists() {
                    container_config
                        .bind_mounts
                        .push(BindMount::readonly(*path, *path));
                }
            }

            // Config files that build tools might need
            for path in &["/etc/passwd", "/etc/group", "/etc/hosts", "/etc/resolv.conf"] {
                if Path::new(path).exists() {
                    container_config
                        .bind_mounts
                        .push(BindMount::readonly(*path, *path));
                }
            }

            // Source directory (read-only - we shouldn't modify sources)
            container_config
                .bind_mounts
                .push(BindMount::readonly(&self.source_dir, &self.source_dir));

            // Destination directory (writable - where install goes)
            container_config
                .bind_mounts
                .push(BindMount::writable(&self.dest_dir, &self.dest_dir));

            // Build directory (writable - for build artifacts)
            container_config
                .bind_mounts
                .push(BindMount::writable(self.build_dir.path(), self.build_dir.path()));
        }

        let mut sandbox = Sandbox::new(container_config);

        // Convert env to the format expected by Sandbox
        let env_refs: Vec<(&str, &str)> = env.iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();

        let (exit_code, stdout, stderr) = sandbox.execute(
            "/bin/sh",
            &format!("cd {} && {}", workdir.display(), command),
            &[],
            &env_refs,
        )?;

        self.log_line(&format!("=== {} (isolated) ===", phase));
        if !stdout.is_empty() {
            self.log.push_str(&stdout);
            self.log.push('\n');
        }
        if !stderr.is_empty() {
            self.log.push_str(&stderr);
            self.log.push('\n');
        }

        if exit_code != 0 {
            return Err(Error::IoError(format!(
                "{} phase failed with exit code {}\nstderr: {}",
                phase, exit_code, stderr
            )));
        }

        Ok(())
    }

    /// Run a build step directly (no isolation)
    fn run_build_step_direct(
        &mut self,
        phase: &str,
        command: &str,
        workdir: &Path,
        env: &[(&str, String)],
    ) -> Result<()> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(workdir)
            .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
            .output()
            .map_err(|e| Error::IoError(format!("Failed to run {} phase: {}", phase, e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        self.log_line(&format!("=== {} ===", phase));
        if !stdout.is_empty() {
            self.log.push_str(&stdout);
            self.log.push('\n');
        }
        if !stderr.is_empty() {
            self.log.push_str(&stderr);
            self.log.push('\n');
        }

        if !output.status.success() {
            return Err(Error::IoError(format!(
                "{} phase failed with exit code {:?}\nstderr: {}",
                phase,
                output.status.code(),
                stderr
            )));
        }

        Ok(())
    }

    /// Phase 4: Plate - package the result as CCS
    fn plate(&mut self, output_dir: &Path) -> Result<PathBuf> {
        // Check that destdir has files
        if fs::read_dir(&self.dest_dir)?.count() == 0 {
            return Err(Error::IoError(
                "No files installed to destdir - install phase may have failed".to_string(),
            ));
        }

        // Create CCS manifest from recipe metadata
        let mut manifest = CcsManifest::new_minimal(
            &self.recipe.package.name,
            &self.recipe.package.version,
        );

        // Copy over additional metadata from recipe
        if let Some(desc) = &self.recipe.package.description {
            manifest.package.description = desc.clone();
        } else if let Some(summary) = &self.recipe.package.summary {
            manifest.package.description = summary.clone();
        }
        manifest.package.license = self.recipe.package.license.clone();
        manifest.package.homepage = self.recipe.package.homepage.clone();

        // Add build dependencies as requires (for reference)
        for dep in &self.recipe.build.requires {
            manifest.requires.packages.push(PackageDep {
                name: dep.clone(),
                version: None,
            });
        }

        // Build CCS package from destdir
        let builder = CcsBuilder::new(manifest, &self.dest_dir);
        let build_result = builder
            .build()
            .map_err(|e| Error::IoError(format!("CCS build failed: {e}")))?;

        // Write CCS package
        let package_name = format!(
            "{}-{}-{}.ccs",
            self.recipe.package.name, self.recipe.package.version, self.recipe.package.release
        );
        let package_path = output_dir.join(&package_name);

        write_ccs_package(&build_result, &package_path)
            .map_err(|e| Error::IoError(format!("Failed to write CCS package: {e}")))?;

        self.log_line(&format!(
            "Created CCS package: {} ({} files, {} blobs)",
            package_path.display(),
            build_result.files.len(),
            build_result.blobs.len()
        ));
        info!(
            "Cooked: {} ({} files)",
            package_path.display(),
            build_result.files.len()
        );

        Ok(package_path)
    }

    fn log_line(&mut self, line: &str) {
        self.log.push_str(line);
        self.log.push('\n');
    }
}

/// Download a file from a URL
fn download_file(url: &str, dest: &Path) -> Result<()> {
    // Use curl for now (could use reqwest later)
    let output = Command::new("curl")
        .args(["-fsSL", "-o", dest.to_str().unwrap(), url])
        .output()
        .map_err(|e| Error::DownloadError(format!("curl failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::DownloadError(format!(
            "Failed to download {}: {}",
            url,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

/// Verify file checksum
fn verify_file_checksum(path: &Path, expected: &str) -> Result<bool> {
    let content = fs::read(path)?;

    let (algorithm, expected_hash) = expected
        .split_once(':')
        .ok_or_else(|| Error::ParseError("Invalid checksum format".to_string()))?;

    let algo = match algorithm {
        "sha256" => HashAlgorithm::Sha256,
        "xxh128" => HashAlgorithm::Xxh128,
        _ => {
            return Err(Error::ParseError(format!(
                "Unsupported checksum algorithm: {} (supported: sha256, xxh128)",
                algorithm
            )))
        }
    };

    let actual = hash_bytes(algo, &content);
    Ok(actual.as_str() == expected_hash)
}

/// Extract an archive
fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    let filename = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let args: Vec<&str> = if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        vec!["-xzf", archive.to_str().unwrap(), "-C", dest.to_str().unwrap()]
    } else if filename.ends_with(".tar.xz") || filename.ends_with(".txz") {
        vec!["-xJf", archive.to_str().unwrap(), "-C", dest.to_str().unwrap()]
    } else if filename.ends_with(".tar.bz2") || filename.ends_with(".tbz2") {
        vec!["-xjf", archive.to_str().unwrap(), "-C", dest.to_str().unwrap()]
    } else if filename.ends_with(".tar.zst") {
        vec!["--zstd", "-xf", archive.to_str().unwrap(), "-C", dest.to_str().unwrap()]
    } else if filename.ends_with(".tar") {
        vec!["-xf", archive.to_str().unwrap(), "-C", dest.to_str().unwrap()]
    } else {
        return Err(Error::ParseError(format!(
            "Unknown archive format: {}",
            filename
        )));
    };

    let output = Command::new("tar")
        .args(&args)
        .output()
        .map_err(|e| Error::IoError(format!("tar failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::IoError(format!(
            "Failed to extract archive: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

/// Apply a patch to the source
fn apply_patch(source_dir: &Path, patch_path: &Path, strip: u32) -> Result<()> {
    let output = Command::new("patch")
        .args(["-p", &strip.to_string(), "-i", patch_path.to_str().unwrap()])
        .current_dir(source_dir)
        .output()
        .map_err(|e| Error::IoError(format!("patch failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::IoError(format!(
            "Failed to apply patch: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

    #[test]
    fn test_kitchen_config_default() {
        let config = KitchenConfig::default();
        assert!(config.jobs > 0);
        assert!(!config.allow_network);
        assert!(!config.keep_builddir);
        assert!(!config.auto_makedepends);
        assert!(config.cleanup_makedepends);
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

    #[test]
    fn test_verify_checksum_format() {
        // This test would need actual file content
        // Just testing the format parsing
        let result = verify_file_checksum(Path::new("/nonexistent"), "invalid");
        assert!(result.is_err());
    }

    /// A mock resolver for testing makedepends resolution
    struct MockResolver {
        installed: Mutex<HashSet<String>>,
        install_calls: Mutex<Vec<Vec<String>>>,
        cleanup_calls: Mutex<Vec<Vec<String>>>,
    }

    impl MockResolver {
        fn new(initially_installed: &[&str]) -> Self {
            Self {
                installed: Mutex::new(
                    initially_installed.iter().map(|s| s.to_string()).collect(),
                ),
                install_calls: Mutex::new(Vec::new()),
                cleanup_calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl MakedependsResolver for MockResolver {
        fn check_missing(&self, deps: &[&str]) -> Result<Vec<String>> {
            let installed = self.installed.lock().unwrap();
            Ok(deps
                .iter()
                .filter(|d| !installed.contains(&d.to_string()))
                .map(|s| s.to_string())
                .collect())
        }

        fn install(&self, deps: &[String]) -> Result<Vec<String>> {
            self.install_calls.lock().unwrap().push(deps.to_vec());
            let mut installed = self.installed.lock().unwrap();
            for dep in deps {
                installed.insert(dep.clone());
            }
            Ok(deps.to_vec())
        }

        fn cleanup(&self, deps: &[String]) -> Result<()> {
            self.cleanup_calls.lock().unwrap().push(deps.to_vec());
            let mut installed = self.installed.lock().unwrap();
            for dep in deps {
                installed.remove(dep);
            }
            Ok(())
        }
    }

    fn make_test_recipe(makedepends: &[&str]) -> Recipe {
        Recipe {
            package: crate::recipe::format::PackageSection {
                name: "test".to_string(),
                version: "1.0".to_string(),
                release: "1".to_string(),
                summary: None,
                description: None,
                license: None,
                homepage: None,
            },
            source: crate::recipe::format::SourceSection {
                archive: "https://example.com/test.tar.gz".to_string(),
                checksum: "sha256:abc".to_string(),
                signature: None,
                additional: Vec::new(),
                extract_dir: None,
            },
            build: crate::recipe::format::BuildSection {
                requires: Vec::new(),
                makedepends: makedepends.iter().map(|s| s.to_string()).collect(),
                configure: None,
                make: None,
                install: None,
                check: None,
                setup: None,
                post_install: None,
                environment: std::collections::HashMap::new(),
                workdir: None,
                script_file: None,
                jobs: None,
            },
            cross: None,
            patches: None,
            components: None,
            variables: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn test_resolve_makedepends_empty() {
        let kitchen = Kitchen::with_defaults();
        let recipe = make_test_recipe(&[]);

        let result = kitchen.resolve_makedepends(&recipe).unwrap();
        assert!(result.already_installed.is_empty());
        assert!(result.newly_installed.is_empty());
        assert!(result.unresolved.is_empty());
    }

    #[test]
    fn test_resolve_makedepends_no_resolver() {
        let kitchen = Kitchen::with_defaults();
        let recipe = make_test_recipe(&["gcc", "make"]);

        let result = kitchen.resolve_makedepends(&recipe).unwrap();
        // Without resolver, assumes all are installed
        assert_eq!(result.already_installed, vec!["gcc", "make"]);
        assert!(result.newly_installed.is_empty());
        assert!(result.unresolved.is_empty());
    }

    #[test]
    fn test_resolve_makedepends_all_installed() {
        let resolver = Arc::new(MockResolver::new(&["gcc", "make", "cmake"]));
        let kitchen = Kitchen::with_resolver(KitchenConfig::default(), resolver.clone());
        let recipe = make_test_recipe(&["gcc", "make"]);

        let result = kitchen.resolve_makedepends(&recipe).unwrap();
        assert_eq!(result.already_installed.len(), 2);
        assert!(result.newly_installed.is_empty());
        assert!(result.unresolved.is_empty());
        // Should not have called install
        assert!(resolver.install_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn test_resolve_makedepends_some_missing() {
        let resolver = Arc::new(MockResolver::new(&["gcc"]));
        let kitchen = Kitchen::with_resolver(KitchenConfig::default(), resolver.clone());
        let recipe = make_test_recipe(&["gcc", "make", "cmake"]);

        let result = kitchen.resolve_makedepends(&recipe).unwrap();
        assert_eq!(result.already_installed, vec!["gcc"]);
        assert!(result.newly_installed.contains(&"make".to_string()));
        assert!(result.newly_installed.contains(&"cmake".to_string()));
        assert!(result.unresolved.is_empty());
        // Should have called install with missing deps
        let install_calls = resolver.install_calls.lock().unwrap();
        assert_eq!(install_calls.len(), 1);
        assert!(install_calls[0].contains(&"make".to_string()));
        assert!(install_calls[0].contains(&"cmake".to_string()));
    }

    #[test]
    fn test_resolve_makedepends_all_missing() {
        let resolver = Arc::new(MockResolver::new(&[]));
        let kitchen = Kitchen::with_resolver(KitchenConfig::default(), resolver.clone());
        let recipe = make_test_recipe(&["gcc", "make"]);

        let result = kitchen.resolve_makedepends(&recipe).unwrap();
        assert!(result.already_installed.is_empty());
        assert_eq!(result.newly_installed.len(), 2);
        assert!(result.unresolved.is_empty());
    }

    #[test]
    fn test_cleanup_makedepends() {
        let resolver = Arc::new(MockResolver::new(&[]));
        let kitchen = Kitchen::with_resolver(KitchenConfig::default(), resolver.clone());

        let result = MakedependsResult {
            already_installed: vec!["existing".to_string()],
            newly_installed: vec!["new1".to_string(), "new2".to_string()],
            unresolved: Vec::new(),
        };

        kitchen.cleanup_makedepends(&result).unwrap();

        // Should have called cleanup with newly_installed only
        let cleanup_calls = resolver.cleanup_calls.lock().unwrap();
        assert_eq!(cleanup_calls.len(), 1);
        assert!(cleanup_calls[0].contains(&"new1".to_string()));
        assert!(cleanup_calls[0].contains(&"new2".to_string()));
        assert!(!cleanup_calls[0].contains(&"existing".to_string()));
    }

    #[test]
    fn test_cleanup_makedepends_empty() {
        let resolver = Arc::new(MockResolver::new(&[]));
        let kitchen = Kitchen::with_resolver(KitchenConfig::default(), resolver.clone());

        let result = MakedependsResult {
            already_installed: vec!["existing".to_string()],
            newly_installed: Vec::new(), // Nothing to clean up
            unresolved: Vec::new(),
        };

        kitchen.cleanup_makedepends(&result).unwrap();

        // Should not have called cleanup
        assert!(resolver.cleanup_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn test_noop_resolver() {
        let noop = NoopResolver;

        // check_missing always returns empty
        let missing = noop.check_missing(&["gcc", "make"]).unwrap();
        assert!(missing.is_empty());

        // install returns empty (nothing installed)
        let installed = noop.install(&["gcc".to_string()]).unwrap();
        assert!(installed.is_empty());

        // cleanup does nothing
        noop.cleanup(&["gcc".to_string()]).unwrap();
    }

    #[test]
    fn test_makedepends_result_default() {
        let result = MakedependsResult::default();
        assert!(result.already_installed.is_empty());
        assert!(result.newly_installed.is_empty());
        assert!(result.unresolved.is_empty());
    }

    #[test]
    fn test_makedepends_result_clone() {
        let result = MakedependsResult {
            already_installed: vec!["a".to_string()],
            newly_installed: vec!["b".to_string()],
            unresolved: vec!["c".to_string()],
        };

        let cloned = result.clone();
        assert_eq!(cloned.already_installed, result.already_installed);
        assert_eq!(cloned.newly_installed, result.newly_installed);
        assert_eq!(cloned.unresolved, result.unresolved);
    }

    // Stage configuration tests

    #[test]
    fn test_stage_config_new() {
        use crate::recipe::format::BuildStage;

        let config = StageConfig::new(BuildStage::Stage0, PathBuf::from("/opt/stage0"));
        assert_eq!(config.stage, BuildStage::Stage0);
        assert_eq!(config.sysroot, PathBuf::from("/opt/stage0"));
        assert!(config.tools_dir.is_none());
        assert!(config.tool_prefix.is_none());
        assert!(config.target_triple.is_none());
    }

    #[test]
    fn test_stage_config_builder() {
        use crate::recipe::format::BuildStage;

        let config = StageConfig::new(BuildStage::Stage1, PathBuf::from("/opt/stage1"))
            .with_tools_dir(PathBuf::from("/opt/tools"))
            .with_tool_prefix("x86_64-linux-gnu".to_string())
            .with_target("x86_64-unknown-linux-gnu".to_string());

        assert_eq!(config.stage, BuildStage::Stage1);
        assert_eq!(config.tools_dir, Some(PathBuf::from("/opt/tools")));
        assert_eq!(config.tool_prefix, Some("x86_64-linux-gnu".to_string()));
        assert_eq!(config.target_triple, Some("x86_64-unknown-linux-gnu".to_string()));
    }

    #[test]
    fn test_stage_config_env_vars() {
        use crate::recipe::format::BuildStage;

        let config = StageConfig::new(BuildStage::Stage0, PathBuf::from("/opt/stage0"))
            .with_tools_dir(PathBuf::from("/opt/cross/bin"))
            .with_tool_prefix("aarch64-linux-gnu".to_string())
            .with_target("aarch64-unknown-linux-gnu".to_string());

        let env = config.env_vars();
        let env_map: std::collections::HashMap<_, _> = env.into_iter().collect();

        // Check sysroot
        assert_eq!(env_map.get("SYSROOT").unwrap(), "/opt/stage0");

        // Check tool paths
        assert_eq!(env_map.get("CC").unwrap(), "/opt/cross/bin/aarch64-linux-gnu-gcc");
        assert_eq!(env_map.get("CXX").unwrap(), "/opt/cross/bin/aarch64-linux-gnu-g++");
        assert_eq!(env_map.get("AR").unwrap(), "/opt/cross/bin/aarch64-linux-gnu-ar");

        // Check target
        assert_eq!(env_map.get("TARGET").unwrap(), "aarch64-unknown-linux-gnu");

        // Check cross_compile prefix
        assert_eq!(env_map.get("CROSS_COMPILE").unwrap(), "aarch64-linux-gnu-");

        // Check stage marker
        assert_eq!(env_map.get("CONARY_STAGE").unwrap(), "stage0");

        // Check sysroot in CFLAGS
        assert!(env_map.get("CFLAGS").unwrap().contains("--sysroot=/opt/stage0"));
    }

    #[test]
    fn test_stage_registry_new() {
        let registry = StageRegistry::new();
        assert!(!registry.has_stage(crate::recipe::format::BuildStage::Stage0));
    }

    #[test]
    fn test_stage_registry_register_and_get() {
        use crate::recipe::format::BuildStage;

        let mut registry = StageRegistry::new();
        let config = StageConfig::new(BuildStage::Stage0, PathBuf::from("/opt/stage0"));
        registry.register(config);

        assert!(registry.has_stage(BuildStage::Stage0));
        assert!(!registry.has_stage(BuildStage::Stage1));

        let retrieved = registry.get(BuildStage::Stage0).unwrap();
        assert_eq!(retrieved.sysroot, PathBuf::from("/opt/stage0"));
    }

    #[test]
    fn test_stage_registry_bootstrap_standard() {
        use crate::recipe::format::BuildStage;

        let registry = StageRegistry::bootstrap_standard(
            Path::new("/opt/bootstrap"),
            "x86_64-conary-linux-gnu"
        );

        // Check all stages exist
        assert!(registry.has_stage(BuildStage::Stage0));
        assert!(registry.has_stage(BuildStage::Stage1));
        assert!(registry.has_stage(BuildStage::Stage2));
        assert!(!registry.has_stage(BuildStage::Final));

        // Check stage0 config
        let stage0 = registry.get(BuildStage::Stage0).unwrap();
        assert_eq!(stage0.sysroot, PathBuf::from("/opt/bootstrap/stage0"));
        assert_eq!(stage0.tools_dir, Some(PathBuf::from("/opt/bootstrap/cross-tools/bin")));
        assert_eq!(stage0.tool_prefix, Some("x86_64-conary-linux-gnu".to_string()));

        // Check stage1 uses stage0 tools
        let stage1 = registry.get(BuildStage::Stage1).unwrap();
        assert_eq!(stage1.sysroot, PathBuf::from("/opt/bootstrap/stage1"));
        assert_eq!(stage1.tools_dir, Some(PathBuf::from("/opt/bootstrap/stage0/usr/bin")));

        // Check stage2 uses stage1 tools
        let stage2 = registry.get(BuildStage::Stage2).unwrap();
        assert_eq!(stage2.sysroot, PathBuf::from("/opt/bootstrap/stage2"));
        assert_eq!(stage2.tools_dir, Some(PathBuf::from("/opt/bootstrap/stage1/usr/bin")));
    }
}
