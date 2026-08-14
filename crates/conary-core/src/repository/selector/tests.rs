// conary-core/src/repository/selector/tests.rs

use super::*;
use crate::db::models::{Repository, RepositoryPackage};
use crate::db::schema;
use crate::repository::resolution_policy::{
    DependencyMixingPolicy, RequestScope, ResolutionPolicy,
};
use rusqlite::Connection;

#[test]
fn test_detect_architecture() {
    let arch = PackageSelector::detect_architecture();
    // Should return one of the known architectures
    assert!(!arch.is_empty());
    // On most development machines, this will be x86_64
    println!("Detected architecture: {}", arch);
}

#[test]
fn test_architecture_compatibility() {
    let system_arch = "x86_64";

    // noarch is compatible with everything
    assert!(PackageSelector::is_architecture_compatible(
        VersionScheme::Rpm,
        Some("noarch"),
        system_arch
    ));

    // Exact match is compatible
    assert!(PackageSelector::is_architecture_compatible(
        VersionScheme::Rpm,
        Some("x86_64"),
        system_arch
    ));

    // Different arch is not compatible
    assert!(!PackageSelector::is_architecture_compatible(
        VersionScheme::Rpm,
        Some("aarch64"),
        system_arch
    ));

    // Missing architecture is not resolution authority.
    assert!(!PackageSelector::is_architecture_compatible(
        VersionScheme::Rpm,
        None,
        system_arch
    ));
}

#[test]
fn test_debian_amd64_compatible_with_x86_64() {
    assert!(PackageSelector::is_architecture_compatible(
        VersionScheme::Debian,
        Some("amd64"),
        "x86_64"
    ));
}

#[test]
fn test_debian_arm64_compatible_with_aarch64() {
    assert!(PackageSelector::is_architecture_compatible(
        VersionScheme::Debian,
        Some("arm64"),
        "aarch64"
    ));
}

#[test]
fn test_debian_i386_compatible_with_i686() {
    assert!(PackageSelector::is_architecture_compatible(
        VersionScheme::Debian,
        Some("i386"),
        "i686"
    ));
}

#[test]
fn architecture_equivalence_requires_both_owning_schemes() {
    assert!(package_architectures_match(
        VersionScheme::Debian,
        "amd64",
        VersionScheme::Rpm,
        "x86_64",
        "x86_64",
    ));
    assert!(!package_architectures_match(
        VersionScheme::Debian,
        "all",
        VersionScheme::Rpm,
        "aarch64",
        "x86_64",
    ));
}

#[test]
fn test_debian_all_architecture_compatible() {
    assert!(PackageSelector::is_architecture_compatible(
        VersionScheme::Debian,
        Some("all"),
        "x86_64"
    ));
}

#[test]
fn test_arch_any_architecture_compatible() {
    assert!(PackageSelector::is_architecture_compatible(
        VersionScheme::Arch,
        Some("any"),
        "x86_64"
    ));
}

#[test]
fn architecture_independent_tokens_are_not_cross_scheme_aliases() {
    assert!(!PackageSelector::is_architecture_compatible(
        VersionScheme::Rpm,
        Some("all"),
        "x86_64",
    ));
    assert!(!PackageSelector::is_architecture_compatible(
        VersionScheme::Debian,
        Some("noarch"),
        "x86_64",
    ));
    assert!(!PackageSelector::is_architecture_compatible(
        VersionScheme::Arch,
        Some("all"),
        "x86_64",
    ));
}

fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    conn
}

#[test]
fn select_best_uses_debian_version_ordering() {
    let conn = test_db();

    let mut repo = Repository::new(
        "ubuntu-noble".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    repo.priority = 10;
    repo.insert(&conn).unwrap();
    let repository = Repository::find_by_name(&conn, "ubuntu-noble")
        .unwrap()
        .unwrap();
    let repo_id = repository.id.unwrap();

    let mut prerelease = RepositoryPackage::new(
        repo_id,
        "demo".to_string(),
        "1.0~beta1".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:beta".to_string(),
        1,
        "https://archive.ubuntu.com/ubuntu/pool/demo_1.0~beta1_amd64.deb".to_string(),
    );
    prerelease.architecture = Some("amd64".to_string());
    prerelease.insert(&conn).unwrap();

    let mut stable = RepositoryPackage::new(
        repo_id,
        "demo".to_string(),
        "1.0".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:stable".to_string(),
        1,
        "https://archive.ubuntu.com/ubuntu/pool/demo_1.0_amd64.deb".to_string(),
    );
    stable.architecture = Some("amd64".to_string());
    stable.insert(&conn).unwrap();

    let candidates =
        PackageSelector::search_packages(&conn, "demo", &SelectionOptions::default()).unwrap();
    let selected = PackageSelector::select_best(candidates).unwrap();

    assert_eq!(selected.package.version, "1.0");
}

#[test]
fn policy_repo_scope_filters_root_request() {
    let conn = test_db();

    // Create two repos: fedora and ubuntu
    let mut fedora_repo = Repository::new(
        "fedora-44".to_string(),
        "https://mirrors.fedoraproject.org/metalink".to_string(),
    );
    fedora_repo.priority = 10;
    fedora_repo.insert(&conn).unwrap();
    let fedora = Repository::find_by_name(&conn, "fedora-44")
        .unwrap()
        .unwrap();

    let mut ubuntu_repo = Repository::new(
        "ubuntu-noble".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    ubuntu_repo.priority = 10;
    ubuntu_repo.insert(&conn).unwrap();
    let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
        .unwrap()
        .unwrap();

    // Add curl to both
    let mut pkg_fed = RepositoryPackage::new(
        fedora.id.unwrap(),
        "curl".into(),
        "8.9.1".into(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:fed".into(),
        1,
        "https://example.com/curl.rpm".into(),
    );
    pkg_fed.architecture = Some("x86_64".into());
    pkg_fed.insert(&conn).unwrap();

    let mut pkg_ubu = RepositoryPackage::new(
        ubuntu.id.unwrap(),
        "curl".into(),
        "8.5.0".into(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:ubu".into(),
        1,
        "https://example.com/curl.deb".into(),
    );
    pkg_ubu.architecture = Some("amd64".into());
    pkg_ubu.insert(&conn).unwrap();

    // With --repo fedora-44, root request should only find fedora
    let policy = ResolutionPolicy::new()
        .with_scope(RequestScope::Repository("fedora-44".into()))
        .with_mixing(DependencyMixingPolicy::Permissive);

    let options = SelectionOptions {
        policy: Some(policy),
        is_root: true,
        ..Default::default()
    };
    let candidates = PackageSelector::search_packages(&conn, "curl", &options).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].repository.name, "fedora-44");
}

#[test]
fn policy_repo_scope_does_not_filter_transitive_deps() {
    let conn = test_db();

    let mut fedora_repo = Repository::new(
        "fedora-44".to_string(),
        "https://mirrors.fedoraproject.org/metalink".to_string(),
    );
    fedora_repo.priority = 10;
    fedora_repo.insert(&conn).unwrap();
    let fedora = Repository::find_by_name(&conn, "fedora-44")
        .unwrap()
        .unwrap();

    let mut ubuntu_repo = Repository::new(
        "ubuntu-noble".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    ubuntu_repo.priority = 10;
    ubuntu_repo.insert(&conn).unwrap();
    let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
        .unwrap()
        .unwrap();

    let mut pkg_fed = RepositoryPackage::new(
        fedora.id.unwrap(),
        "libcurl".into(),
        "8.9.1".into(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:fed".into(),
        1,
        "https://example.com/libcurl.rpm".into(),
    );
    pkg_fed.architecture = Some("x86_64".into());
    pkg_fed.insert(&conn).unwrap();

    let mut pkg_ubu = RepositoryPackage::new(
        ubuntu.id.unwrap(),
        "libcurl".into(),
        "8.5.0".into(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:ubu".into(),
        1,
        "https://example.com/libcurl.deb".into(),
    );
    pkg_ubu.architecture = Some("amd64".into());
    pkg_ubu.insert(&conn).unwrap();

    // Request scope targets fedora, but is_root=false so scope is ignored
    let policy = ResolutionPolicy::new()
        .with_scope(RequestScope::Repository("fedora-44".into()))
        .with_mixing(DependencyMixingPolicy::Permissive);

    let options = SelectionOptions {
        policy: Some(policy),
        is_root: false,
        ..Default::default()
    };
    let candidates = PackageSelector::search_packages(&conn, "libcurl", &options).unwrap();
    assert_eq!(candidates.len(), 2, "transitive dep sees both repos");
}

#[test]
fn strict_policy_rejects_cross_profile_dep() {
    let conn = test_db();

    let mut ubuntu_repo = Repository::new(
        "ubuntu-noble".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    ubuntu_repo.priority = 10;
    ubuntu_repo.source_profile = Some("ubuntu-26.04".to_string());
    ubuntu_repo.insert(&conn).unwrap();
    let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
        .unwrap()
        .unwrap();

    let mut pkg = RepositoryPackage::new(
        ubuntu.id.unwrap(),
        "libssl3".into(),
        "3.0.13".into(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:ssl".into(),
        1,
        "https://example.com/libssl3.deb".into(),
    );
    pkg.architecture = Some("amd64".into());
    pkg.insert(&conn).unwrap();

    let policy = ResolutionPolicy::new()
        .with_mixing(DependencyMixingPolicy::Strict)
        .with_primary_source_identity("fedora-44");

    let options = SelectionOptions {
        policy: Some(policy),
        is_root: false,
        ..Default::default()
    };
    let candidates = PackageSelector::search_packages(&conn, "libssl3", &options).unwrap();
    assert!(candidates.is_empty(), "strict policy rejects cross-flavor");
}

#[test]
fn strict_policy_accepts_candidate_by_exact_repository_profile() {
    let conn = test_db();

    let mut repo = Repository::new(
        "slice-d-local-update".to_string(),
        "http://127.0.0.1:18087".to_string(),
    );
    repo.priority = 500;
    repo.source_profile = Some("ubuntu-26.04".to_string());
    repo.insert(&conn).unwrap();
    let repo = Repository::find_by_name(&conn, "slice-d-local-update")
        .unwrap()
        .unwrap();

    let mut pkg = RepositoryPackage::new(
        repo.id.unwrap(),
        "phase4-runtime-fixture".into(),
        "1.0.1".into(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:fixture".into(),
        1,
        "http://127.0.0.1:18087/phase4-runtime-fixture_1.0.1_amd64.deb".into(),
    );
    pkg.architecture = Some("amd64".into());
    pkg.source_profile = Some("ubuntu-26.04".into());
    pkg.insert(&conn).unwrap();

    let candidates = PackageSelector::search_packages(
        &conn,
        "phase4-runtime-fixture",
        &SelectionOptions {
            policy: Some(
                ResolutionPolicy::new()
                    .with_mixing(DependencyMixingPolicy::Strict)
                    .with_primary_source_identity("ubuntu-26.04"),
            ),
            is_root: false,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].repository.name, "slice-d-local-update");
}

#[test]
fn permissive_policy_allows_cross_profile_dep() {
    let conn = test_db();

    let mut ubuntu_repo = Repository::new(
        "ubuntu-noble".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    ubuntu_repo.priority = 10;
    ubuntu_repo.source_profile = Some("ubuntu-26.04".to_string());
    ubuntu_repo.insert(&conn).unwrap();
    let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
        .unwrap()
        .unwrap();

    let mut pkg = RepositoryPackage::new(
        ubuntu.id.unwrap(),
        "libssl3".into(),
        "3.0.13".into(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:ssl".into(),
        1,
        "https://example.com/libssl3.deb".into(),
    );
    pkg.architecture = Some("amd64".into());
    pkg.insert(&conn).unwrap();

    let policy = ResolutionPolicy::new()
        .with_mixing(DependencyMixingPolicy::Permissive)
        .with_primary_source_identity("fedora-44");

    let options = SelectionOptions {
        policy: Some(policy),
        is_root: false,
        ..Default::default()
    };
    let candidates = PackageSelector::search_packages(&conn, "libssl3", &options).unwrap();
    assert_eq!(candidates.len(), 1, "permissive policy allows cross-flavor");
}

#[test]
fn guarded_policy_allows_cross_profile_dep() {
    let conn = test_db();

    let mut ubuntu_repo = Repository::new(
        "ubuntu-noble".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    ubuntu_repo.priority = 10;
    ubuntu_repo.source_profile = Some("ubuntu-26.04".to_string());
    ubuntu_repo.insert(&conn).unwrap();
    let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
        .unwrap()
        .unwrap();

    let mut pkg = RepositoryPackage::new(
        ubuntu.id.unwrap(),
        "libssl3".into(),
        "3.0.13".into(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:ssl".into(),
        1,
        "https://example.com/libssl3.deb".into(),
    );
    pkg.architecture = Some("amd64".into());
    pkg.insert(&conn).unwrap();

    // Guarded policy allows cross-profile candidates.
    let policy = ResolutionPolicy::new()
        .with_mixing(DependencyMixingPolicy::Guarded)
        .with_primary_source_identity("fedora-44");

    let options = SelectionOptions {
        policy: Some(policy),
        is_root: false,
        ..Default::default()
    };
    let candidates = PackageSelector::search_packages(&conn, "libssl3", &options).unwrap();
    assert_eq!(candidates.len(), 1, "guarded policy allows cross-flavor");
}

#[test]
fn equal_priority_cross_scheme_candidates_are_typed_ambiguity() {
    let conn = test_db();

    // Create fedora and ubuntu repos at same priority
    let mut fedora_repo = Repository::new(
        "fedora-44".to_string(),
        "https://mirrors.fedoraproject.org/metalink".to_string(),
    );
    fedora_repo.priority = 10;
    fedora_repo.insert(&conn).unwrap();
    let fedora = Repository::find_by_name(&conn, "fedora-44")
        .unwrap()
        .unwrap();

    let mut ubuntu_repo = Repository::new(
        "ubuntu-noble".to_string(),
        "https://archive.ubuntu.com/ubuntu".to_string(),
    );
    ubuntu_repo.priority = 10;
    ubuntu_repo.insert(&conn).unwrap();
    let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
        .unwrap()
        .unwrap();

    let mut pkg_fed = RepositoryPackage::new(
        fedora.id.unwrap(),
        "curl".into(),
        "8.9.1-2.fc44".into(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:fed".into(),
        1,
        "https://example.com/curl.rpm".into(),
    );
    pkg_fed.architecture = Some("x86_64".into());
    pkg_fed.insert(&conn).unwrap();

    let mut pkg_ubu = RepositoryPackage::new(
        ubuntu.id.unwrap(),
        "curl".into(),
        "8.5.0-2ubuntu1".into(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:ubu".into(),
        1,
        "https://example.com/curl.deb".into(),
    );
    pkg_ubu.architecture = Some("amd64".into());
    pkg_ubu.insert(&conn).unwrap();

    // With permissive policy, both candidates are present
    let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Permissive);

    let options = SelectionOptions {
        policy: Some(policy),
        is_root: true,
        ..Default::default()
    };
    let candidates = PackageSelector::search_packages(&conn, "curl", &options).unwrap();
    assert_eq!(candidates.len(), 2);

    let error = PackageSelector::select_best(candidates.clone()).unwrap_err();
    assert!(matches!(
        error,
        Error::AmbiguousPackageSelection { ref package, .. } if package == "curl"
    ));
    assert!(error.to_string().contains("fedora-44"));
    assert!(error.to_string().contains("ubuntu-noble"));

    let mut priority_scoped = candidates;
    priority_scoped[0].repository.priority = 11;
    let selected = PackageSelector::select_best(priority_scoped).unwrap();
    assert_eq!(selected.repository.name, "fedora-44");
}

#[test]
fn repology_signal_cannot_override_exact_repository_priority() {
    let conn = test_db();
    let fresh = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('python', 'package')",
        [],
    )
    .unwrap();
    let canonical_id = conn.last_insert_rowid();

    let mut fedora_repo = Repository::new(
        "fedora-remi".to_string(),
        "https://example.invalid".to_string(),
    );
    fedora_repo.priority = 20;
    fedora_repo.source_profile = Some("fedora-44".to_string());
    fedora_repo.insert(&conn).unwrap();
    let fedora = Repository::find_by_name(&conn, "fedora-remi")
        .unwrap()
        .unwrap();

    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.invalid".to_string(),
    );
    arch_repo.priority = 5;
    arch_repo.source_profile = Some("arch".to_string());
    arch_repo.insert(&conn).unwrap();
    let arch = Repository::find_by_name(&conn, "arch-core")
        .unwrap()
        .unwrap();

    let mut fedora_pkg = RepositoryPackage::new(
        fedora.id.unwrap(),
        "python".into(),
        "3.12.2-1.fc44".into(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:fedora".into(),
        1,
        "https://example.invalid/python-fedora.rpm".into(),
    );
    fedora_pkg.canonical_id = Some(canonical_id);
    fedora_pkg.architecture = Some("x86_64".into());
    fedora_pkg.insert(&conn).unwrap();

    let mut arch_pkg = RepositoryPackage::new(
        arch.id.unwrap(),
        "python".into(),
        "3.13.0-1".into(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:arch".into(),
        1,
        "https://example.invalid/python-arch.pkg.tar.zst".into(),
    );
    arch_pkg.canonical_id = Some(canonical_id);
    arch_pkg.architecture = Some("x86_64".into());
    arch_pkg.insert(&conn).unwrap();

    crate::db::models::RepologyCacheEntry::insert_or_replace(
        &conn,
        &crate::db::models::RepologyCacheEntry {
            project_name: "python".into(),
            distro: "fedora-44".into(),
            distro_name: "python".into(),
            version: Some("3.12.2".into()),
            status: Some("outdated".into()),
            fetched_at: fresh.clone(),
        },
    )
    .unwrap();
    crate::db::models::RepologyCacheEntry::insert_or_replace(
        &conn,
        &crate::db::models::RepologyCacheEntry {
            project_name: "python".into(),
            distro: "arch".into(),
            distro_name: "python".into(),
            version: Some("3.13.0".into()),
            status: Some("newest".into()),
            fetched_at: fresh,
        },
    )
    .unwrap();

    let selected = PackageSelector::find_best_package(
        &conn,
        "python",
        &SelectionOptions {
            policy: Some(ResolutionPolicy::new()),
            is_root: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(selected.repository.name, "fedora-remi");
}

#[test]
fn permissive_policy_rejects_conflicting_package_and_repository_profiles() {
    let conn = test_db();
    let mut repository = Repository::new(
        "fedora-remi".to_string(),
        "https://example.invalid".to_string(),
    );
    repository.source_profile = Some("fedora-44".to_string());
    repository.insert(&conn).unwrap();

    let mut package = RepositoryPackage::new(
        repository.id.unwrap(),
        "python".to_string(),
        "3.12.2-1.fc44".to_string(),
        VersionScheme::Rpm,
        "sha256:conflict".to_string(),
        1,
        "https://example.invalid/python.rpm".to_string(),
    );
    package.architecture = Some("x86_64".to_string());
    package.source_profile = Some("arch".to_string());
    package.insert(&conn).unwrap();

    let error = PackageSelector::search_packages(
        &conn,
        "python",
        &SelectionOptions {
            architecture: Some("x86_64".to_string()),
            policy: Some(ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Permissive)),
            is_root: true,
            ..Default::default()
        },
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("declares source profile 'arch'"),
        "{error}"
    );
}
