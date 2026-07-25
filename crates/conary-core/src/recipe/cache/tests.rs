// conary-core/src/recipe/cache/tests.rs

use super::*;
use crate::recipe::format::{
    BuildSection, LocalSourceSection, PackageSection, RemoteSourceSection, SourceSection,
};
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
        source: SourceSection::Remote(RemoteSourceSection {
            archive: format!("https://example.com/{}-{}.tar.gz", name, version),
            checksum: "sha256:abc123".to_string(),
            signature: None,
            additional: Vec::new(),
            extract_dir: None,
        }),
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
            stage: None,
        },
        cross: None,
        patches: None,
        components: None,
        variables: std::collections::HashMap::new(),
    }
}

fn make_local_source_recipe() -> Recipe {
    let mut recipe = make_test_recipe("local", "1.0.0");
    recipe.source = SourceSection::Local(LocalSourceSection {
        path: PathBuf::from("src"),
    });
    recipe
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
fn test_get_rejects_local_source_recipe_cache_key() {
    let temp = TempDir::new().unwrap();
    let cache = BuildCache::new(CacheConfig {
        cache_dir: temp.path().to_path_buf(),
        ..Default::default()
    })
    .unwrap();
    let recipe = make_local_source_recipe();
    let toolchain = ToolchainInfo::default();

    let error = cache.get(&recipe, &toolchain).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("local source recipes are not supported by cached cooking in M1a"),
        "expected local-source cache rejection, got: {error}"
    );
}

#[test]
fn test_put_rejects_local_source_recipe_cache_key() {
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("local-1.0.0-1.ccs");
    fs::write(&package, b"package").unwrap();
    let cache = BuildCache::new(CacheConfig {
        cache_dir: temp.path().join("cache"),
        ..Default::default()
    })
    .unwrap();
    let recipe = make_local_source_recipe();
    let toolchain = ToolchainInfo::default();

    let error = cache.put(&recipe, &toolchain, &package).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("local source recipes are not supported by cached cooking in M1a"),
        "expected local-source cache rejection, got: {error}"
    );
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
        max_size: 2000,          // Very small limit
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
fn test_cache_verify_corrupted_file() {
    let temp = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp.path().to_path_buf(),
        verify_integrity: true,
        ..Default::default()
    };
    let cache = BuildCache::new(config).unwrap();

    let recipe = make_test_recipe("test", "1.0.0");
    let toolchain = ToolchainInfo::default();

    // Create and cache a package
    let package_path = temp.path().join("test.ccs");
    fs::write(&package_path, b"original content").unwrap();
    let entry = cache.put(&recipe, &toolchain, &package_path).unwrap();

    // Corrupt the cached file
    fs::write(&entry.package_path, b"corrupted content").unwrap();

    // Should detect corruption and return None
    let result = cache.get(&recipe, &toolchain).unwrap();
    assert!(result.is_none());

    // Corrupted file should have been removed
    assert!(!entry.package_path.exists());
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

#[test]
fn cache_integrity_rejects_entry_without_checksum_metadata() {
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
    let cache_path = cache.cache_path(&key);
    fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    fs::write(&cache_path, b"untyped cache entry").unwrap();

    assert!(cache.get_by_key(&key).unwrap().is_none());
    assert!(!cache_path.exists());
}

#[test]
fn test_dependency_hashes_empty() {
    let deps = DependencyHashes::new();
    assert!(deps.is_empty());
    assert!(deps.hash().is_empty());
}

#[test]
fn test_dependency_hashes_add() {
    let mut deps = DependencyHashes::new();
    deps.add("gcc", "sha256:abc123");
    deps.add("make", "sha256:def456");

    assert!(!deps.is_empty());
    assert_eq!(deps.packages.len(), 2);
}

#[test]
fn test_dependency_hashes_deterministic() {
    let mut deps1 = DependencyHashes::new();
    deps1.add("gcc", "sha256:abc");
    deps1.add("make", "sha256:def");

    let mut deps2 = DependencyHashes::new();
    // Add in different order
    deps2.add("make", "sha256:def");
    deps2.add("gcc", "sha256:abc");

    // Should produce same hash regardless of insertion order
    assert_eq!(deps1.hash(), deps2.hash());
}

#[test]
fn test_cache_key_with_deps() {
    let temp = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp.path().to_path_buf(),
        ..Default::default()
    };
    let cache = BuildCache::new(config).unwrap();

    let recipe = make_test_recipe("test", "1.0.0");
    let toolchain = ToolchainInfo::default();

    // Key without deps
    let key_no_deps = cache.cache_key(&recipe, &toolchain);

    // Key with deps
    let mut deps = DependencyHashes::new();
    deps.add("gcc", "sha256:abc123");
    let key_with_deps = cache.cache_key_with_deps(&recipe, &toolchain, Some(&deps));

    // Keys should be different
    assert_ne!(key_no_deps, key_with_deps);
}

#[test]
fn test_cache_key_with_deps_changes_on_dep_update() {
    let temp = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp.path().to_path_buf(),
        ..Default::default()
    };
    let cache = BuildCache::new(config).unwrap();

    let recipe = make_test_recipe("test", "1.0.0");
    let toolchain = ToolchainInfo::default();

    // First build with gcc version A
    let mut deps1 = DependencyHashes::new();
    deps1.add("gcc", "sha256:version_a");
    let key1 = cache.cache_key_with_deps(&recipe, &toolchain, Some(&deps1));

    // Second build with gcc version B (updated)
    let mut deps2 = DependencyHashes::new();
    deps2.add("gcc", "sha256:version_b");
    let key2 = cache.cache_key_with_deps(&recipe, &toolchain, Some(&deps2));

    // Keys should differ because gcc content changed
    assert_ne!(key1, key2);
}

#[test]
fn test_cache_key_without_deps_matches_exact_no_dependency_contract() {
    let temp = TempDir::new().unwrap();
    let config = CacheConfig {
        cache_dir: temp.path().to_path_buf(),
        ..Default::default()
    };
    let cache = BuildCache::new(config).unwrap();

    let recipe = make_test_recipe("test", "1.0.0");
    let toolchain = ToolchainInfo::default();

    // The convenience path is the exact no-dependency-key contract.
    let key1 = cache.cache_key(&recipe, &toolchain);
    let key2 = cache.cache_key_with_deps(&recipe, &toolchain, None);

    assert_eq!(key1, key2);
}
