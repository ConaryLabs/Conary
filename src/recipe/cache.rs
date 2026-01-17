// src/recipe/cache.rs

//! Build artifact caching for the Kitchen
//!
//! Caches built CCS packages based on a hash of:
//! - Recipe content (name, version, build config, patches)
//! - Toolchain version (compiler, linker)
//! - Build environment (environment variables, stage)
//!
//! This allows skipping expensive builds when nothing has changed.

use crate::error::Result;
use crate::hash::{hash_bytes, HashAlgorithm};
use crate::recipe::format::{BuildStage, Recipe};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::{debug, info, warn};

/// Configuration for the build cache
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Root directory for cached artifacts
    pub cache_dir: PathBuf,
    /// Maximum cache size in bytes (0 = unlimited)
    pub max_size: u64,
    /// Maximum age for cache entries (0 = no expiry)
    pub max_age: Duration,
    /// Whether to verify cached artifacts before use
    pub verify_integrity: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            cache_dir: PathBuf::from("/var/cache/conary/builds"),
            max_size: 10 * 1024 * 1024 * 1024, // 10 GB
            max_age: Duration::from_secs(30 * 24 * 60 * 60), // 30 days
            verify_integrity: true,
        }
    }
}

/// Toolchain information for cache key generation
#[derive(Debug, Clone, Default)]
pub struct ToolchainInfo {
    /// Compiler version (e.g., "gcc 13.2.0")
    pub compiler_version: Option<String>,
    /// Linker version
    pub linker_version: Option<String>,
    /// Target triple (e.g., "x86_64-unknown-linux-gnu")
    pub target: Option<String>,
    /// Sysroot path
    pub sysroot: Option<PathBuf>,
    /// Build stage
    pub stage: Option<BuildStage>,
}

impl ToolchainInfo {
    /// Create toolchain info from environment
    pub fn from_env() -> Self {
        Self {
            compiler_version: std::env::var("CC_VERSION").ok(),
            linker_version: std::env::var("LD_VERSION").ok(),
            target: std::env::var("TARGET").ok(),
            sysroot: std::env::var("SYSROOT").ok().map(PathBuf::from),
            stage: std::env::var("CONARY_STAGE").ok().and_then(|s| match s.as_str() {
                "stage0" => Some(BuildStage::Stage0),
                "stage1" => Some(BuildStage::Stage1),
                "stage2" => Some(BuildStage::Stage2),
                "final" => Some(BuildStage::Final),
                _ => None,
            }),
        }
    }

    /// Compute hash of toolchain info
    fn hash(&self) -> String {
        let mut data = String::new();

        if let Some(ref v) = self.compiler_version {
            data.push_str("cc:");
            data.push_str(v);
            data.push('\n');
        }
        if let Some(ref v) = self.linker_version {
            data.push_str("ld:");
            data.push_str(v);
            data.push('\n');
        }
        if let Some(ref t) = self.target {
            data.push_str("target:");
            data.push_str(t);
            data.push('\n');
        }
        if let Some(ref s) = self.sysroot {
            data.push_str("sysroot:");
            data.push_str(&s.to_string_lossy());
            data.push('\n');
        }
        if let Some(stage) = self.stage {
            data.push_str("stage:");
            data.push_str(stage.as_str());
            data.push('\n');
        }

        hash_bytes(HashAlgorithm::Sha256, data.as_bytes()).as_str().to_string()
    }
}

/// A cache entry for a built package
#[derive(Debug)]
pub struct CacheEntry {
    /// Path to the cached CCS package
    pub package_path: PathBuf,
    /// Cache key used
    pub cache_key: String,
    /// When the entry was created
    pub created: SystemTime,
    /// Size of the cached package in bytes
    pub size: u64,
}

/// Build artifact cache
#[derive(Debug)]
pub struct BuildCache {
    config: CacheConfig,
}

impl BuildCache {
    /// Create a new build cache with the given configuration
    pub fn new(config: CacheConfig) -> Result<Self> {
        // Ensure cache directory exists
        fs::create_dir_all(&config.cache_dir)?;

        Ok(Self { config })
    }

    /// Create a build cache with default configuration
    pub fn with_defaults() -> Result<Self> {
        Self::new(CacheConfig::default())
    }

    /// Compute a cache key for a recipe and toolchain
    pub fn cache_key(&self, recipe: &Recipe, toolchain: &ToolchainInfo) -> String {
        let recipe_hash = self.hash_recipe(recipe);
        let toolchain_hash = toolchain.hash();

        // Combine hashes for final key
        let combined = format!("{}\n{}", recipe_hash, toolchain_hash);
        let key = hash_bytes(HashAlgorithm::Sha256, combined.as_bytes())
            .as_str()
            .to_string();

        debug!(
            "Cache key for {}-{}: {} (recipe: {:.8}, toolchain: {:.8})",
            recipe.package.name, recipe.package.version, &key[..16], recipe_hash, toolchain_hash
        );

        key
    }

    /// Hash a recipe's build-relevant content
    fn hash_recipe(&self, recipe: &Recipe) -> String {
        // Use a deterministic serialization for hashing
        // BTreeMap ensures consistent ordering
        let mut data = String::new();

        // Package identity
        data.push_str(&format!(
            "name:{}\nversion:{}\nrelease:{}\n",
            recipe.package.name, recipe.package.version, recipe.package.release
        ));

        // Source info
        data.push_str(&format!(
            "archive:{}\nchecksum:{}\n",
            recipe.source.archive, recipe.source.checksum
        ));

        // Additional sources (sorted for determinism)
        let mut additional: Vec<_> = recipe
            .source
            .additional
            .iter()
            .map(|a| format!("{}:{}", a.url, a.checksum))
            .collect();
        additional.sort();
        for a in additional {
            data.push_str(&format!("additional:{}\n", a));
        }

        // Patches (order matters for patches)
        if let Some(patches) = &recipe.patches {
            for patch in &patches.files {
                data.push_str(&format!(
                    "patch:{}:{}:{}\n",
                    patch.file,
                    patch.checksum.as_deref().unwrap_or(""),
                    patch.strip
                ));
            }
        }

        // Build configuration
        if let Some(ref configure) = recipe.build.configure {
            data.push_str(&format!("configure:{}\n", configure));
        }
        if let Some(ref make) = recipe.build.make {
            data.push_str(&format!("make:{}\n", make));
        }
        if let Some(ref install) = recipe.build.install {
            data.push_str(&format!("install:{}\n", install));
        }
        if let Some(ref setup) = recipe.build.setup {
            data.push_str(&format!("setup:{}\n", setup));
        }
        if let Some(ref check) = recipe.build.check {
            data.push_str(&format!("check:{}\n", check));
        }
        if let Some(ref post_install) = recipe.build.post_install {
            data.push_str(&format!("post_install:{}\n", post_install));
        }

        // Environment (sorted for determinism)
        let env: BTreeMap<_, _> = recipe.build.environment.iter().collect();
        for (k, v) in env {
            data.push_str(&format!("env:{}={}\n", k, v));
        }

        // Dependencies (sorted)
        let mut requires: Vec<_> = recipe.build.requires.iter().cloned().collect();
        requires.sort();
        for req in requires {
            data.push_str(&format!("requires:{}\n", req));
        }

        let mut makedepends: Vec<_> = recipe.build.makedepends.iter().cloned().collect();
        makedepends.sort();
        for dep in makedepends {
            data.push_str(&format!("makedepends:{}\n", dep));
        }

        // Cross-compilation settings
        if let Some(ref cross) = recipe.cross {
            if let Some(ref target) = cross.target {
                data.push_str(&format!("cross.target:{}\n", target));
            }
            if let Some(ref sysroot) = cross.sysroot {
                data.push_str(&format!("cross.sysroot:{}\n", sysroot));
            }
            if let Some(stage) = cross.stage {
                data.push_str(&format!("cross.stage:{}\n", stage.as_str()));
            }
        }

        hash_bytes(HashAlgorithm::Sha256, data.as_bytes())
            .as_str()
            .to_string()
    }

    /// Get the cache path for a given key
    fn cache_path(&self, key: &str) -> PathBuf {
        // Use first 2 chars as subdirectory for sharding
        let shard = &key[..2];
        self.config.cache_dir.join(shard).join(format!("{}.ccs", key))
    }

    /// Get the metadata path for a cache entry
    fn metadata_path(&self, key: &str) -> PathBuf {
        let shard = &key[..2];
        self.config.cache_dir.join(shard).join(format!("{}.meta", key))
    }

    /// Check if a cached build exists for the given recipe and toolchain
    pub fn get(
        &self,
        recipe: &Recipe,
        toolchain: &ToolchainInfo,
    ) -> Result<Option<CacheEntry>> {
        let key = self.cache_key(recipe, toolchain);
        self.get_by_key(&key)
    }

    /// Get a cached build by key
    pub fn get_by_key(&self, key: &str) -> Result<Option<CacheEntry>> {
        let cache_path = self.cache_path(key);

        if !cache_path.exists() {
            debug!("Cache miss: {}", &key[..16]);
            return Ok(None);
        }

        // Check file metadata
        let metadata = fs::metadata(&cache_path)?;
        let created = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let size = metadata.len();

        // Check age (skip if max_age is zero, meaning no expiry)
        if !self.config.max_age.is_zero() {
            let age = SystemTime::now()
                .duration_since(created)
                .unwrap_or(Duration::ZERO);
            if age > self.config.max_age {
                debug!("Cache expired: {} (age: {:?})", &key[..16], age);
                // Remove expired entry
                let _ = fs::remove_file(&cache_path);
                let _ = fs::remove_file(self.metadata_path(key));
                return Ok(None);
            }
        }

        // Verify integrity if enabled
        if self.config.verify_integrity {
            if !self.verify_entry(&cache_path)? {
                warn!("Cache corruption detected: {}", &key[..16]);
                let _ = fs::remove_file(&cache_path);
                let _ = fs::remove_file(self.metadata_path(key));
                return Ok(None);
            }
        }

        info!("Cache hit: {} ({} bytes)", &key[..16], size);

        Ok(Some(CacheEntry {
            package_path: cache_path,
            cache_key: key.to_string(),
            created,
            size,
        }))
    }

    /// Verify the integrity of a cached entry
    fn verify_entry(&self, path: &Path) -> Result<bool> {
        // Read the CCS file and verify it's a valid archive
        // For now, just check that it exists and is non-empty
        let metadata = fs::metadata(path)?;
        if metadata.len() == 0 {
            return Ok(false);
        }

        // Could add more sophisticated verification here:
        // - Check CCS magic bytes
        // - Verify internal checksums
        // - Parse manifest

        Ok(true)
    }

    /// Store a built package in the cache
    pub fn put(
        &self,
        recipe: &Recipe,
        toolchain: &ToolchainInfo,
        package_path: &Path,
    ) -> Result<CacheEntry> {
        let key = self.cache_key(recipe, toolchain);
        self.put_with_key(&key, package_path, recipe)
    }

    /// Store a package with a specific key
    fn put_with_key(
        &self,
        key: &str,
        package_path: &Path,
        recipe: &Recipe,
    ) -> Result<CacheEntry> {
        let cache_path = self.cache_path(key);

        // Create shard directory
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Copy package to cache
        fs::copy(package_path, &cache_path)?;

        // Write metadata
        let metadata_path = self.metadata_path(key);
        let metadata = format!(
            "name={}\nversion={}\nrelease={}\n",
            recipe.package.name, recipe.package.version, recipe.package.release
        );
        fs::write(&metadata_path, metadata)?;

        let file_metadata = fs::metadata(&cache_path)?;
        let created = file_metadata.modified().unwrap_or(SystemTime::now());
        let size = file_metadata.len();

        info!(
            "Cached: {}-{} as {} ({} bytes)",
            recipe.package.name,
            recipe.package.version,
            &key[..16],
            size
        );

        // Enforce size limits
        self.enforce_limits()?;

        Ok(CacheEntry {
            package_path: cache_path,
            cache_key: key.to_string(),
            created,
            size,
        })
    }

    /// Copy a cached package to a destination
    pub fn copy_to(&self, entry: &CacheEntry, dest: &Path) -> Result<PathBuf> {
        fs::copy(&entry.package_path, dest)?;
        Ok(dest.to_path_buf())
    }

    /// Enforce cache size limits using LRU eviction
    fn enforce_limits(&self) -> Result<()> {
        if self.config.max_size == 0 {
            return Ok(());
        }

        // Collect all cache entries with their metadata
        let mut entries: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
        let mut total_size = 0u64;

        for shard_entry in fs::read_dir(&self.config.cache_dir)? {
            let shard_entry = shard_entry?;
            if !shard_entry.file_type()?.is_dir() {
                continue;
            }

            for file_entry in fs::read_dir(shard_entry.path())? {
                let file_entry = file_entry?;
                let path = file_entry.path();

                if path.extension().is_some_and(|e| e == "ccs") {
                    if let Ok(metadata) = fs::metadata(&path) {
                        let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                        let size = metadata.len();
                        entries.push((path, mtime, size));
                        total_size += size;
                    }
                }
            }
        }

        // If under limit, nothing to do
        if total_size <= self.config.max_size {
            return Ok(());
        }

        // Sort by access time (oldest first)
        entries.sort_by_key(|(_, mtime, _)| *mtime);

        // Remove oldest entries until under limit
        for (path, _, size) in entries {
            if total_size <= self.config.max_size {
                break;
            }

            debug!("Evicting {} ({} bytes)", path.display(), size);

            // Remove CCS and metadata files
            let _ = fs::remove_file(&path);
            let meta_path = path.with_extension("meta");
            let _ = fs::remove_file(meta_path);

            total_size = total_size.saturating_sub(size);
        }

        Ok(())
    }

    /// Clear all cached builds
    pub fn clear(&self) -> Result<u64> {
        let mut removed = 0u64;

        for entry in fs::read_dir(&self.config.cache_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Remove shard directory contents
                for file in fs::read_dir(&path)? {
                    let file = file?;
                    fs::remove_file(file.path())?;
                    removed += 1;
                }
                // Try to remove empty shard directory
                let _ = fs::remove_dir(&path);
            }
        }

        info!("Cleared {} cache entries", removed);
        Ok(removed)
    }

    /// Get cache statistics
    pub fn stats(&self) -> Result<CacheStats> {
        let mut total_size = 0u64;
        let mut entry_count = 0u64;
        let mut oldest: Option<SystemTime> = None;
        let mut newest: Option<SystemTime> = None;

        for shard_entry in fs::read_dir(&self.config.cache_dir)? {
            let shard_entry = shard_entry?;
            if !shard_entry.file_type()?.is_dir() {
                continue;
            }

            for file_entry in fs::read_dir(shard_entry.path())? {
                let file_entry = file_entry?;
                let path = file_entry.path();

                if path.extension().is_some_and(|e| e == "ccs") {
                    if let Ok(metadata) = fs::metadata(&path) {
                        let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                        let size = metadata.len();

                        total_size += size;
                        entry_count += 1;

                        oldest = Some(oldest.map_or(mtime, |o| o.min(mtime)));
                        newest = Some(newest.map_or(mtime, |n| n.max(mtime)));
                    }
                }
            }
        }

        Ok(CacheStats {
            total_size,
            entry_count,
            max_size: self.config.max_size,
            oldest,
            newest,
        })
    }
}

/// Cache statistics
#[derive(Debug)]
pub struct CacheStats {
    /// Total size of cached artifacts in bytes
    pub total_size: u64,
    /// Number of cached entries
    pub entry_count: u64,
    /// Maximum configured size
    pub max_size: u64,
    /// Oldest cache entry
    pub oldest: Option<SystemTime>,
    /// Newest cache entry
    pub newest: Option<SystemTime>,
}

impl CacheStats {
    /// Get cache utilization as a percentage
    pub fn utilization(&self) -> f64 {
        if self.max_size == 0 {
            0.0
        } else {
            (self.total_size as f64 / self.max_size as f64) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::format::{BuildSection, PackageSection, SourceSection};
    use tempfile::TempDir;

    fn make_test_recipe(name: &str, version: &str) -> Recipe {
        Recipe {
            package: PackageSection {
                name: name.to_string(),
                version: version.to_string(),
                release: "1".to_string(),
                summary: None,
                description: None,
                license: None,
                homepage: None,
            },
            source: SourceSection {
                archive: format!("https://example.com/{}-{}.tar.gz", name, version),
                checksum: "sha256:abc123".to_string(),
                signature: None,
                additional: Vec::new(),
                extract_dir: None,
            },
            build: BuildSection {
                requires: Vec::new(),
                makedepends: Vec::new(),
                configure: Some("./configure".to_string()),
                make: Some("make".to_string()),
                install: Some("make install".to_string()),
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
    fn test_cache_key_deterministic() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        let recipe = make_test_recipe("test", "1.0.0");
        let toolchain = ToolchainInfo::default();

        let key1 = cache.cache_key(&recipe, &toolchain);
        let key2 = cache.cache_key(&recipe, &toolchain);

        assert_eq!(key1, key2);
    }

    #[test]
    fn test_cache_key_changes_with_version() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        let recipe1 = make_test_recipe("test", "1.0.0");
        let recipe2 = make_test_recipe("test", "1.0.1");
        let toolchain = ToolchainInfo::default();

        let key1 = cache.cache_key(&recipe1, &toolchain);
        let key2 = cache.cache_key(&recipe2, &toolchain);

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_cache_key_changes_with_toolchain() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        let recipe = make_test_recipe("test", "1.0.0");

        let toolchain1 = ToolchainInfo {
            compiler_version: Some("gcc 13.2.0".to_string()),
            ..Default::default()
        };
        let toolchain2 = ToolchainInfo {
            compiler_version: Some("gcc 14.0.0".to_string()),
            ..Default::default()
        };

        let key1 = cache.cache_key(&recipe, &toolchain1);
        let key2 = cache.cache_key(&recipe, &toolchain2);

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_cache_key_changes_with_stage() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        let recipe = make_test_recipe("test", "1.0.0");

        let toolchain1 = ToolchainInfo {
            stage: Some(BuildStage::Stage0),
            ..Default::default()
        };
        let toolchain2 = ToolchainInfo {
            stage: Some(BuildStage::Stage1),
            ..Default::default()
        };

        let key1 = cache.cache_key(&recipe, &toolchain1);
        let key2 = cache.cache_key(&recipe, &toolchain2);

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_cache_miss() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        let recipe = make_test_recipe("test", "1.0.0");
        let toolchain = ToolchainInfo::default();

        let result = cache.get(&recipe, &toolchain).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_cache_put_and_get() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            verify_integrity: true,
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        let recipe = make_test_recipe("test", "1.0.0");
        let toolchain = ToolchainInfo::default();

        // Create a fake package file
        let package_path = temp.path().join("test-1.0.0.ccs");
        fs::write(&package_path, b"fake ccs content").unwrap();

        // Put in cache
        let entry = cache.put(&recipe, &toolchain, &package_path).unwrap();
        assert_eq!(entry.size, 16);

        // Get from cache
        let retrieved = cache.get(&recipe, &toolchain).unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.cache_key, entry.cache_key);
    }

    #[test]
    fn test_cache_copy_to() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        let recipe = make_test_recipe("test", "1.0.0");
        let toolchain = ToolchainInfo::default();

        // Create and cache a package
        let package_path = temp.path().join("test-1.0.0.ccs");
        fs::write(&package_path, b"test content").unwrap();
        let entry = cache.put(&recipe, &toolchain, &package_path).unwrap();

        // Copy to destination
        let dest = temp.path().join("output.ccs");
        cache.copy_to(&entry, &dest).unwrap();

        assert!(dest.exists());
        assert_eq!(fs::read(&dest).unwrap(), b"test content");
    }

    #[test]
    fn test_cache_clear() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        // Add some entries
        for i in 0..3 {
            let recipe = make_test_recipe("test", &format!("1.0.{}", i));
            let toolchain = ToolchainInfo::default();
            let package_path = temp.path().join(format!("test-1.0.{}.ccs", i));
            fs::write(&package_path, b"content").unwrap();
            cache.put(&recipe, &toolchain, &package_path).unwrap();
        }

        // Verify entries exist
        let stats = cache.stats().unwrap();
        assert_eq!(stats.entry_count, 3);

        // Clear cache
        let removed = cache.clear().unwrap();
        assert!(removed > 0);

        // Verify cache is empty
        let stats = cache.stats().unwrap();
        assert_eq!(stats.entry_count, 0);
    }

    #[test]
    fn test_cache_stats() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            max_size: 1024 * 1024, // 1 MB
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        // Initially empty
        let stats = cache.stats().unwrap();
        assert_eq!(stats.entry_count, 0);
        assert_eq!(stats.total_size, 0);

        // Add an entry
        let recipe = make_test_recipe("test", "1.0.0");
        let toolchain = ToolchainInfo::default();
        let package_path = temp.path().join("test.ccs");
        let content = vec![0u8; 1000];
        fs::write(&package_path, &content).unwrap();
        cache.put(&recipe, &toolchain, &package_path).unwrap();

        // Check stats
        let stats = cache.stats().unwrap();
        assert_eq!(stats.entry_count, 1);
        assert_eq!(stats.total_size, 1000);
        assert!(stats.utilization() > 0.0);
        assert!(stats.utilization() < 1.0);
    }

    #[test]
    fn test_cache_lru_eviction() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            max_size: 2000, // Very small limit
            max_age: Duration::ZERO, // No expiry
            verify_integrity: false,
        };
        let cache = BuildCache::new(config).unwrap();

        // Add entries that exceed limit
        for i in 0..5 {
            let recipe = make_test_recipe("test", &format!("1.0.{}", i));
            let toolchain = ToolchainInfo::default();
            let package_path = temp.path().join(format!("test-{}.ccs", i));
            let content = vec![0u8; 500]; // 500 bytes each
            fs::write(&package_path, &content).unwrap();
            cache.put(&recipe, &toolchain, &package_path).unwrap();

            // Small delay to ensure different timestamps
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Check that we're at or under limit
        let stats = cache.stats().unwrap();
        assert!(stats.total_size <= 2000);
        // Should have evicted some entries
        assert!(stats.entry_count < 5);
    }

    #[test]
    fn test_toolchain_info_from_env() {
        // This just tests that it doesn't panic
        let info = ToolchainInfo::from_env();
        // Most vars won't be set in test env
        assert!(info.compiler_version.is_none() || info.compiler_version.is_some());
    }

    #[test]
    fn test_toolchain_info_hash_deterministic() {
        let info = ToolchainInfo {
            compiler_version: Some("gcc 13.2.0".to_string()),
            linker_version: Some("ld 2.40".to_string()),
            target: Some("x86_64-unknown-linux-gnu".to_string()),
            sysroot: Some(PathBuf::from("/opt/sysroot")),
            stage: Some(BuildStage::Stage1),
        };

        let hash1 = info.hash();
        let hash2 = info.hash();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_cache_path_sharding() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        // Keys should be stored in sharded directories
        let path = cache.cache_path("abcdef123456");
        assert!(path.to_string_lossy().contains("/ab/"));
    }

    #[test]
    fn test_cache_expired_entry() {
        let temp = TempDir::new().unwrap();
        // Use a 100ms expiry - short but reliable across filesystems
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            max_age: Duration::from_millis(100),
            verify_integrity: false,
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        let recipe = make_test_recipe("test", "1.0.0");
        let toolchain = ToolchainInfo::default();

        // Create and cache a package
        let package_path = temp.path().join("test.ccs");
        fs::write(&package_path, b"content").unwrap();
        cache.put(&recipe, &toolchain, &package_path).unwrap();

        // Wait for expiry (200ms to be safe with filesystem time resolution)
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Should be expired
        let result = cache.get(&recipe, &toolchain).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_cache_verify_empty_file() {
        let temp = TempDir::new().unwrap();
        let config = CacheConfig {
            cache_dir: temp.path().to_path_buf(),
            verify_integrity: true,
            ..Default::default()
        };
        let cache = BuildCache::new(config).unwrap();

        let recipe = make_test_recipe("test", "1.0.0");
        let toolchain = ToolchainInfo::default();
        let key = cache.cache_key(&recipe, &toolchain);

        // Create cache directory and empty file
        let cache_path = cache.cache_path(&key);
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(&cache_path, b"").unwrap(); // Empty file

        // Should fail verification
        let result = cache.get_by_key(&key).unwrap();
        assert!(result.is_none());
    }
}
