// src/recipe/kitchen/mod.rs

//! Kitchen: the isolated build environment for cooking recipes
//!
//! The Kitchen provides a sandboxed environment for building packages
//! from source recipes. It handles:
//! - Fetching source archives and patches
//! - Extracting and patching sources
//! - Running build commands in isolation
//! - Packaging the result as CCS

mod archive;
mod config;
mod cook;
pub mod makedepends;
pub mod provenance_capture;

pub use config::{CookResult, KitchenConfig, StageConfig, StageRegistry};
pub use cook::Cook;
pub use makedepends::{MakedependsResolver, MakedependsResult, NoopResolver};
// ProvenanceCapture is used internally by cook.rs; export for external use if needed
#[allow(unused_imports)]
pub use provenance_capture::{CapturedDep, CapturedPatch, ProvenanceCapture};

use crate::error::{Error, Result};
use crate::recipe::cache::{BuildCache, ToolchainInfo};
use crate::recipe::format::Recipe;
use archive::{download_file, verify_file_checksum};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

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

        // Fetch main source archive
        let archive_url = recipe.archive_url();
        info!("Fetching: {}", archive_url);
        let path = self.fetch_source(&archive_url, &recipe.source.checksum)?;
        fetched.push(path);

        // Fetch additional sources
        for additional in &recipe.source.additional {
            info!("Fetching additional: {}", additional.url);
            let path = self.fetch_source(&additional.url, &additional.checksum)?;
            fetched.push(path);
        }

        // Fetch remote patches
        if let Some(patches) = &recipe.patches {
            for patch in &patches.files {
                if patch.file.starts_with("http://") || patch.file.starts_with("https://") {
                    info!("Fetching patch: {}", patch.file);
                    let checksum = patch.checksum.as_deref().unwrap_or("sha256:0");
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
        // Check main archive
        let cache_key = recipe.source.checksum.replace(':', "_");
        let cached_path = self.config.source_cache.join(&cache_key);
        if !cached_path.exists() {
            return false;
        }

        // Check additional sources
        for additional in &recipe.source.additional {
            let cache_key = additional.checksum.replace(':', "_");
            let cached_path = self.config.source_cache.join(&cache_key);
            if !cached_path.exists() {
                return false;
            }
        }

        // Check remote patches
        if let Some(patches) = &recipe.patches {
            for patch in &patches.files {
                if patch.file.starts_with("http://") || patch.file.starts_with("https://") {
                    if let Some(checksum) = &patch.checksum {
                        let cache_key = checksum.replace(':', "_");
                        let cached_path = self.config.source_cache.join(&cache_key);
                        if !cached_path.exists() {
                            return false;
                        }
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

    /// Fetch a source archive (with caching)
    pub(crate) fn fetch_source(&self, url: &str, checksum: &str) -> Result<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::format::{BuildSection, PackageSection, SourceSection};

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
            source: SourceSection {
                archive: "https://example.com/test.tar.gz".to_string(),
                checksum: "sha256:abc".to_string(),
                signature: None,
                additional: Vec::new(),
                extract_dir: None,
            },
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
            },
            patches: None,
            cross: None,
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
        // Without a resolver, all deps are assumed available
        assert_eq!(result.already_installed.len(), 2);
        assert!(result.newly_installed.is_empty());
        assert!(result.unresolved.is_empty());
    }
}
