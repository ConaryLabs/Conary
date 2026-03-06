//! Integration tests for canonical package identity system
//!
//! These tests exercise the full pipeline: schema setup, canonical package
//! creation, implementation registration, distro pinning, and resolution.

use conary_core::canonical::rules::{parse_rules, RulesEngine};
use conary_core::db::models::{
    CanonicalPackage, DistroPin, PackageImplementation, PackageOverride,
};
use conary_core::resolver::canonical::CanonicalResolver;
use rusqlite::Connection;
use tempfile::NamedTempFile;

fn setup_test_db() -> (NamedTempFile, Connection) {
    let temp = NamedTempFile::new().unwrap();
    let conn = Connection::open(temp.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();
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

    PackageImplementation::new(apache_id, "fedora-41".into(), "httpd".into(), "curated".into())
        .insert_or_ignore(&conn)
        .unwrap();
    PackageImplementation::new(
        apache_id,
        "ubuntu-noble".into(),
        "apache2".into(),
        "curated".into(),
    )
    .insert_or_ignore(&conn)
    .unwrap();
    PackageImplementation::new(apache_id, "arch".into(), "apache".into(), "curated".into())
        .insert_or_ignore(&conn)
        .unwrap();

    DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();

    let resolver = CanonicalResolver::new(&conn);

    let candidates = resolver.expand("apache-httpd").unwrap();
    assert_eq!(candidates.len(), 3);

    let ranked = resolver.rank_candidates(&candidates).unwrap();
    assert_eq!(ranked[0].distro, "ubuntu-noble");
    assert_eq!(ranked[0].distro_name, "apache2");
}

#[test]
fn test_full_canonical_resolution_unpinned_affinity() {
    let (_t, conn) = setup_test_db();

    let mut pkg = CanonicalPackage::new("curl".into(), "package".into());
    let curl_id = pkg.insert(&conn).unwrap();

    PackageImplementation::new(curl_id, "fedora-41".into(), "curl".into(), "auto".into())
        .insert_or_ignore(&conn)
        .unwrap();
    PackageImplementation::new(curl_id, "ubuntu-noble".into(), "curl".into(), "auto".into())
        .insert_or_ignore(&conn)
        .unwrap();

    conn.execute(
        "INSERT INTO system_affinity (distro, package_count, percentage, updated_at) \
         VALUES ('fedora-41', 80, 80.0, '2026-03-05')",
        [],
    )
    .unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let candidates = resolver.expand("curl").unwrap();
    let ranked = resolver.rank_candidates(&candidates).unwrap();
    assert_eq!(ranked[0].distro, "fedora-41");
}

// ---------------------------------------------------------------------------
// Distro-name -> canonical reverse lookup
// ---------------------------------------------------------------------------

#[test]
fn test_distro_name_resolves_through_canonical() {
    let (_t, conn) = setup_test_db();

    let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
    let cid = pkg.insert(&conn).unwrap();

    PackageImplementation::new(cid, "fedora-41".into(), "httpd".into(), "curated".into())
        .insert_or_ignore(&conn)
        .unwrap();
    PackageImplementation::new(
        cid,
        "ubuntu-noble".into(),
        "apache2".into(),
        "curated".into(),
    )
    .insert_or_ignore(&conn)
    .unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let candidates = resolver.expand("httpd").unwrap();
    assert_eq!(candidates.len(), 2);
}

// ---------------------------------------------------------------------------
// Package overrides
// ---------------------------------------------------------------------------

#[test]
fn test_package_override_bypasses_pin() {
    let (_t, conn) = setup_test_db();

    let mut pkg = CanonicalPackage::new("mesa".into(), "package".into());
    let mesa_id = pkg.insert(&conn).unwrap();

    PackageImplementation::new(
        mesa_id,
        "fedora-41".into(),
        "mesa-fedora".into(),
        "auto".into(),
    )
    .insert_or_ignore(&conn)
    .unwrap();
    PackageImplementation::new(
        mesa_id,
        "ubuntu-noble".into(),
        "mesa-ubuntu".into(),
        "auto".into(),
    )
    .insert_or_ignore(&conn)
    .unwrap();

    DistroPin::set(&conn, "ubuntu-noble", "strict").unwrap();
    PackageOverride::set(&conn, mesa_id, "fedora-41", Some("want newer Mesa")).unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let override_distro = resolver.get_override(mesa_id).unwrap();
    assert_eq!(override_distro.as_deref(), Some("fedora-41"));
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
        "ubuntu-noble".into(),
        "build-essential".into(),
        "curated".into(),
    )
    .insert_or_ignore(&conn)
    .unwrap();
    PackageImplementation::new(
        group_id,
        "fedora-41".into(),
        "@development-tools".into(),
        "curated".into(),
    )
    .insert_or_ignore(&conn)
    .unwrap();
    PackageImplementation::new(
        group_id,
        "arch".into(),
        "base-devel".into(),
        "curated".into(),
    )
    .insert_or_ignore(&conn)
    .unwrap();

    DistroPin::set(&conn, "fedora-41", "guarded").unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let candidates = resolver.expand("dev-tools").unwrap();
    let ranked = resolver.rank_candidates(&candidates).unwrap();
    assert_eq!(ranked[0].distro_name, "@development-tools");
}

// ---------------------------------------------------------------------------
// Mixing policy enforcement
// ---------------------------------------------------------------------------

#[test]
fn test_mixing_policy_enforcement() {
    let (_t, conn) = setup_test_db();

    // Strict mode rejects cross-distro
    DistroPin::set(&conn, "ubuntu-noble", "strict").unwrap();
    let resolver = CanonicalResolver::new(&conn);
    assert!(resolver.check_mixing_policy("fedora-41").is_err());

    // Guarded mode warns
    DistroPin::set_mixing_policy(&conn, "guarded").unwrap();
    let result = resolver.check_mixing_policy("fedora-41").unwrap();
    assert!(result.has_warning());

    // Permissive mode: no warning
    DistroPin::set_mixing_policy(&conn, "permissive").unwrap();
    let result = resolver.check_mixing_policy("fedora-41").unwrap();
    assert!(!result.has_warning());

    // No pin = no policy
    DistroPin::remove(&conn).unwrap();
    let result = resolver.check_mixing_policy("fedora-41").unwrap();
    assert!(!result.has_warning());
}

// ---------------------------------------------------------------------------
// Conflict detection
// ---------------------------------------------------------------------------

#[test]
fn test_conflicts_between_canonical_equivalents() {
    let (_t, conn) = setup_test_db();

    let mut pkg = CanonicalPackage::new("apache-httpd".into(), "package".into());
    let cid = pkg.insert(&conn).unwrap();

    PackageImplementation::new(cid, "fedora-41".into(), "httpd".into(), "curated".into())
        .insert_or_ignore(&conn)
        .unwrap();
    PackageImplementation::new(
        cid,
        "ubuntu-noble".into(),
        "apache2".into(),
        "curated".into(),
    )
    .insert_or_ignore(&conn)
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
// Rules engine (YAML parsing + resolution)
// ---------------------------------------------------------------------------

#[test]
fn test_rules_engine_exact_name() {
    let yaml = r#"
rules:
  - name: httpd
    setname: apache-httpd
  - name: apache2
    setname: apache-httpd
"#;
    let rules = parse_rules(yaml).unwrap();
    let engine = RulesEngine::new(rules).unwrap();

    assert_eq!(engine.resolve("httpd", None), Some("apache-httpd".into()));
    assert_eq!(engine.resolve("apache2", None), Some("apache-httpd".into()));
    assert_eq!(engine.resolve("nginx", None), None);
}

#[test]
fn test_rules_engine_regex_pattern() {
    let yaml = r#"
rules:
  - namepat: "^lib.+-dev$"
    setname: dev-library
"#;
    let rules = parse_rules(yaml).unwrap();
    let engine = RulesEngine::new(rules).unwrap();

    // Regex matches any lib*-dev package
    assert_eq!(
        engine.resolve("libssl-dev", None),
        Some("dev-library".into())
    );
    assert_eq!(
        engine.resolve("libcurl-dev", None),
        Some("dev-library".into())
    );
    // Non-matching name returns None
    assert_eq!(engine.resolve("curl", None), None);
}

#[test]
fn test_rules_engine_repo_filter() {
    let yaml = r#"
rules:
  - name: python3
    repo: fedora_41
    setname: python
  - name: python3
    setname: python3-generic
"#;
    let rules = parse_rules(yaml).unwrap();
    let engine = RulesEngine::new(rules).unwrap();

    assert_eq!(
        engine.resolve("python3", Some("fedora_41")),
        Some("python".into())
    );
    assert_eq!(
        engine.resolve("python3", Some("ubuntu_noble")),
        Some("python3-generic".into())
    );
}

#[test]
fn test_rules_engine_first_match_wins() {
    let yaml = r#"
rules:
  - name: curl
    setname: curl-first
  - name: curl
    setname: curl-second
"#;
    let rules = parse_rules(yaml).unwrap();
    let engine = RulesEngine::new(rules).unwrap();
    assert_eq!(engine.resolve("curl", None), Some("curl-first".into()));
}

// ---------------------------------------------------------------------------
// End-to-end: rules engine + DB + resolver
// ---------------------------------------------------------------------------

#[test]
fn test_end_to_end_rules_then_resolve() {
    let (_t, conn) = setup_test_db();

    // Step 1: Map distro name to canonical via rules engine
    let yaml = r#"
rules:
  - name: httpd
    setname: apache-httpd
  - name: apache2
    setname: apache-httpd
"#;
    let rules = parse_rules(yaml).unwrap();
    let engine = RulesEngine::new(rules).unwrap();

    let canonical_name = engine.resolve("httpd", None).unwrap();
    assert_eq!(canonical_name, "apache-httpd");

    // Step 2: Populate canonical DB
    let mut pkg = CanonicalPackage::new(canonical_name.clone(), "package".into());
    let cid = pkg.insert(&conn).unwrap();

    PackageImplementation::new(cid, "fedora-41".into(), "httpd".into(), "curated".into())
        .insert_or_ignore(&conn)
        .unwrap();
    PackageImplementation::new(
        cid,
        "ubuntu-noble".into(),
        "apache2".into(),
        "curated".into(),
    )
    .insert_or_ignore(&conn)
    .unwrap();

    // Step 3: Pin to ubuntu and resolve
    DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let candidates = resolver.expand(&canonical_name).unwrap();
    let ranked = resolver.rank_candidates(&candidates).unwrap();

    assert_eq!(ranked[0].distro, "ubuntu-noble");
    assert_eq!(ranked[0].distro_name, "apache2");

    // Step 4: Verify mixing policy
    let mix = resolver.check_mixing_policy("fedora-41").unwrap();
    assert!(mix.has_warning());
}

// ---------------------------------------------------------------------------
// Multiple canonical packages stay independent
// ---------------------------------------------------------------------------

#[test]
fn test_independent_canonical_packages() {
    let (_t, conn) = setup_test_db();

    let mut curl_pkg = CanonicalPackage::new("curl".into(), "package".into());
    let curl_id = curl_pkg.insert(&conn).unwrap();
    PackageImplementation::new(curl_id, "fedora-41".into(), "curl".into(), "auto".into())
        .insert_or_ignore(&conn)
        .unwrap();

    let mut wget_pkg = CanonicalPackage::new("wget".into(), "package".into());
    let wget_id = wget_pkg.insert(&conn).unwrap();
    PackageImplementation::new(wget_id, "fedora-41".into(), "wget".into(), "auto".into())
        .insert_or_ignore(&conn)
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

// ---------------------------------------------------------------------------
// Override removal restores normal pin behaviour
// ---------------------------------------------------------------------------

#[test]
fn test_override_removal() {
    let (_t, conn) = setup_test_db();

    let mut pkg = CanonicalPackage::new("mesa".into(), "package".into());
    let mesa_id = pkg.insert(&conn).unwrap();

    PackageOverride::set(&conn, mesa_id, "fedora-41", None).unwrap();

    let resolver = CanonicalResolver::new(&conn);
    assert!(resolver.get_override(mesa_id).unwrap().is_some());

    PackageOverride::remove(&conn, mesa_id).unwrap();
    assert!(resolver.get_override(mesa_id).unwrap().is_none());
}
