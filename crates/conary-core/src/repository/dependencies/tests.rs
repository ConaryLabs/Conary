// conary-core/src/repository/dependencies/tests.rs

use super::*;
use crate::db::models::{Repository, RepositoryPackage, RepositoryProvide};
use crate::db::schema;
use crate::repository::resolution_policy::ResolutionPolicy;
use crate::repository::versioning::VersionScheme;
use rusqlite::Connection;

fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;",
    )
    .unwrap();
    schema::ensure_current(&conn).unwrap();
    conn
}

#[test]
fn does_not_resolve_soname_dependency_from_package_name_guess() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "libjq".to_string(),
        "1.8.1-1.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:test".to_string(),
        123,
        "https://example.invalid/libjq.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();

    let err = resolve_repo_dependency_request(
        &conn,
        "libjq.so.1",
        &VersionConstraint::Any,
        &SelectionOptions::default(),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("Required dependency 'libjq.so.1' not found"),
        "{err}"
    );
}

#[test]
fn does_not_resolve_soname_dependency_by_search_stem() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    for name in ["oniguruma", "oniguruma-devel", "rust-onig-devel"] {
        let mut pkg = RepositoryPackage::new(
            repo_id,
            name.to_string(),
            "6.9.10-3.fc44".to_string(),
            crate::repository::versioning::VersionScheme::Rpm,
            format!("sha256:{name}"),
            123,
            format!("https://example.invalid/{name}.rpm"),
        );
        pkg.insert(&conn).unwrap();
    }

    let err = resolve_repo_dependency_request(
        &conn,
        "libonig.so.5",
        &VersionConstraint::Any,
        &SelectionOptions::default(),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("Required dependency 'libonig.so.5' not found"),
        "{err}"
    );
}

#[test]
fn does_not_scan_legacy_json_metadata_for_capability_providers() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "kernel-core".to_string(),
        "6.19.6-200.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:test".to_string(),
        123,
        "https://example.invalid/kernel-core.rpm".to_string(),
    );
    pkg.metadata = Some(
        serde_json::json!({
            "rpm_provides": ["kernel-core-uname-r = 6.19.6-200.fc44.x86_64"]
        })
        .to_string(),
    );
    pkg.insert(&conn).unwrap();

    let error = resolve_repo_dependency_request(
        &conn,
        "kernel-core-uname-r",
        &VersionConstraint::parse("= 6.19.6-200.fc44.x86_64").unwrap(),
        &SelectionOptions::default(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Required dependency 'kernel-core-uname-r' not found"),
        "{error}"
    );
}

#[test]
fn resolves_capability_dependency_from_normalized_repo_provides() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "kernel-core".to_string(),
        "6.19.6-200.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:test".to_string(),
        123,
        "https://example.invalid/kernel-core.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let repo_package_id = pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        repo_package_id,
        "kernel-core-uname-r".to_string(),
        Some("6.19.6-200.fc44.x86_64".to_string()),
        "package".to_string(),
        Some("kernel-core-uname-r = 6.19.6-200.fc44.x86_64".to_string()),
        VersionScheme::Rpm,
    );
    provide.insert(&conn).unwrap();

    let (resolved, constraint) = resolve_repo_dependency_request(
        &conn,
        "kernel-core-uname-r",
        &VersionConstraint::parse("= 6.19.6-200.fc44.x86_64").unwrap(),
        &SelectionOptions::default(),
    )
    .unwrap();
    assert_eq!(resolved, "kernel-core");
    assert_eq!(
        constraint,
        VersionConstraint::parse("= 6.19.6-200.fc44").unwrap()
    );
}

#[test]
fn resolve_dependency_requests_finds_direct_packages_without_transitive_expansion() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    // Package A depends on B (but we only resolve A, not transitively)
    let mut pkg_a = RepositoryPackage::new(
        repo_id,
        "pkg-a".to_string(),
        "1.0-1.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:a".to_string(),
        100,
        "https://example.invalid/pkg-a.rpm".to_string(),
    );
    pkg_a.insert(&conn).unwrap();

    let mut pkg_b = RepositoryPackage::new(
        repo_id,
        "pkg-b".to_string(),
        "2.0-1.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:b".to_string(),
        200,
        "https://example.invalid/pkg-b.rpm".to_string(),
    );
    pkg_b.insert(&conn).unwrap();

    let requests = vec![
        ("pkg-a".to_string(), VersionConstraint::Any),
        ("pkg-b".to_string(), VersionConstraint::Any),
    ];

    let result =
        resolve_dependency_requests(&conn, &requests, &SelectionOptions::default()).unwrap();

    // Both should be found (non-transitive: just the requested packages)
    assert_eq!(result.len(), 2);
    let names: Vec<&str> = result.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"pkg-a"));
    assert!(names.contains(&"pkg-b"));
}

#[test]
fn direct_repository_requests_do_not_guess_satisfaction_from_package_name() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "already-here".to_string(),
        "1.0-1.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:ah".to_string(),
        100,
        "https://example.invalid/already-here.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();

    // Install a trove with the same name
    conn.execute(
        "INSERT INTO troves (
             name, version, type, install_source, install_reason, version_scheme
         ) VALUES (
             'already-here', '1.0-1.fc44', 'package', 'repository', 'explicit', 'rpm'
         )",
        [],
    )
    .unwrap();

    let requests = vec![("already-here".to_string(), VersionConstraint::Any)];
    let result =
        resolve_dependency_requests(&conn, &requests, &SelectionOptions::default()).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].1.package.name, "already-here");
}

#[test]
fn dependency_requests_apply_explicit_source_policy() {
    let conn = test_db();

    for (repo_name, version) in [("allowed", "1.0-1"), ("excluded", "9.0-1")] {
        let mut repo = Repository::new(
            repo_name.to_string(),
            format!("https://{repo_name}.example.invalid"),
        );
        repo.insert(&conn).unwrap();
        let mut package = RepositoryPackage::new(
            repo.id.unwrap(),
            "policy-dep".to_string(),
            version.to_string(),
            crate::repository::versioning::VersionScheme::Rpm,
            format!("sha256:{repo_name}"),
            100,
            format!("https://{repo_name}.example.invalid/policy-dep.rpm"),
        );
        package.insert(&conn).unwrap();
    }

    let options = SelectionOptions {
        policy: Some(ResolutionPolicy {
            allowed_distros: vec!["allowed".to_string()],
            ..ResolutionPolicy::default()
        }),
        ..SelectionOptions::default()
    };
    let requests = vec![("policy-dep".to_string(), VersionConstraint::Any)];
    let resolved = resolve_dependency_requests(&conn, &requests, &options).unwrap();

    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].1.repository.name, "allowed");
    assert_eq!(resolved[0].1.package.version, "1.0-1");
}

#[test]
fn resolve_dependency_requests_deduplicates_capabilities_to_same_package() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "kmod".to_string(),
        "34.2-2.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:kmod".to_string(),
        100,
        "https://example.invalid/kmod.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    for capability in ["libkmod.so.2()(64bit)", "libkmod.so.2(LIBKMOD_22)(64bit)"] {
        let mut provide = RepositoryProvide::new(
            pkg_id,
            capability.to_string(),
            None,
            "soname".to_string(),
            Some(capability.to_string()),
            VersionScheme::Rpm,
        );
        provide.insert(&conn).unwrap();
    }

    let requests = vec![
        ("kmod".to_string(), VersionConstraint::Any),
        ("libkmod.so.2()(64bit)".to_string(), VersionConstraint::Any),
        (
            "libkmod.so.2(LIBKMOD_22)(64bit)".to_string(),
            VersionConstraint::Any,
        ),
    ];

    let result =
        resolve_dependency_requests(&conn, &requests, &SelectionOptions::default()).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].1.package.name, "kmod");
    assert_eq!(result[0].1.package.version, "34.2-2.fc44");
}

#[test]
fn resolve_dependency_requests_requires_normalized_rpm_soname_key() {
    let conn = test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "glib2".to_string(),
        "2.86.0-2.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:glib2".to_string(),
        100,
        "https://example.invalid/glib2.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        pkg_id,
        "libglib-2.0.so.0".to_string(),
        None,
        "soname".to_string(),
        Some("libglib-2.0.so.0()(64bit)".to_string()),
        VersionScheme::Rpm,
    );
    provide.insert(&conn).unwrap();

    let raw_requests = vec![(
        "libglib-2.0.so.0()(64bit)".to_string(),
        VersionConstraint::Any,
    )];
    let err = resolve_dependency_requests(&conn, &raw_requests, &SelectionOptions::default())
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("Required dependency 'libglib-2.0.so.0()(64bit)' not found"),
        "{err}"
    );

    let normalized_requests = vec![("libglib-2.0.so.0".to_string(), VersionConstraint::Any)];
    let result =
        resolve_dependency_requests(&conn, &normalized_requests, &SelectionOptions::default())
            .unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].1.package.name, "glib2");
}

#[test]
fn resolves_rpm_provided_capability_via_normalized_provides() {
    let conn = test_db();

    let mut repo = Repository::new("fedora".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "glibc".to_string(),
        "2.39-1.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:glibc".to_string(),
        500,
        "https://example.invalid/glibc.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    // RPM-style soname provide
    let mut provide = RepositoryProvide::new(
        pkg_id,
        "libc.so.6(GLIBC_2.17)(64bit)".to_string(),
        Some("2.39".to_string()),
        "soname".to_string(),
        Some("libc.so.6(GLIBC_2.17)(64bit)".to_string()),
        VersionScheme::Rpm,
    );
    provide.insert(&conn).unwrap();

    let (resolved, _) = resolve_repo_dependency_request(
        &conn,
        "libc.so.6(GLIBC_2.17)(64bit)",
        &VersionConstraint::Any,
        &SelectionOptions::default(),
    )
    .unwrap();
    assert_eq!(resolved, "glibc");
}

#[test]
fn resolves_debian_virtual_package_via_normalized_provides() {
    let conn = test_db();

    let mut repo = Repository::new("ubuntu".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "postfix".to_string(),
        "3.8.4-1".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:postfix".to_string(),
        300,
        "https://example.invalid/postfix.deb".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    // Debian virtual package provide
    let mut provide = RepositoryProvide::new(
        pkg_id,
        "mail-transport-agent".to_string(),
        None,
        "virtual".to_string(),
        Some("mail-transport-agent".to_string()),
        VersionScheme::Debian,
    );
    provide.insert(&conn).unwrap();

    let (resolved, _) = resolve_repo_dependency_request(
        &conn,
        "mail-transport-agent",
        &VersionConstraint::Any,
        &SelectionOptions::default(),
    )
    .unwrap();
    assert_eq!(resolved, "postfix");
}

#[test]
fn resolves_arch_versioned_provide_via_normalized_provides() {
    let conn = test_db();

    let mut repo = Repository::new("arch".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "sh".to_string(),
        "5.2.37-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:sh".to_string(),
        200,
        "https://example.invalid/sh.pkg.tar.zst".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    // Arch versioned provide
    let mut provide = RepositoryProvide::new(
        pkg_id,
        "sh".to_string(),
        Some("5.2.37".to_string()),
        "package".to_string(),
        Some("sh=5.2.37".to_string()),
        VersionScheme::Arch,
    );
    provide.insert(&conn).unwrap();

    let (resolved, constraint) = resolve_repo_dependency_request(
        &conn,
        "sh",
        &VersionConstraint::Any,
        &SelectionOptions::default(),
    )
    .unwrap();
    // Direct package name match takes precedence
    assert_eq!(resolved, "sh");
    assert_eq!(constraint, VersionConstraint::parse("= 5.2.37-1").unwrap());
}

#[test]
fn no_fallback_to_name_guessing_when_normalized_provide_exists() {
    // When a normalized provide exists, resolution should use only the
    // package that declared it.
    let conn = test_db();

    let mut repo = Repository::new("fedora".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    // Two packages: one whose name resembles the soname, one that
    // actually declares the provide.
    let mut wrong_pkg = RepositoryPackage::new(
        repo_id,
        "libfoo".to_string(),
        "1.0-1.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:wrong".to_string(),
        100,
        "https://example.invalid/libfoo.rpm".to_string(),
    );
    wrong_pkg.insert(&conn).unwrap();

    let mut correct_pkg = RepositoryPackage::new(
        repo_id,
        "libfoo-compat".to_string(),
        "2.0-1.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:correct".to_string(),
        100,
        "https://example.invalid/libfoo-compat.rpm".to_string(),
    );
    correct_pkg.insert(&conn).unwrap();
    let correct_id = correct_pkg.id.unwrap();

    // Only the compat package actually provides the soname
    let mut provide = RepositoryProvide::new(
        correct_id,
        "libfoo.so.1".to_string(),
        None,
        "soname".to_string(),
        Some("libfoo.so.1".to_string()),
        VersionScheme::Rpm,
    );
    provide.insert(&conn).unwrap();

    let (resolved, _) = resolve_repo_dependency_request(
        &conn,
        "libfoo.so.1",
        &VersionConstraint::Any,
        &SelectionOptions::default(),
    )
    .unwrap();
    // Must resolve to the package that declares the provide, not the
    // one whose name merely resembles it.
    assert_eq!(resolved, "libfoo-compat");
}

#[test]
fn capability_lookup_ignores_lookalike_package_names() {
    // Verify that normalized capability lookup is the source of truth.
    let conn = test_db();

    let mut repo = Repository::new("fedora".to_string(), "https://example.invalid".into());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    // A package whose name looks related but declares no provide.
    let mut lookalike_pkg = RepositoryPackage::new(
        repo_id,
        "libssl3".to_string(),
        "3.2.0-1.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:lookalike".to_string(),
        100,
        "https://example.invalid/libssl3.rpm".to_string(),
    );
    lookalike_pkg.insert(&conn).unwrap();

    // A different package that actually provides the capability
    let mut provider_pkg = RepositoryPackage::new(
        repo_id,
        "openssl-libs".to_string(),
        "3.2.0-1.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:provider".to_string(),
        200,
        "https://example.invalid/openssl-libs.rpm".to_string(),
    );
    provider_pkg.insert(&conn).unwrap();
    let provider_id = provider_pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        provider_id,
        "libssl.so.3".to_string(),
        None,
        "soname".to_string(),
        Some("libssl.so.3()(64bit)".to_string()),
        VersionScheme::Rpm,
    );
    provide.insert(&conn).unwrap();

    let (resolved, _) = resolve_repo_dependency_request(
        &conn,
        "libssl.so.3",
        &VersionConstraint::Any,
        &SelectionOptions::default(),
    )
    .unwrap();
    // Should resolve via declared capability, not package-name resemblance.
    assert_eq!(resolved, "openssl-libs");
}
