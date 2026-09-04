// apps/remi/src/server/canonical_job/tests.rs

use super::*;
use conary_core::db::schema;
use rusqlite::Connection;
use tempfile::TempDir;

fn create_test_db(dir: &TempDir) -> std::path::PathBuf {
    let db_path = dir.path().join("conary.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();

    // Insert a test repository
    conn.execute(
        "INSERT INTO repositories (name, url, enabled, source_profile)
         VALUES ('fedora-44', 'https://example.com/fedora', 1, 'fedora-44')",
        [],
    )
    .unwrap();

    let repo_id: i64 = conn
        .query_row(
            "SELECT id FROM repositories WHERE name = 'fedora-44'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    // Insert test packages
    conn.execute(
        "INSERT INTO repository_packages
            (repository_id, name, version, checksum, size, download_url, version_scheme)
         VALUES
            (?1, 'curl', '8.5.0', 'abc123', 1000, 'https://example.com/curl.rpm', 'rpm')",
        [repo_id],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO repository_packages
            (repository_id, name, version, checksum, size, download_url, version_scheme)
         VALUES
            (?1, 'wget', '1.21', 'def456', 2000, 'https://example.com/wget.rpm', 'rpm')",
        [repo_id],
    )
    .unwrap();

    // Insert a second repository with the same package names. Similarity
    // must never create canonical mapping authority.
    conn.execute(
        "INSERT INTO repositories (name, url, enabled, source_profile)
         VALUES ('arch', 'https://example.com/arch', 1, 'arch')",
        [],
    )
    .unwrap();

    let repo2_id: i64 = conn
        .query_row("SELECT id FROM repositories WHERE name = 'arch'", [], |r| {
            r.get(0)
        })
        .unwrap();

    conn.execute(
        "INSERT INTO repository_packages
            (repository_id, name, version, checksum, size, download_url, version_scheme)
         VALUES
            (?1, 'curl', '8.5.0', 'abc124', 1000, 'https://example.com/curl.pkg.tar.zst', 'arch')",
        [repo2_id],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO repository_packages
            (repository_id, name, version, checksum, size, download_url, version_scheme)
         VALUES
            (?1, 'wget', '1.21', 'def457', 2000, 'https://example.com/wget.pkg.tar.zst', 'arch')",
        [repo2_id],
    )
    .unwrap();

    db_path
}

#[test]
fn test_bump_map_revision() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();

    let v1 = bump_map_revision(&conn).unwrap();
    assert_eq!(v1, 1);

    let v2 = bump_map_revision(&conn).unwrap();
    assert_eq!(v2, 2);
}

#[test]
fn bump_map_revision_rejects_corrupt_persisted_state() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    set_metadata(
        &conn,
        MetadataTable::Server,
        "canonical_map_revision",
        "not-a-version",
    )
    .unwrap();

    let error = bump_map_revision(&conn).unwrap_err();
    assert!(error.to_string().contains("invalid canonical map revision"));
}

#[test]
fn test_rebuild_canonical_map_empty_db() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("conary.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    drop(conn);

    let config = CanonicalSection {
        rules_dir: dir.path().join("rules").to_string_lossy().to_string(),
        ..Default::default()
    };
    let count = rebuild_canonical_map(&db_path, &config, &DatabaseWriter::default()).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn repository_similarity_does_not_create_canonical_authority() {
    let dir = TempDir::new().unwrap();
    let db_path = create_test_db(&dir);
    let config = CanonicalSection {
        rules_dir: dir.path().join("rules").to_string_lossy().to_string(),
        ..Default::default()
    };

    let count = rebuild_canonical_map(&db_path, &config, &DatabaseWriter::default()).unwrap();
    assert_eq!(count, 0);

    let conn = conary_core::db::open(&db_path).unwrap();
    assert!(
        CanonicalPackage::find_by_name(&conn, "curl")
            .unwrap()
            .is_none(),
        "matching package names across repositories are discovery evidence, not mapping authority"
    );
    assert!(
        CanonicalPackage::find_by_name(&conn, "wget")
            .unwrap()
            .is_none(),
        "matching package names across repositories are discovery evidence, not mapping authority"
    );
}

#[test]
fn test_rebuild_canonical_map_with_exact_contract() {
    let dir = TempDir::new().unwrap();
    let db_path = create_test_db(&dir);

    let rules_dir = dir.path().join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();
    std::fs::write(
        rules_dir.join("01-rename.yaml"),
        "version: 1\nmappings:\n  - canonical: curl-tools\n    package: curl\n    profiles: fedora-44\n",
    )
    .unwrap();

    let config = CanonicalSection {
        rules_dir: rules_dir.to_string_lossy().to_string(),
        ..Default::default()
    };
    let count = rebuild_canonical_map(&db_path, &config, &DatabaseWriter::default()).unwrap();
    assert!(count > 0);

    // Verify the exact contract took effect
    let conn = conary_core::db::open(&db_path).unwrap();
    let pkg = conary_core::db::models::CanonicalPackage::find_by_name(&conn, "curl-tools").unwrap();
    assert!(
        pkg.is_some(),
        "exact contract should create 'curl-tools' canonical entry"
    );
}

#[test]
fn configured_invalid_contract_is_a_hard_error() {
    let dir = TempDir::new().unwrap();
    let rules_dir = dir.path().join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();
    std::fs::write(rules_dir.join("invalid.yaml"), "mappings: [").unwrap();

    let conn = Connection::open_in_memory().unwrap();
    schema::ensure_current(&conn).unwrap();

    let error = phase_exact_contract(&conn, &rules_dir).unwrap_err();
    assert!(
        error.to_string().contains("YAML parse error"),
        "configured mappings are an authority contract and parse failures must abort: {error}"
    );
}

#[test]
fn mapping_and_revision_failure_roll_back_together() {
    let dir = TempDir::new().unwrap();
    let rules_dir = dir.path().join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();
    std::fs::write(
        rules_dir.join("contract.yaml"),
        "version: 1\nmappings:\n  - canonical: curl\n    package: curl\n    profiles: fedora-44\n",
    )
    .unwrap();
    let conn = Connection::open_in_memory().unwrap();
    schema::ensure_current(&conn).unwrap();
    set_metadata(
        &conn,
        MetadataTable::Server,
        "canonical_map_revision",
        "invalid",
    )
    .unwrap();

    let error = phase_exact_contract(&conn, &rules_dir).unwrap_err();
    assert!(error.to_string().contains("invalid canonical map revision"));
    assert!(
        CanonicalPackage::find_by_name(&conn, "curl")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        get_metadata(&conn, MetadataTable::Server, "last_canonical_rebuild").unwrap(),
        None
    );
}

#[test]
fn exact_contract_rebuild_removes_retired_contract_authority() {
    let dir = TempDir::new().unwrap();
    let rules_dir = dir.path().join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();
    let contract_path = rules_dir.join("contract.yaml");
    std::fs::write(
        &contract_path,
        "version: 1\nmappings:\n  - canonical: curl\n    package: curl\n    profiles: fedora-44\n",
    )
    .unwrap();
    let conn = Connection::open_in_memory().unwrap();
    schema::ensure_current(&conn).unwrap();

    phase_exact_contract(&conn, &rules_dir).unwrap();
    let curl = CanonicalPackage::find_by_name(&conn, "curl")
        .unwrap()
        .unwrap();
    assert!(
        PackageImplementation::find_for_distro(&conn, curl.id.unwrap(), "fedora-44")
            .unwrap()
            .is_some()
    );

    std::fs::write(&contract_path, "version: 1\nmappings: []\n").unwrap();
    phase_exact_contract(&conn, &rules_dir).unwrap();
    assert!(
        CanonicalPackage::find_by_name(&conn, "curl")
            .unwrap()
            .is_none()
    );
}

#[test]
fn repology_cache_never_creates_canonical_authority() {
    let conn = Connection::open_in_memory().unwrap();
    schema::ensure_current(&conn).unwrap();

    conary_core::db::models::RepologyCacheEntry::insert_or_replace(
        &conn,
        &conary_core::db::models::RepologyCacheEntry {
            project_name: "python".into(),
            distro: "fedora-44".into(),
            distro_name: "python3".into(),
            version: Some("3.12.0".into()),
            status: Some("newest".into()),
            fetched_at: "2026-03-19T00:00:00Z".into(),
        },
    )
    .unwrap();
    conary_core::db::models::RepologyCacheEntry::insert_or_replace(
        &conn,
        &conary_core::db::models::RepologyCacheEntry {
            project_name: "python".into(),
            distro: "arch".into(),
            distro_name: "python".into(),
            version: Some("3.12.0".into()),
            status: Some("newest".into()),
            fetched_at: "2026-03-19T00:00:00Z".into(),
        },
    )
    .unwrap();

    assert!(
        CanonicalPackage::find_by_name(&conn, "python")
            .unwrap()
            .is_none(),
        "Repology co-occurrence is discovery metadata, not equivalence authority"
    );
}

#[test]
fn test_phase_appstream_enriches_existing() {
    let conn = Connection::open_in_memory().unwrap();
    schema::ensure_current(&conn).unwrap();

    // Create a canonical package with an implementation
    let mut pkg = CanonicalPackage::new("firefox".into(), "package".into());
    let can_id = pkg.insert(&conn).unwrap();
    let mut imp = PackageImplementation::new(
        can_id,
        "fedora-44".into(),
        "firefox".into(),
        CanonicalMappingAuthority::Contract,
    );
    imp.insert(&conn).unwrap();

    // Insert AppStream entry matching the implementation
    AppstreamCacheEntry::insert_or_replace(
        &conn,
        &AppstreamCacheEntry {
            appstream_id: "org.mozilla.firefox".into(),
            pkgname: "firefox".into(),
            display_name: Some("Firefox".into()),
            summary: Some("Web Browser".into()),
            distro: "fedora-44".into(),
            fetched_at: "2026-03-19T00:00:00Z".into(),
        },
    )
    .unwrap();

    let count = phase_appstream_enrichment(&conn).unwrap();
    assert_eq!(count, 1);

    let updated = CanonicalPackage::find_by_name(&conn, "firefox")
        .unwrap()
        .unwrap();
    assert_eq!(updated.appstream_id.as_deref(), Some("org.mozilla.firefox"));
}

#[test]
fn appstream_enrichment_uses_exact_distro_identity() {
    let conn = Connection::open_in_memory().unwrap();
    schema::ensure_current(&conn).unwrap();

    let mut fedora_pkg = CanonicalPackage::new("fedora-editor".into(), "package".into());
    let fedora_id = fedora_pkg.insert(&conn).unwrap();
    let mut fedora_imp = PackageImplementation::new(
        fedora_id,
        "fedora-44".into(),
        "editor".into(),
        CanonicalMappingAuthority::Contract,
    );
    fedora_imp.insert(&conn).unwrap();

    let mut arch_pkg = CanonicalPackage::new("arch-editor".into(), "package".into());
    let arch_id = arch_pkg.insert(&conn).unwrap();
    let mut arch_imp = PackageImplementation::new(
        arch_id,
        "arch".into(),
        "editor".into(),
        CanonicalMappingAuthority::Contract,
    );
    arch_imp.insert(&conn).unwrap();

    AppstreamCacheEntry::insert_or_replace(
        &conn,
        &AppstreamCacheEntry {
            appstream_id: "org.example.Editor".into(),
            pkgname: "editor".into(),
            display_name: Some("Editor".into()),
            summary: None,
            distro: "fedora-44".into(),
            fetched_at: "2026-03-19T00:00:00Z".into(),
        },
    )
    .unwrap();

    let count = phase_appstream_enrichment(&conn).unwrap();
    assert_eq!(count, 1);

    let fedora = CanonicalPackage::find_by_name(&conn, "fedora-editor")
        .unwrap()
        .unwrap();
    let arch = CanonicalPackage::find_by_name(&conn, "arch-editor")
        .unwrap()
        .unwrap();
    assert_eq!(fedora.appstream_id.as_deref(), Some("org.example.Editor"));
    assert_eq!(
        arch.appstream_id, None,
        "same package name in another distro is not AppStream identity authority"
    );
}

#[test]
fn appstream_without_exact_mapping_remains_discovery_only() {
    let conn = Connection::open_in_memory().unwrap();
    schema::ensure_current(&conn).unwrap();
    AppstreamCacheEntry::insert_or_replace(
        &conn,
        &AppstreamCacheEntry {
            appstream_id: "org.example.Unmapped".into(),
            pkgname: "unmapped".into(),
            display_name: Some("Unmapped".into()),
            summary: None,
            distro: "fedora-44".into(),
            fetched_at: "2026-03-19T00:00:00Z".into(),
        },
    )
    .unwrap();

    assert_eq!(phase_appstream_enrichment(&conn).unwrap(), 0);
    assert!(
        CanonicalPackage::find_by_name(&conn, "unmapped")
            .unwrap()
            .is_none()
    );
}

#[test]
fn appstream_cannot_silently_replace_existing_exact_id() {
    let conn = Connection::open_in_memory().unwrap();
    schema::ensure_current(&conn).unwrap();
    let mut package = CanonicalPackage::new("firefox".into(), "package".into());
    package.appstream_id = Some("org.mozilla.Firefox".into());
    let canonical_id = package.insert(&conn).unwrap();
    PackageImplementation::new(
        canonical_id,
        "fedora-44".into(),
        "firefox".into(),
        CanonicalMappingAuthority::Contract,
    )
    .insert(&conn)
    .unwrap();
    AppstreamCacheEntry::insert_or_replace(
        &conn,
        &AppstreamCacheEntry {
            appstream_id: "org.example.NotFirefox".into(),
            pkgname: "firefox".into(),
            display_name: None,
            summary: None,
            distro: "fedora-44".into(),
            fetched_at: "2026-07-26T00:00:00Z".into(),
        },
    )
    .unwrap();

    let error = phase_appstream_enrichment(&conn).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("already has AppStream ID 'org.mozilla.Firefox'")
    );
}
