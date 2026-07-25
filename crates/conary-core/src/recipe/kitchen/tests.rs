// conary-core/src/recipe/kitchen/tests.rs

use super::*;
use crate::hash;
use crate::recipe::CacheConfig;
use crate::recipe::format::{
    BuildSection, LocalSourceSection, PackageSection, RemoteSourceSection, SourceSection,
};
use crate::recipe::hermetic::{
    BuilderEnvironmentKind, CiMode, DivergenceStatus, HermeticBuildInput, HostBuildRecord,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use tempfile::tempdir;

fn test_signing_authority() -> CcsPackageSigningAuthority {
    CcsPackageSigningAuthority::new(
        crate::ccs::SigningKeyPair::generate().with_key_id("kitchen-test"),
    )
}

fn make_test_recipe(makedepends: &[&str]) -> Recipe {
    Recipe {
        package: PackageSection {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
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
            version: "1.0.0".to_string(),
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

fn host_build_record(output_merkle_root: &str) -> HostBuildRecord {
    HostBuildRecord {
        package_name: "hermetic-local".to_string(),
        package_version: "1.0.0".to_string(),
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

fn cook_hermetic_or_skip_unprivileged_builder(
    kitchen: &Kitchen,
    recipe: &Recipe,
    input: HermeticBuildInput,
    output_dir: &Path,
) -> Option<CookResult> {
    match kitchen.cook_hermetic(recipe, input, output_dir, CiMode::Off) {
        Ok(result) => Some(result),
        Err(error) if unprivileged_hermetic_builder_error(&error) => {
            eprintln!(
                "skipping hermetic kitchen assertion on a host without builder setup privileges"
            );
            None
        }
        Err(error) => panic!("{error}"),
    }
}

fn unprivileged_hermetic_builder_error(error: &Error) -> bool {
    let text = error.to_string();
    text.contains("setup phase failed")
        && (text.contains("RTNETLINK answers: Operation not permitted")
            || text.contains("sethostname failed: Operation not permitted")
            || text.contains("mount --make-rprivate failed"))
}

#[test]
fn unprivileged_hermetic_builder_error_detects_ci_network_setup_failure() {
    let error = Error::IoError(
            "setup phase failed with exit code 127\nstderr: RTNETLINK answers: Operation not permitted\n"
                .to_string(),
        );

    assert!(unprivileged_hermetic_builder_error(&error));
}

fn copy_tool_with_runtime_deps(tool: &Path, sysroot: &Path, target_relative: &Path) {
    copy_host_file_into_sysroot(tool, sysroot, target_relative);
    for dependency in ldd_paths(tool) {
        copy_host_file_into_sysroot(&dependency, sysroot, dependency.strip_prefix("/").unwrap());
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
fn test_fetch_source_verifies_upstream_md5_checksum() {
    let cache = tempdir().unwrap();
    let checksum = "md5:d41d8cd98f00b204e9800998ecf8427e";
    let cached_path = cache.path().join(source_cache_key(checksum));
    fs::write(&cached_path, b"").unwrap();

    let kitchen = Kitchen::new(KitchenConfig {
        source_cache: cache.path().to_path_buf(),
        ..KitchenConfig::default()
    });

    let resolved = kitchen
        .fetch_source("https://example.invalid/test.tar.gz", checksum)
        .unwrap();
    assert_eq!(resolved, cached_path);
}

#[test]
fn test_fetch_source_rejects_md5_mismatch() {
    let cache = tempdir().unwrap();
    let source = tempdir().unwrap();
    let source_path = source.path().join("source.tar.gz");
    fs::write(&source_path, b"").unwrap();
    let checksum = "md5:098f6bcd4621d373cade4e832627b4f6";

    let kitchen = Kitchen::new(KitchenConfig {
        source_cache: cache.path().to_path_buf(),
        ..KitchenConfig::default()
    });

    let error = kitchen
        .fetch_source(source_path.to_str().unwrap(), checksum)
        .unwrap_err();

    assert!(matches!(error, Error::ChecksumMismatch { .. }));
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
        ccs_signing_authority: Some(test_signing_authority()),
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

    let Some(result) =
        cook_hermetic_or_skip_unprivileged_builder(&kitchen, &recipe, input, &output_dir)
    else {
        return;
    };

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
        ccs_signing_authority: Some(test_signing_authority()),
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

    let Some(result) =
        cook_hermetic_or_skip_unprivileged_builder(&kitchen, &recipe, input, &output_dir)
    else {
        return;
    };

    let provenance = result.provenance.unwrap();
    let evidence = provenance.hermetic_evidence.unwrap();
    assert_eq!(
        evidence.divergence.status,
        DivergenceStatus::DiffersFromHost
    );
    assert!(evidence.divergence.compared);
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

#[test]
fn cook_cached_requires_the_configured_signing_authority() {
    let dir = tempdir().unwrap();
    let payload = dir.path().join("payload");
    let cache_dir = dir.path().join("cache");
    let trusted_output = dir.path().join("trusted-output");
    let untrusted_output = dir.path().join("untrusted-output");
    fs::create_dir_all(payload.join("usr/share/test")).unwrap();
    fs::create_dir_all(&trusted_output).unwrap();
    fs::create_dir_all(&untrusted_output).unwrap();
    fs::write(payload.join("usr/share/test/data"), b"cached\n").unwrap();

    let manifest = crate::ccs::manifest::CcsManifest::new_minimal("test", "1.0.0");
    let build = crate::ccs::builder::CcsBuilder::new(manifest, &payload)
        .build()
        .unwrap();
    let package_path = dir.path().join("cached.ccs");
    let signer = crate::ccs::SigningKeyPair::generate().with_key_id("kitchen-cache-test");
    crate::ccs::builder::write_signed_current_ccs_package(&build, &package_path, &signer, false)
        .unwrap();

    let recipe = make_test_recipe(&[]);
    let toolchain = ToolchainInfo::default();
    let cache = BuildCache::new(CacheConfig {
        cache_dir,
        ..Default::default()
    })
    .unwrap();
    cache.put(&recipe, &toolchain, &package_path).unwrap();

    let trusted = Kitchen::new(KitchenConfig {
        ccs_signing_authority: Some(CcsPackageSigningAuthority::from_key_pair(&signer)),
        ..KitchenConfig::default()
    });
    let result = trusted
        .cook_cached(&recipe, &trusted_output, &cache, &toolchain)
        .unwrap();
    assert!(result.from_cache);
    assert!(result.package_path.exists());

    let untrusted = Kitchen::new(KitchenConfig {
        ccs_signing_authority: Some(CcsPackageSigningAuthority::new(
            crate::ccs::SigningKeyPair::generate(),
        )),
        ..KitchenConfig::default()
    });
    let error = untrusted
        .cook_cached(&recipe, &untrusted_output, &cache, &toolchain)
        .unwrap_err();
    assert!(
        error.to_string().contains("configured authority"),
        "{error}"
    );
    assert!(!untrusted_output.join("test-1.0.0-1.ccs").exists());
}
