// conary-core/src/resolver/sat/tests/canonical.rs

use super::formal_dependencies::insert_repo_pkg_with_reqs;
use super::*;
use crate::db::models::{CanonicalMappingAuthority, CanonicalPackage, PackageImplementation};

/// Helper: insert a canonical package with distro-specific implementations.
fn insert_canonical(
    conn: &Connection,
    canonical_name: &str,
    impls: &[(&str, &str)], // (distro, distro_name)
) {
    let mut pkg = CanonicalPackage::new(canonical_name.to_string(), "package".to_string());
    let can_id = pkg.insert(conn).unwrap();

    for (distro, distro_name) in impls {
        let mut pi = PackageImplementation::new(
            can_id,
            distro.to_string(),
            distro_name.to_string(),
            CanonicalMappingAuthority::Contract,
        );
        pi.insert(conn).unwrap();
    }
}

#[test]
fn canonical_fallback_resolves_cross_distro_dep() {
    // App depends on "libssl3" (Debian name), but only "openssl" (Fedora)
    // is available in repos. Canonical mapping links them.
    let (_dir, conn) = setup_test_db();

    insert_canonical(
        &conn,
        "openssl",
        &[("fedora-44", "openssl"), ("ubuntu-26.04", "libssl3")],
    );

    let mut repo = Repository::new(
        "fedora-main".to_string(),
        "https://mirror.fedora.invalid".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "openssl",
        "3.2.0-1.fc44",
        "https://mirror.fedora.invalid/openssl.rpm",
        "rpm",
        &[],
    );

    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "myapp",
        "1.0-1.fc44",
        "https://mirror.fedora.invalid/myapp.rpm",
        "rpm",
        &[("libssl3", None)],
    );

    let result = solve_install(&conn, &[("myapp".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(
        result.conflict_message.is_none(),
        "Canonical fallback should resolve libssl3 -> openssl: {:?}",
        result.conflict_message,
    );
    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"myapp"));
    assert!(
        names.contains(&"openssl"),
        "openssl should be pulled in via canonical fallback for libssl3"
    );
}

#[test]
fn canonical_fallback_not_used_when_exact_match_exists() {
    // When the exact name exists in repos, canonical fallback should not
    // interfere -- the direct candidate should be used.
    let (_dir, conn) = setup_test_db();

    insert_canonical(
        &conn,
        "kernel",
        &[("fedora-44", "kernel"), ("arch", "linux")],
    );

    let mut repo = Repository::new(
        "fedora-main".to_string(),
        "https://mirror.fedora.invalid".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "kernel",
        "6.19.6-200.fc44",
        "https://mirror.fedora.invalid/kernel.rpm",
        "rpm",
        &[],
    );

    // Also add "linux" in case it leaks through
    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "linux",
        "6.19.6-1",
        "https://mirror.fedora.invalid/linux.rpm",
        "rpm",
        &[],
    );

    let result = solve_install(&conn, &[("kernel".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(result.conflict_message.is_none());
    let names: Vec<&str> = result
        .install_order
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"kernel"));
    // "linux" should NOT appear -- exact name matched first
    assert!(
        !names.contains(&"linux"),
        "Canonical fallback should not fire when exact name exists"
    );
}

#[test]
fn canonical_fallback_with_no_mapping_still_fails() {
    // When no canonical mapping exists and the name is absent, the
    // solver should still report a conflict.
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new(
        "fedora-main".to_string(),
        "https://mirror.fedora.invalid".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    insert_repo_pkg_with_reqs(
        &conn,
        repo_id,
        "myapp",
        "1.0-1",
        "https://mirror.fedora.invalid/myapp.rpm",
        "rpm",
        &[("nonexistent-lib", None)],
    );

    let result = solve_install(&conn, &[("myapp".to_string(), VersionConstraint::Any)]).unwrap();

    assert!(
        result.conflict_message.is_some(),
        "Should report conflict when dep has no candidates and no canonical mapping"
    );
}

// ==========================================================================
// Task 10: Integration tests for resolver pipeline redesign
// ==========================================================================

use crate::repository::versioning::VersionScheme;
use crate::resolver::identity::PackageIdentity;
use crate::resolver::provides_index::ProvidesIndex;

#[test]
fn test_cross_distro_canonical_resolution() {
    // Two repos (fedora, debian) with same canonical_id.
    // fedora has "httpd" 2.4, debian has "apache2" 2.4.
    // Both linked to canonical "apache".
    // PackageIdentity should find httpd with its canonical_id,
    // and find_canonical_equivalents should return "apache2".
    let (_dir, conn) = setup_test_db();

    // Create canonical mapping
    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('apache', 'package')",
        [],
    )
    .unwrap();
    let canonical_id = conn.last_insert_rowid();

    // Create two repos
    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority, source_profile)
             VALUES ('fedora-44', 'https://f.com', 1, 10, 'fedora-44')",
        [],
    )
    .unwrap();
    let fed_repo = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority, source_profile)
             VALUES ('debian-bookworm', 'https://d.com', 1, 5, 'ubuntu-26.04')",
        [],
    )
    .unwrap();
    let deb_repo = conn.last_insert_rowid();

    // Insert packages with canonical links
    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'httpd', '2.4.58', 'sha256:a', 100, 'https://f.com/httpd', 'rpm', ?2)",
            rusqlite::params![fed_repo, canonical_id],
        )
        .unwrap();

    conn.execute(
        "INSERT INTO repository_packages (
                 repository_id, name, version, architecture, debian_multi_arch,
                 checksum, size, download_url, version_scheme, canonical_id
             ) VALUES (
                 ?1, 'apache2', '2.4.57', 'amd64', 'no',
                 'sha256:b', 100, 'https://d.com/apache2', 'debian', ?2
             )",
        rusqlite::params![deb_repo, canonical_id],
    )
    .unwrap();

    // Verify PackageIdentity finds httpd with canonical link
    let httpd = PackageIdentity::find_all_by_name(&conn, "httpd").unwrap();
    assert_eq!(httpd.len(), 1);
    assert_eq!(httpd[0].canonical_id, Some(canonical_id));

    // Verify canonical equivalents
    let equivs = PackageIdentity::find_canonical_equivalents(&conn, "httpd").unwrap();
    assert_eq!(equivs, vec!["apache2"]);
}

#[test]
fn test_multi_arch_candidates_coexist() {
    // Same package name with different architectures should return
    // multiple candidates from PackageIdentity::find_all_by_name.
    let (_dir, conn) = setup_test_db();

    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fedora', 'https://f.com', 1, 10)",
        [],
    )
    .unwrap();
    let repo_id = conn.last_insert_rowid();

    // Same name, different architectures
    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, architecture, checksum, size, download_url, version_scheme)
             VALUES (?1, 'glibc', '2.38', 'x86_64', 'sha256:a', 100, 'https://f.com/glibc-x64', 'rpm')",
            [repo_id],
        )
        .unwrap();

    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, architecture, checksum, size, download_url, version_scheme)
             VALUES (?1, 'glibc', '2.38', 'i686', 'sha256:b', 100, 'https://f.com/glibc-i686', 'rpm')",
            [repo_id],
        )
        .unwrap();

    let candidates = PackageIdentity::find_all_by_name(&conn, "glibc").unwrap();
    assert_eq!(candidates.len(), 2);

    let arches: Vec<_> = candidates
        .iter()
        .map(|c| c.architecture.as_deref())
        .collect();
    assert!(arches.contains(&Some("x86_64")));
    assert!(arches.contains(&Some("i686")));
}

#[test]
fn test_provides_index_cross_source() {
    // ProvidesIndex should aggregate providers from both repository_provides
    // and appstream_provides into a single unified index.
    let (_dir, conn) = setup_test_db();

    // Repo provide
    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fedora', 'https://f.com', 1, 10)",
        [],
    )
    .unwrap();
    let repo_id = conn.last_insert_rowid();
    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (?1, 'openssl-libs', '3.2', 'sha256:a', 100, 'https://f.com/x', 'rpm')",
            [repo_id],
        )
        .unwrap();
    let pkg_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO repository_provides
             (repository_package_id, capability, version, version_relation, kind, version_scheme,
              architecture_qualifier_kind)
             VALUES (?1, 'libssl.so.3', '3.2', 'eq', 'soname', 'rpm', 'implicit')",
        [pkg_id],
    )
    .unwrap();

    // AppStream provide (cross-distro)
    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('openssl', 'package')",
        [],
    )
    .unwrap();
    let canonical_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO appstream_provides (canonical_id, provide_type, capability)
             VALUES (?1, 'library', 'libcrypto.so.3')",
        [canonical_id],
    )
    .unwrap();

    let index = ProvidesIndex::build(&conn).unwrap();

    // Repo provide found
    assert_eq!(index.find_providers("libssl.so.3").len(), 1);
    // AppStream provide found
    assert_eq!(index.find_providers("libcrypto.so.3").len(), 1);
    // Unknown not found
    assert!(index.find_providers("libfoo.so.1").is_empty());
}

#[test]
fn test_version_scheme_comes_from_package_contract() {
    let (_dir, conn) = setup_test_db();

    // Repository labels do not override the package's exact version contract.
    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority, source_profile)
             VALUES ('mixed-repo', 'https://m.com', 1, 10, 'fedora-44')",
        [],
    )
    .unwrap();
    let repo_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO repository_packages (
                 repository_id, name, version, architecture, debian_multi_arch,
                 checksum, size, download_url, version_scheme
             ) VALUES (
                 ?1, 'pkg', '1.0', 'amd64', 'no',
                 'sha256:a', 100, 'https://m.com/x', 'debian'
             )",
        [repo_id],
    )
    .unwrap();

    let identities = PackageIdentity::find_all_by_name(&conn, "pkg").unwrap();
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0].version_scheme, VersionScheme::Debian);
}

#[test]
fn repology_latest_signal_cannot_override_repository_priority() {
    let (_dir, conn) = setup_test_db();
    let fresh = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('python', 'package')",
        [],
    )
    .unwrap();
    let canonical_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority, source_profile)
             VALUES ('fedora-remi', 'https://f.com', 1, 20, 'fedora-44')",
        [],
    )
    .unwrap();
    let fedora_repo_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority, source_profile)
             VALUES ('arch-core', 'https://a.com', 1, 5, 'arch')",
        [],
    )
    .unwrap();
    let arch_repo_id = conn.last_insert_rowid();

    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, architecture, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'python', '3.12.2-1.fc44', 'x86_64', 'sha256:fedora', 100, 'https://f.com/python', 'rpm', ?2)",
            rusqlite::params![fedora_repo_id, canonical_id],
        )
        .unwrap();
    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, architecture, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'python', '3.13.0-1', 'x86_64', 'sha256:arch', 100, 'https://a.com/python', 'arch', ?2)",
            rusqlite::params![arch_repo_id, canonical_id],
        )
        .unwrap();

    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, architecture, checksum, size, download_url, version_scheme)
             VALUES (?1, 'libc', '1.0-1', 'x86_64', 'sha256:libc', 100, 'https://a.com/libc', 'arch')",
            [arch_repo_id],
        )
        .unwrap();

    crate::db::models::RepologyCacheEntry::insert_or_replace(
        &conn,
        &crate::db::models::RepologyCacheEntry {
            project_name: "python".into(),
            distro: "fedora".into(),
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

    let resolution = solve_install_with_policy(
        &conn,
        &[("python".to_string(), VersionConstraint::Any)],
        &ResolutionPolicy::new()
            .with_mixing(crate::repository::resolution_policy::DependencyMixingPolicy::Permissive),
    )
    .unwrap();

    let python = resolution
        .install_order
        .iter()
        .find(|pkg| pkg.name == "python")
        .expect("python should be present in SAT install order");
    assert_eq!(python.version, "3.12.2-1.fc44");
}
