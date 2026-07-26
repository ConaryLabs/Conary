// conary-core/tests/canonical.rs

//! Integration tests for canonical package identity system
//!
//! These tests exercise the full pipeline: schema setup, canonical package
//! creation, implementation registration, distro pinning, and resolution.

use conary_core::Error;
use conary_core::canonical::rules::parse_contract;
use conary_core::db::models::{
    CanonicalMappingAuthority, CanonicalPackage, DistroPin, PackageImplementation,
};
use conary_core::repository::resolution_policy::{DependencyMixingPolicy, ResolutionPolicy};
use conary_core::resolver::canonical::CanonicalResolver;
use rusqlite::Connection;
use tempfile::NamedTempFile;

fn setup_test_db() -> (NamedTempFile, Connection) {
    let temp = NamedTempFile::new().unwrap();
    let conn = Connection::open(temp.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    (temp, conn)
}

// ---------------------------------------------------------------------------
// Full resolution pipeline
// ---------------------------------------------------------------------------

#[test]
fn test_full_canonical_resolution_pinned() {
    let (_t, conn) = setup_test_db();

    let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
    let apache_id = pkg.insert(&conn).unwrap();

    PackageImplementation::new(
        apache_id,
        "fedora-44".into(),
        "httpd".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();
    PackageImplementation::new(
        apache_id,
        "ubuntu-26.04".into(),
        "apache2".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();
    PackageImplementation::new(
        apache_id,
        "arch".into(),
        "apache".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();

    DistroPin::set(&conn, "ubuntu-26.04", DependencyMixingPolicy::Guarded).unwrap();

    let resolver = CanonicalResolver::new(&conn);

    let candidates = resolver.expand("apache-httpd").unwrap();
    assert_eq!(candidates.len(), 3);

    let ranked = resolver.rank_candidates(&candidates).unwrap();
    assert_eq!(ranked[0].distro, "ubuntu-26.04");
    assert_eq!(ranked[0].distro_name, "apache2");
}

#[test]
fn test_unpinned_multi_profile_resolution_is_ambiguous_despite_affinity() {
    let (_t, conn) = setup_test_db();

    let mut pkg = CanonicalPackage::new("curl".into(), "package".into());
    let curl_id = pkg.insert(&conn).unwrap();

    PackageImplementation::new(
        curl_id,
        "fedora-44".into(),
        "curl".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();
    PackageImplementation::new(
        curl_id,
        "ubuntu-26.04".into(),
        "curl".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();

    conn.execute(
        "INSERT INTO system_affinity (distro, package_count, percentage, updated_at) \
         VALUES ('fedora-44', 80, 80.0, '2026-03-05')",
        [],
    )
    .unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let candidates = resolver.expand("curl").unwrap();
    let error = resolver
        .select_candidate_with_policy(&candidates, &ResolutionPolicy::new())
        .unwrap_err();
    assert!(matches!(error, Error::AmbiguousPackageSelection { .. }));
}

// ---------------------------------------------------------------------------
// Distro-name -> canonical reverse lookup
// ---------------------------------------------------------------------------

#[test]
fn test_distro_name_resolves_through_canonical() {
    let (_t, conn) = setup_test_db();

    let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
    let cid = pkg.insert(&conn).unwrap();

    PackageImplementation::new(
        cid,
        "fedora-44".into(),
        "httpd".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();
    PackageImplementation::new(
        cid,
        "ubuntu-26.04".into(),
        "apache2".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let candidates = resolver.expand("httpd").unwrap();
    assert_eq!(candidates.len(), 2);
}

// ---------------------------------------------------------------------------
// Group resolution
// ---------------------------------------------------------------------------

#[test]
fn test_group_resolution() {
    let (_t, conn) = setup_test_db();

    let mut pkg = CanonicalPackage::new("dev-tools".into(), "group".into());
    let group_id = pkg.insert(&conn).unwrap();

    PackageImplementation::new(
        group_id,
        "ubuntu-26.04".into(),
        "build-essential".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();
    PackageImplementation::new(
        group_id,
        "fedora-44".into(),
        "@development-tools".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();
    PackageImplementation::new(
        group_id,
        "arch".into(),
        "base-devel".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();

    DistroPin::set(&conn, "fedora-44", DependencyMixingPolicy::Guarded).unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let candidates = resolver.expand("dev-tools").unwrap();
    let ranked = resolver.rank_candidates(&candidates).unwrap();
    assert_eq!(ranked[0].distro_name, "@development-tools");
}

// ---------------------------------------------------------------------------
// Conflict detection
// ---------------------------------------------------------------------------

#[test]
fn test_conflicts_between_canonical_equivalents() {
    let (_t, conn) = setup_test_db();

    let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
    let cid = pkg.insert(&conn).unwrap();

    PackageImplementation::new(
        cid,
        "fedora-44".into(),
        "httpd".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();
    PackageImplementation::new(
        cid,
        "ubuntu-26.04".into(),
        "apache2".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let conflicts = resolver.get_conflicts("httpd").unwrap();
    assert!(conflicts.contains(&"apache2".to_string()));
    assert!(!conflicts.contains(&"httpd".to_string()));
}

#[test]
fn test_no_conflicts_for_unknown_package() {
    let (_t, conn) = setup_test_db();
    let resolver = CanonicalResolver::new(&conn);
    let conflicts = resolver.get_conflicts("nonexistent").unwrap();
    assert!(conflicts.is_empty());
}

// ---------------------------------------------------------------------------
// Exact canonical-map contract
// ---------------------------------------------------------------------------

#[test]
fn exact_contract_preserves_literal_profile_package_mapping() {
    let yaml = r#"
version: 1
mappings:
  - canonical: apache-httpd
    package: httpd
    profiles: fedora-44
  - canonical: apache-httpd
    package: apache2
    profiles: ubuntu-26.04
"#;
    let contract = parse_contract(yaml).unwrap();
    let mappings = contract.mappings().collect::<Vec<_>>();
    assert_eq!(mappings.len(), 2);
    assert_eq!(mappings[0].canonical, "apache-httpd");
    assert_eq!(mappings[0].package, "httpd");
    assert_eq!(
        mappings[0].profiles.iter().collect::<Vec<_>>(),
        ["fedora-44"]
    );
}

// ---------------------------------------------------------------------------
// Multiple canonical packages stay independent
// ---------------------------------------------------------------------------

#[test]
fn test_independent_canonical_packages() {
    let (_t, conn) = setup_test_db();

    let mut curl_pkg = CanonicalPackage::new("curl".into(), "package".into());
    let curl_id = curl_pkg.insert(&conn).unwrap();
    PackageImplementation::new(
        curl_id,
        "fedora-44".into(),
        "curl".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();

    let mut wget_pkg = CanonicalPackage::new("wget".into(), "package".into());
    let wget_id = wget_pkg.insert(&conn).unwrap();
    PackageImplementation::new(
        wget_id,
        "fedora-44".into(),
        "wget".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert_or_verify(&conn)
    .unwrap();

    let resolver = CanonicalResolver::new(&conn);

    // Each resolves independently
    let curl_cands = resolver.expand("curl").unwrap();
    assert_eq!(curl_cands.len(), 1);
    assert_eq!(curl_cands[0].distro_name, "curl");

    let wget_cands = resolver.expand("wget").unwrap();
    assert_eq!(wget_cands.len(), 1);
    assert_eq!(wget_cands[0].distro_name, "wget");

    // No cross-conflicts
    let conflicts = resolver.get_conflicts("curl").unwrap();
    assert!(!conflicts.contains(&"wget".to_string()));
}
