// conary-core/src/recipe/kitchen/mod.rs

//! Kitchen: the isolated build environment for cooking recipes
//!
//! The Kitchen provides a sandboxed environment for building packages
//! from source recipes. It handles:
//! - Fetching source archives and patches
//! - Extracting and patching sources
//! - Running build commands in isolation
//! - Packaging the result as CCS

pub(crate) mod archive;
mod config;
mod cook;
pub mod local_source;
pub mod makedepends;
pub mod provenance_capture;
mod reproducibility_env;

pub use config::{
    CookResult, KitchenConfig, SourceChecksumPolicy, SourceDownloadPolicy, StageConfig,
    StageRegistry,
};
pub use cook::Cook;
pub use makedepends::{MakedependsResolver, MakedependsResult, NoopResolver};
// Re-exported for external consumers (e.g., CLI tools that inspect provenance)
#[allow(unused_imports)]
pub use provenance_capture::{CapturedDep, CapturedPatch, ProvenanceCapture};

use crate::error::{Error, Result};
use crate::recipe::cache::{BuildCache, ToolchainInfo};
use crate::recipe::format::{LocalSourceSection, Recipe, SourceSection, is_remote_url};
use crate::recipe::hermetic::{CiMode, HermeticBuildInput, HermeticBuildPlan};
use archive::{download_file, verify_file_checksum};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Convert a checksum string into a source cache filename
///
/// Replaces ':' with '_' so "sha256:abc123" becomes "sha256_abc123"
fn source_cache_key(checksum: &str) -> String {
    checksum.replace(':', "_")
}

fn has_url_scheme(input: &str) -> bool {
    let Some(colon_index) = input.find(':') else {
        return false;
    };

    let scheme = &input[..colon_index];
    let mut bytes = scheme.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    first.is_ascii_alphabetic()
        && bytes.all(|byte| {
            matches!(
                byte,
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.'
            )
        })
}

/// The Kitchen: where recipes are cooked
pub struct Kitchen {
    pub(crate) config: KitchenConfig,
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

    fn with_config_preserving_resolver(&self, config: KitchenConfig) -> Self {
        Self {
            config,
            resolver: self.resolver.clone(),
        }
    }

    /// Resolve makedepends for a recipe
    ///
    /// Checks which makedepends are missing and installs them if a resolver
    /// is configured and auto_makedepends is enabled.
    ///
    /// Returns the resolution result with lists of installed and missing packages.
    pub fn resolve_makedepends(&self, recipe: &Recipe) -> Result<MakedependsResult> {
        let makedepends: Vec<&str> = recipe
            .build
            .makedepends
            .iter()
            .map(|s| s.as_str())
            .collect();

        if makedepends.is_empty() {
            debug!("No makedepends specified in recipe");
            return Ok(MakedependsResult::default());
        }

        info!("Checking makedepends: {}", makedepends.join(", "));

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

        info!("Installing missing makedepends: {}", missing.join(", "));

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
            warn!("Could not resolve makedepends: {}", unresolved.join(", "));
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
    /// ## Hermetic Build Architecture
    ///
    /// The cooking process is split into two distinct phases with different
    /// network access policies, following the BuildStream model:
    ///
    /// ### Fetch Phase (Network ALLOWED)
    /// - Download source archives
    /// - Download patches
    /// - Verify checksums
    /// - Cache sources locally
    ///
    /// ### Build Phase (Network BLOCKED)
    /// - Extract cached sources
    /// - Apply patches
    /// - Run configure/make/install
    /// - Package artifacts
    ///
    /// This separation ensures reproducible builds: if all sources are cached,
    /// the build phase cannot accidentally depend on network resources.
    ///
    /// ## Full Cooking Process
    /// 1. **Makedepends**: Resolve and install build dependencies (if enabled)
    /// 2. **Prep**: Fetch source archives and patches (with network)
    /// 3. **Unpack**: Extract sources and apply patches
    /// 4. **Simmer**: Run configure/make/install (network blocked)
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
            let (package_path, provenance) = cook.plate(output_dir)?;

            Ok(CookResult {
                package_path,
                log: cook.log,
                warnings: cook.warnings,
                makedepends: makedepends_result.clone(),
                from_cache: false,
                cache_key: None,
                provenance: Some(provenance),
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

    /// Cook a recipe through the M2a hermetic path.
    ///
    /// Sources are prefetched with the caller's Kitchen first, then the build
    /// runs through a cloned Kitchen whose config has hermetic evidence,
    /// reproducibility controls, pristine isolation, and offline source policy.
    pub fn cook_hermetic(
        &self,
        recipe: &Recipe,
        input: HermeticBuildInput,
        output_dir: &Path,
        ci_mode: CiMode,
    ) -> Result<CookResult> {
        let mut prefetch_config = self.config.clone();
        prefetch_config.recipe_source_base_dir = Some(input.recipe_source_base_dir.clone());
        self.with_config_preserving_resolver(prefetch_config)
            .fetch(recipe)?;
        let plan = HermeticBuildPlan::from_recipe(recipe, input, ci_mode)?;
        let mut build_config = self.config.clone();
        plan.apply_to_kitchen_config(&mut build_config);
        build_config.auto_makedepends = self.config.auto_makedepends;
        build_config.cleanup_makedepends = self.config.cleanup_makedepends;
        assert_hermetic_build_execution_boundary(&build_config)?;
        let kitchen = self.with_config_preserving_resolver(build_config);
        kitchen.cook(recipe, output_dir)
    }

    /// Fetch sources for a recipe without building
    ///
    /// Downloads and verifies all source archives and patches for a recipe,
    /// caching them locally. This is useful for:
    /// - Pre-fetching sources for offline builds
    /// - Warming source caches on build servers
    /// - Verifying source availability before building
    ///
    /// This method runs WITH network access (the "fetch phase" of hermetic builds).
    /// After sources are fetched, they can be built offline using `cook()`.
    ///
    /// # Returns
    /// A list of paths to the fetched and cached source files.
    pub fn fetch(&self, recipe: &Recipe) -> Result<Vec<PathBuf>> {
        info!(
            "Fetching sources for {} version {}",
            recipe.package.name, recipe.package.version
        );

        let mut fetched = Vec::new();

        match &recipe.source {
            SourceSection::Remote(source) => {
                // Fetch main source archive
                let archive_url = recipe.archive_url();
                info!("Fetching: {}", archive_url);
                let path = self.fetch_source(&archive_url, &source.checksum)?;
                fetched.push(path);

                // Fetch additional sources
                for additional in &source.additional {
                    info!("Fetching additional: {}", additional.url);
                    let path = self.fetch_source(&additional.url, &additional.checksum)?;
                    fetched.push(path);
                }
            }
            SourceSection::Local(source) => {
                let source_path = self.resolve_local_source(source)?;
                fetched.push(source_path);
            }
        }

        // Fetch remote patches
        if let Some(patches) = &recipe.patches {
            for patch in &patches.files {
                if is_remote_url(&patch.file) {
                    let checksum = patch.checksum.as_ref().ok_or_else(|| {
                        Error::ConfigError(format!(
                            "Remote patch '{}' has no checksum. \
                             All remote patches must include a sha256 checksum \
                             to prevent MITM or compromised-server attacks. \
                             Add a 'checksum' field to the patch entry in your recipe.",
                            patch.file
                        ))
                    })?;
                    info!("Fetching patch: {}", patch.file);
                    let path = self.fetch_source(&patch.file, checksum)?;
                    fetched.push(path);
                }
            }
        }

        info!(
            "Fetched {} source file(s) for {}",
            fetched.len(),
            recipe.package.name
        );

        Ok(fetched)
    }

    /// Check if all sources for a recipe are already cached
    ///
    /// Returns `true` if all source archives and patches are available locally,
    /// meaning the build can proceed without network access.
    pub fn sources_cached(&self, recipe: &Recipe) -> bool {
        match &recipe.source {
            SourceSection::Remote(source) => {
                // Check main archive
                let key = source_cache_key(&source.checksum);
                let cached_path = self.config.source_cache.join(&key);
                if !cached_path.exists() {
                    return false;
                }

                // Check additional sources
                for additional in &source.additional {
                    let key = source_cache_key(&additional.checksum);
                    let cached_path = self.config.source_cache.join(&key);
                    if !cached_path.exists() {
                        return false;
                    }
                }
            }
            SourceSection::Local(source) => {
                let Ok(source_path) = self.resolve_local_source(source) else {
                    return false;
                };
                if !source_path.is_dir() {
                    return false;
                }
            }
        }
        // Check remote patches
        if let Some(patches) = &recipe.patches {
            for patch in &patches.files {
                if is_remote_url(&patch.file)
                    && let Some(checksum) = &patch.checksum
                {
                    let key = source_cache_key(checksum);
                    let cached_path = self.config.source_cache.join(&key);
                    if !cached_path.exists() {
                        return false;
                    }
                }
            }
        }

        true
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
        let cache_key = cache.try_cache_key(recipe, toolchain)?;

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
                provenance: None, // Provenance not available from cache (yet)
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

    /// Create a Cook that installs to an external destination directory.
    ///
    /// Used by bootstrap phases where files install directly to `$LFS`.
    pub fn new_cook_with_dest<'a>(
        &'a self,
        recipe: &'a Recipe,
        dest_dir: &Path,
    ) -> Result<Cook<'a>> {
        Cook::new_with_dest(self, recipe, dest_dir)
    }

    /// Fetch a source archive (with caching)
    pub(crate) fn fetch_source(&self, url: &str, checksum: &str) -> Result<PathBuf> {
        // Create cache directory if needed
        fs::create_dir_all(&self.config.source_cache)?;

        // Use checksum as cache key
        let cache_key = source_cache_key(checksum);
        let cached_path = self.config.source_cache.join(&cache_key);

        // Check if already cached
        if cached_path.exists() {
            debug!("Using cached source: {}", cached_path.display());
            // Verify checksum -- None means match
            if verify_file_checksum(&cached_path, checksum, self.config.checksum_policy)?.is_none()
            {
                return Ok(cached_path);
            }
            warn!("Cached file checksum mismatch, re-downloading");
            fs::remove_file(&cached_path)?;
        }

        if self.config.source_download_policy == SourceDownloadPolicy::OfflineCacheOnly {
            return Err(Error::ConfigError(format!(
                "source cache miss for {url}; hermetic offline build requires prefetch before build"
            )));
        }

        // Download the source
        info!("Downloading: {}", url);
        let temp_path = self.config.source_cache.join(format!("{}.tmp", cache_key));

        let resolved_url = self.recipe_relative_archive_source(url);
        download_file(&resolved_url, &temp_path)?;

        // Verify checksum -- Some(actual) means mismatch
        if let Some(actual) =
            verify_file_checksum(&temp_path, checksum, self.config.checksum_policy)?
        {
            fs::remove_file(&temp_path)?;
            return Err(Error::ChecksumMismatch {
                expected: checksum.to_string(),
                actual,
            });
        }

        // Move to final location
        fs::rename(&temp_path, &cached_path)?;
        Ok(cached_path)
    }

    fn recipe_relative_archive_source(&self, source: &str) -> String {
        if has_url_scheme(source) || Path::new(source).is_absolute() {
            return source.to_string();
        }

        self.config
            .recipe_source_base_dir
            .as_ref()
            .map(|base_dir| base_dir.join(source).to_string_lossy().to_string())
            .unwrap_or_else(|| source.to_string())
    }

    pub(crate) fn resolve_local_source(&self, source: &LocalSourceSection) -> Result<PathBuf> {
        let recipe_dir = self.config.recipe_source_base_dir.as_ref().ok_or_else(|| {
            Error::ConfigError(
                "Local source recipes require KitchenConfig.recipe_source_base_dir; set recipe source base dir to the recipe file directory"
                    .to_string(),
            )
        })?;

        let resolved = source
            .resolve_against(recipe_dir)
            .map_err(Error::ConfigError)?;

        let canonical_recipe_dir = fs::canonicalize(recipe_dir).map_err(|e| {
            Error::ConfigError(format!(
                "Recipe source base dir not found: {} ({e})",
                recipe_dir.display()
            ))
        })?;
        let canonical_source = fs::canonicalize(&resolved).map_err(|e| {
            Error::NotFound(format!(
                "Local source path not found: {} ({e})",
                resolved.display()
            ))
        })?;

        if !canonical_source.starts_with(&canonical_recipe_dir) {
            return Err(Error::ConfigError(format!(
                "Local source path must stay within the recipe directory: {}",
                resolved.display()
            )));
        }

        Ok(canonical_source)
    }
}

fn assert_hermetic_build_execution_boundary(config: &KitchenConfig) -> Result<()> {
    if config.hermetic_evidence.is_none() {
        return Ok(());
    }
    if config.allow_network {
        return Err(Error::ConfigError(
            "hermetic build execution requires allow_network=false".to_string(),
        ));
    }
    if config.source_download_policy != SourceDownloadPolicy::OfflineCacheOnly {
        return Err(Error::ConfigError(
            "hermetic build execution requires source_download_policy=OfflineCacheOnly".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash;
    use crate::recipe::CacheConfig;
    use crate::recipe::format::{
        BuildSection, LocalSourceSection, PackageSection, RemoteSourceSection, SourceSection,
    };
    use crate::recipe::hermetic::evidence::LockedRepositoryDependency;
    use crate::recipe::hermetic::{
        BuilderEnvironmentKind, CiMode, DivergenceStatus, HermeticBuildInput, HostBuildRecord,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    fn make_test_recipe(makedepends: &[&str]) -> Recipe {
        Recipe {
            package: PackageSection {
                name: "test".to_string(),
                version: "1.0".to_string(),
                release: "1".to_string(),
                summary: None,
                description: None,
                license: None,
                homepage: None,
            },
            source: SourceSection::Remote(RemoteSourceSection {
                archive: "https://example.com/test.tar.gz".to_string(),
                checksum: "sha256:abc".to_string(),
                signature: None,
                additional: Vec::new(),
                extract_dir: None,
            }),
            build: BuildSection {
                requires: Vec::new(),
                makedepends: makedepends.iter().map(|s| s.to_string()).collect(),
                configure: None,
                make: None,
                install: None,
                check: None,
                setup: None,
                post_install: None,
                workdir: None,
                environment: std::collections::HashMap::new(),
                jobs: None,
                script_file: None,
                stage: None,
            },
            patches: None,
            cross: None,
            components: None,
            variables: std::collections::HashMap::new(),
        }
    }

    fn make_local_cargo_recipe() -> Recipe {
        Recipe {
            package: PackageSection {
                name: "hermetic-local".to_string(),
                version: "1.0".to_string(),
                release: "1".to_string(),
                summary: None,
                description: None,
                license: None,
                homepage: None,
            },
            source: SourceSection::Local(LocalSourceSection {
                path: PathBuf::from("."),
            }),
            build: BuildSection {
                requires: Vec::new(),
                makedepends: Vec::new(),
                configure: None,
                make: None,
                setup: Some("true # cargo build --locked --offline".to_string()),
                check: None,
                install: Some("printf cooked > %(destdir)s/output.txt".to_string()),
                post_install: None,
                workdir: None,
                environment: std::collections::HashMap::new(),
                jobs: None,
                script_file: None,
                stage: None,
            },
            patches: None,
            cross: None,
            components: None,
            variables: std::collections::HashMap::new(),
        }
    }

    struct RecordingResolver {
        check_calls: Mutex<Vec<Vec<String>>>,
        install_calls: Mutex<Vec<Vec<String>>>,
    }

    impl RecordingResolver {
        fn new() -> Self {
            Self {
                check_calls: Mutex::new(Vec::new()),
                install_calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl MakedependsResolver for RecordingResolver {
        fn check_missing(&self, deps: &[&str]) -> Result<Vec<String>> {
            let deps = deps.iter().map(|dep| dep.to_string()).collect::<Vec<_>>();
            self.check_calls.lock().unwrap().push(deps.clone());
            Ok(deps)
        }

        fn install(&self, deps: &[String]) -> Result<Vec<String>> {
            self.install_calls.lock().unwrap().push(deps.to_vec());
            Ok(deps.to_vec())
        }

        fn cleanup(&self, _installed: &[String]) -> Result<()> {
            Ok(())
        }
    }

    fn locked_repository_dependency(package: &str) -> LockedRepositoryDependency {
        LockedRepositoryDependency {
            repository_url: "https://repo.example.invalid".to_string(),
            snapshot_version: "2026-06-14T00:00:00Z".to_string(),
            package: package.to_string(),
            version: "1.0".to_string(),
            release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            content_identity: "sha256:dependency".to_string(),
        }
    }

    fn host_build_record(output_merkle_root: &str) -> HostBuildRecord {
        HostBuildRecord {
            package_name: "hermetic-local".to_string(),
            package_version: "1.0".to_string(),
            package_release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            output_merkle_root: output_merkle_root.to_string(),
            diagnostic_input_key: None,
            diagnostic_dna_hash: None,
            package_path: None,
            build_timestamp: Some("2026-06-14T00:00:00Z".to_string()),
        }
    }

    fn write_shell_sysroot(sysroot: &Path) {
        copy_tool_with_runtime_deps(Path::new("/bin/sh"), sysroot, Path::new("bin/sh"));
    }

    fn copy_tool_with_runtime_deps(tool: &Path, sysroot: &Path, target_relative: &Path) {
        copy_host_file_into_sysroot(tool, sysroot, target_relative);
        for dependency in ldd_paths(tool) {
            copy_host_file_into_sysroot(
                &dependency,
                sysroot,
                dependency.strip_prefix("/").unwrap(),
            );
        }
    }

    fn copy_host_file_into_sysroot(source: &Path, sysroot: &Path, target_relative: &Path) {
        let destination = sysroot.join(target_relative);
        fs::create_dir_all(destination.parent().expect("sysroot file parent")).unwrap();
        fs::copy(source, &destination)
            .unwrap_or_else(|error| panic!("copy {source:?} to {destination:?}: {error}"));
        let mut permissions = fs::metadata(&destination).unwrap().permissions();
        permissions.set_mode(fs::metadata(source).unwrap().permissions().mode() | 0o555);
        fs::set_permissions(&destination, permissions).unwrap();
    }

    fn ldd_paths(binary: &Path) -> Vec<PathBuf> {
        let output = Command::new("ldd")
            .arg(binary)
            .output()
            .unwrap_or_else(|error| panic!("run ldd {binary:?}: {error}"));
        if !output.status.success() {
            return Vec::new();
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let mut paths = Vec::new();
        for line in text.lines() {
            for token in line.split_whitespace() {
                let token = token.trim_end_matches(':');
                if token.starts_with('/') {
                    let path = PathBuf::from(token);
                    if path.exists() && !paths.contains(&path) {
                        paths.push(path);
                    }
                }
            }
        }
        paths
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
        // Without a resolver, all deps are assumed available
        assert_eq!(result.already_installed.len(), 2);
        assert!(result.newly_installed.is_empty());
        assert!(result.unresolved.is_empty());
    }

    #[test]
    fn test_fetch_source_rejects_md5_in_supported_mode() {
        let cache = tempdir().unwrap();
        let checksum = "md5:d41d8cd98f00b204e9800998ecf8427e";
        let cached_path = cache.path().join(source_cache_key(checksum));
        fs::write(&cached_path, b"").unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            source_cache: cache.path().to_path_buf(),
            ..KitchenConfig::default()
        });

        let err = kitchen
            .fetch_source("https://example.invalid/test.tar.gz", checksum)
            .unwrap_err();

        assert!(
            format!("{err}").contains("Unsupported checksum algorithm: md5"),
            "expected unsupported md5 error, got {err}"
        );
    }

    #[test]
    fn test_fetch_source_allows_md5_in_bootstrap_legacy_mode() {
        let cache = tempdir().unwrap();
        let checksum = "md5:d41d8cd98f00b204e9800998ecf8427e";
        let cached_path = cache.path().join(source_cache_key(checksum));
        fs::write(&cached_path, b"").unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            source_cache: cache.path().to_path_buf(),
            checksum_policy: SourceChecksumPolicy::BootstrapLegacy,
            ..KitchenConfig::default()
        });

        let resolved = kitchen
            .fetch_source("https://example.invalid/test.tar.gz", checksum)
            .unwrap();

        assert_eq!(resolved, cached_path);
    }

    #[test]
    fn offline_cache_only_refuses_missing_source() {
        let cache = tempdir().unwrap();
        let kitchen = Kitchen::new(KitchenConfig {
            source_cache: cache.path().to_path_buf(),
            source_download_policy: SourceDownloadPolicy::OfflineCacheOnly,
            ..KitchenConfig::default()
        });

        let error = kitchen
            .fetch_source("https://example.invalid/test.tar.gz", "sha256:missing")
            .unwrap_err();

        assert!(error.to_string().contains("source cache miss"));
        assert!(
            error
                .to_string()
                .contains("https://example.invalid/test.tar.gz")
        );
        assert!(error.to_string().contains("offline"));
        assert!(error.to_string().contains("prefetch"));
    }

    #[test]
    fn test_fetch_remote_archive_source_uses_archive_cache() {
        let dir = tempdir().unwrap();
        let archive = dir.path().join("source.tar");
        let cache = dir.path().join("cache");
        let bytes = b"archive bytes";
        fs::write(&archive, bytes).unwrap();

        let checksum = hash::sha256_prefixed(bytes);
        let kitchen = Kitchen::new(KitchenConfig {
            source_cache: cache.clone(),
            ..KitchenConfig::default()
        });
        let mut recipe = make_test_recipe(&[]);
        recipe.source = SourceSection::Remote(RemoteSourceSection {
            archive: archive.to_string_lossy().to_string(),
            checksum: checksum.clone(),
            signature: None,
            additional: Vec::new(),
            extract_dir: None,
        });

        let fetched = kitchen.fetch(&recipe).unwrap();

        assert_eq!(fetched, vec![cache.join(source_cache_key(&checksum))]);
        assert_eq!(fs::read(&fetched[0]).unwrap(), bytes);
        assert!(kitchen.sources_cached(&recipe));
    }

    #[test]
    fn test_fetch_remote_archive_source_resolves_relative_to_recipe_base_dir() {
        let dir = tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let sources_dir = recipe_dir.join("sources");
        let cache = dir.path().join("cache");
        fs::create_dir_all(&sources_dir).unwrap();
        let archive = sources_dir.join("source.tar");
        let bytes = b"archive bytes";
        fs::write(&archive, bytes).unwrap();

        let checksum = hash::sha256_prefixed(bytes);
        let kitchen = Kitchen::new(KitchenConfig {
            source_cache: cache.clone(),
            recipe_source_base_dir: Some(recipe_dir),
            ..KitchenConfig::default()
        });
        let mut recipe = make_test_recipe(&[]);
        recipe.source = SourceSection::Remote(RemoteSourceSection {
            archive: "sources/source.tar".to_string(),
            checksum: checksum.clone(),
            signature: None,
            additional: Vec::new(),
            extract_dir: None,
        });

        let fetched = kitchen.fetch(&recipe).unwrap();

        assert_eq!(fetched, vec![cache.join(source_cache_key(&checksum))]);
        assert_eq!(fs::read(&fetched[0]).unwrap(), bytes);
    }

    #[test]
    fn test_fetch_local_path_source_requires_recipe_source_base_dir() {
        let kitchen = Kitchen::new(KitchenConfig::default());
        let mut recipe = make_test_recipe(&[]);
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        let error = kitchen.fetch(&recipe).unwrap_err();

        assert!(
            error.to_string().contains("recipe source base dir"),
            "expected missing base dir error, got: {error}"
        );
    }

    #[test]
    fn test_sources_cached_returns_false_when_local_source_has_no_base_dir() {
        let kitchen = Kitchen::new(KitchenConfig::default());
        let mut recipe = make_test_recipe(&[]);
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        assert!(
            !kitchen.sources_cached(&recipe),
            "local sources without a recipe base dir should not be reported as cached"
        );
    }

    #[test]
    fn test_sources_cached_returns_false_when_local_source_is_not_directory() {
        let dir = tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        fs::create_dir_all(&recipe_dir).unwrap();
        fs::write(recipe_dir.join("src"), b"not a directory").unwrap();
        let kitchen = Kitchen::new(KitchenConfig {
            recipe_source_base_dir: Some(recipe_dir),
            ..KitchenConfig::default()
        });
        let mut recipe = make_test_recipe(&[]);
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        assert!(
            !kitchen.sources_cached(&recipe),
            "local source files should not be reported as cached source directories"
        );
    }

    #[test]
    fn cook_hermetic_prefetches_then_builds_offline() {
        let dir = tempdir().unwrap();
        let source_root = dir.path().join("source");
        let output_dir = dir.path().join("out");
        let sysroot = dir.path().join("sysroot");
        fs::create_dir_all(source_root.join("src")).unwrap();
        write_shell_sysroot(&sysroot);
        fs::write(
            source_root.join("Cargo.toml"),
            "[package]\nname = \"hermetic-local\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(source_root.join("Cargo.lock"), "version = 3\n").unwrap();
        fs::write(source_root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(source_root.join("recipe.toml"), "recipe fixture\n").unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            source_cache: dir.path().join("cache"),
            recipe_source_base_dir: Some(source_root.clone()),
            sysroot: Some(sysroot),
            use_isolation: false,
            allow_network: true,
            memory_limit: 64 * 1024 * 1024 * 1024,
            ..KitchenConfig::default()
        });
        let recipe = make_local_cargo_recipe();
        let input = HermeticBuildInput::explicit_recipe(
            &source_root,
            source_root.join("recipe.toml"),
            hash::sha256_prefixed(b"recipe fixture\n"),
        )
        .with_pristine_builder_environment(
            Some("sha256:1111111111111111111111111111111111111111111111111111111111111111"),
            Some("sha256:2222222222222222222222222222222222222222222222222222222222222222"),
        );

        let result = kitchen
            .cook_hermetic(&recipe, input, &output_dir, CiMode::Off)
            .unwrap();

        assert!(result.package_path.exists());
        let provenance = result.provenance.unwrap();
        assert_eq!(provenance.hardening_level.as_deref(), Some("hermetic"));
        let evidence = provenance.hermetic_evidence.unwrap();
        assert_eq!(
            evidence.build_input.builder_environment.kind,
            BuilderEnvironmentKind::Pristine
        );
    }

    #[test]
    fn cook_hermetic_records_host_divergence_after_merkle_root_is_known() {
        let dir = tempdir().unwrap();
        let source_root = dir.path().join("source");
        let output_dir = dir.path().join("out");
        let sysroot = dir.path().join("sysroot");
        fs::create_dir_all(source_root.join("src")).unwrap();
        write_shell_sysroot(&sysroot);
        fs::write(
            source_root.join("Cargo.toml"),
            "[package]\nname = \"hermetic-local\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(source_root.join("Cargo.lock"), "version = 3\n").unwrap();
        fs::write(source_root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(source_root.join("recipe.toml"), "recipe fixture\n").unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            source_cache: dir.path().join("cache"),
            recipe_source_base_dir: Some(source_root.clone()),
            sysroot: Some(sysroot),
            expected_host_build_record: Some(host_build_record("sha256:host-output")),
            use_isolation: false,
            allow_network: true,
            memory_limit: 64 * 1024 * 1024 * 1024,
            ..KitchenConfig::default()
        });
        let recipe = make_local_cargo_recipe();
        let input = HermeticBuildInput::explicit_recipe(
            &source_root,
            source_root.join("recipe.toml"),
            hash::sha256_prefixed(b"recipe fixture\n"),
        )
        .with_pristine_builder_environment(
            Some("sha256:1111111111111111111111111111111111111111111111111111111111111111"),
            Some("sha256:2222222222222222222222222222222222222222222222222222222222222222"),
        );

        let result = kitchen
            .cook_hermetic(&recipe, input, &output_dir, CiMode::Off)
            .unwrap();

        let provenance = result.provenance.unwrap();
        let evidence = provenance.hermetic_evidence.unwrap();
        assert_eq!(
            evidence.divergence.status,
            DivergenceStatus::DiffersFromHost
        );
        assert!(evidence.divergence.compared);
    }

    #[test]
    fn cook_hermetic_preserves_makedepends_resolver_for_offline_build() {
        let dir = tempdir().unwrap();
        let source_root = dir.path().join("source");
        let output_dir = dir.path().join("out");
        let sysroot = dir.path().join("sysroot");
        fs::create_dir_all(source_root.join("src")).unwrap();
        write_shell_sysroot(&sysroot);
        fs::write(
            source_root.join("Cargo.toml"),
            "[package]\nname = \"hermetic-local\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(source_root.join("Cargo.lock"), "version = 3\n").unwrap();
        fs::write(source_root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(source_root.join("recipe.toml"), "recipe fixture\n").unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let resolver = Arc::new(RecordingResolver::new());
        let kitchen = Kitchen::with_resolver(
            KitchenConfig {
                source_cache: dir.path().join("cache"),
                recipe_source_base_dir: Some(source_root.clone()),
                sysroot: Some(sysroot),
                use_isolation: false,
                allow_network: true,
                auto_makedepends: true,
                cleanup_makedepends: false,
                memory_limit: 64 * 1024 * 1024 * 1024,
                ..KitchenConfig::default()
            },
            resolver.clone(),
        );
        let mut recipe = make_local_cargo_recipe();
        recipe.build.makedepends = vec!["build-tool".to_string()];
        let input = HermeticBuildInput::explicit_recipe(
            &source_root,
            source_root.join("recipe.toml"),
            hash::sha256_prefixed(b"recipe fixture\n"),
        )
        .with_pristine_builder_environment(
            Some("sha256:1111111111111111111111111111111111111111111111111111111111111111"),
            Some("sha256:2222222222222222222222222222222222222222222222222222222222222222"),
        )
        .with_locked_repository_dependencies(vec![locked_repository_dependency("build-tool")]);

        kitchen
            .cook_hermetic(&recipe, input, &output_dir, CiMode::Off)
            .unwrap();

        assert_eq!(
            *resolver.check_calls.lock().unwrap(),
            vec![vec!["build-tool".to_string()]]
        );
        assert_eq!(
            *resolver.install_calls.lock().unwrap(),
            vec![vec!["build-tool".to_string()]]
        );
    }

    #[test]
    fn cook_hermetic_prefetch_uses_input_source_base() {
        let dir = tempdir().unwrap();
        let source_root = dir.path().join("source");
        let output_dir = dir.path().join("out");
        fs::create_dir_all(source_root.join("src")).unwrap();
        fs::write(
            source_root.join("Cargo.toml"),
            "[package]\nname = \"hermetic-local\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(source_root.join("Cargo.lock"), "version = 3\n").unwrap();
        fs::write(source_root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(source_root.join("recipe.toml"), "recipe fixture\n").unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let kitchen = Kitchen::new(KitchenConfig {
            source_cache: dir.path().join("cache"),
            recipe_source_base_dir: None,
            ..KitchenConfig::default()
        });
        let recipe = make_local_cargo_recipe();
        let input = HermeticBuildInput::explicit_recipe(
            &source_root,
            source_root.join("recipe.toml"),
            hash::sha256_prefixed(b"recipe fixture\n"),
        );

        let error = kitchen
            .cook_hermetic(&recipe, input, &output_dir, CiMode::Off)
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("builder environment identity"),
            "cook_hermetic should prefetch using input.recipe_source_base_dir and then reach planning: {error}"
        );
        assert!(
            !error.contains("recipe_source_base_dir"),
            "prefetch should not use the caller's missing KitchenConfig.recipe_source_base_dir: {error}"
        );
    }

    #[test]
    fn hermetic_build_execution_boundary_requires_offline_network_policy() {
        let mut config = KitchenConfig {
            hermetic_evidence: Some(
                crate::ccs::attestation::test_support::sample_hermetic_evidence_for_tests(),
            ),
            allow_network: true,
            source_download_policy: SourceDownloadPolicy::AllowDownloads,
            ..KitchenConfig::default()
        };

        let error = assert_hermetic_build_execution_boundary(&config).unwrap_err();
        assert!(error.to_string().contains("allow_network=false"));

        config.allow_network = false;
        let error = assert_hermetic_build_execution_boundary(&config).unwrap_err();
        assert!(error.to_string().contains("OfflineCacheOnly"));

        config.source_download_policy = SourceDownloadPolicy::OfflineCacheOnly;
        assert_hermetic_build_execution_boundary(&config).unwrap();
    }

    #[test]
    fn test_cook_cached_rejects_local_source_recipe() {
        let dir = tempdir().unwrap();
        let recipe_dir = dir.path().join("recipe");
        let workspace = recipe_dir.join("src");
        let output_dir = dir.path().join("out");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&output_dir).unwrap();
        let kitchen = Kitchen::new(KitchenConfig {
            recipe_source_base_dir: Some(recipe_dir),
            use_isolation: false,
            ..KitchenConfig::default()
        });
        let cache = BuildCache::new(CacheConfig {
            cache_dir: dir.path().join("cache"),
            ..Default::default()
        })
        .unwrap();
        let mut recipe = make_test_recipe(&[]);
        recipe.source = SourceSection::Local(LocalSourceSection {
            path: PathBuf::from("./src"),
        });

        let error = kitchen
            .cook_cached(&recipe, &output_dir, &cache, &ToolchainInfo::default())
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("local source recipes are not supported by cached cooking in M1a"),
            "expected cached-cook local source rejection, got: {error}"
        );
    }
}
