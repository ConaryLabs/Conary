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
mod package_output;
pub mod provenance_capture;
mod reproducibility_env;

pub use config::{
    CcsPackageSigningAuthority, CookResult, KitchenConfig, SourceDownloadPolicy, StageConfig,
    StageRegistry,
};
pub use cook::Cook;
// Re-exported for external consumers (e.g., CLI tools that inspect provenance)
#[allow(unused_imports)]
pub use provenance_capture::{CapturedDep, CapturedPatch, ProvenanceCapture};

use crate::ccs::verify::{TrustPolicy, verify_package};
use crate::error::{Error, Result};
use crate::recipe::cache::{BuildCache, ToolchainInfo};
use crate::recipe::format::{LocalSourceSection, Recipe, SourceSection, is_remote_url};
use crate::recipe::hermetic::{CiMode, HermeticBuildInput, HermeticBuildPlan};
use archive::{download_file, verify_file_checksum};
use std::fs;
use std::path::{Path, PathBuf};
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
}

impl Kitchen {
    /// Create a new Kitchen with the given configuration
    pub fn new(config: KitchenConfig) -> Self {
        Self { config }
    }

    /// Create a Kitchen with default configuration
    pub fn with_defaults() -> Self {
        Self::new(KitchenConfig::default())
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
    /// 1. **Prep**: Fetch source archives and patches (with network)
    /// 2. **Unpack**: Extract sources and apply patches
    /// 3. **Simmer**: Run configure/make/install (network blocked)
    /// 4. **Plate**: Package result as CCS
    ///
    /// Recipe build requirements describe the required builder environment.
    /// Kitchen does not invoke a host package manager or invent dependency
    /// identities; hermetic callers must supply locked repository identities.
    pub fn cook(&self, recipe: &Recipe, output_dir: &Path) -> Result<CookResult> {
        info!(
            "Cooking {} version {}",
            recipe.package.name, recipe.package.version
        );

        let mut cook = Cook::new(self, recipe)?;

        info!("Prep: fetching ingredients...");
        cook.prep()?;

        info!("Unpacking and patching sources...");
        cook.unpack()?;
        cook.patch()?;

        info!("Simmering: running build...");
        cook.simmer()?;

        info!("Plating: creating CCS package...");
        let (package_path, provenance) = cook.plate(output_dir)?;

        Ok(CookResult {
            package_path,
            log: cook.log,
            warnings: cook.warnings,
            from_cache: false,
            cache_key: None,
            provenance: Some(provenance),
        })
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
        Self::new(prefetch_config).fetch(recipe)?;
        let plan = HermeticBuildPlan::from_recipe(recipe, input, ci_mode)?;
        let mut build_config = self.config.clone();
        plan.apply_to_kitchen_config(&mut build_config);
        assert_hermetic_build_execution_boundary(&build_config)?;
        let kitchen = Self::new(build_config);
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
            let signing_key = self
                .config
                .ccs_signing_authority
                .as_ref()
                .ok_or_else(|| {
                    Error::ConfigError(
                        "cached Kitchen CCS output requires an explicit CcsPackageSigningAuthority"
                            .to_string(),
                    )
                })?
                .key_pair();
            verify_package(
                &entry.package_path,
                &TrustPolicy::strict(vec![signing_key.public_key_base64()]),
            )
            .map_err(|error| {
                Error::IoError(format!(
                    "cached CCS package {} is not a signed current-v2 artifact from the configured authority: {error}",
                    entry.package_path.display()
                ))
            })?;
            cache.copy_to(&entry, &output_path)?;

            return Ok(CookResult {
                package_path: output_path,
                log: format!("Cache hit: {}", entry.cache_key),
                warnings: Vec::new(),
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
            if verify_file_checksum(&cached_path, checksum)?.is_none() {
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
        if let Some(actual) = verify_file_checksum(&temp_path, checksum)? {
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
#[path = "tests.rs"]
mod tests;
