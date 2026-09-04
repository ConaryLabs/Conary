// apps/remi/src/server/search/tests.rs

use super::*;
use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
use crate::server::native_publish::test_support::seed_native_publication;
use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};
use tempfile::TempDir;

fn create_test_engine() -> (TempDir, SearchEngine) {
    let dir = TempDir::new().unwrap();
    let engine = SearchEngine::new(dir.path()).unwrap();
    (dir, engine)
}

fn insert_stale_conversion(
    conn: &rusqlite::Connection,
    source_profile: &str,
    profile_revision_sha256: &str,
    package: &str,
    version: &str,
) {
    let transport = crate::server::conversion::test_support::test_transport(&[format!(
        "sha256:{package}-{version}-chunk"
    )]);
    let mut converted = ConvertedPackage::new_repository(
        source_profile.to_string(),
        profile_revision_sha256.to_string(),
        package.to_string(),
        version.to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        format!("sha256:{package}-{version}-source"),
        &transport,
        42,
        format!("sha256:{package}-{version}-content"),
        format!("/tmp/{package}-{version}.ccs"),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    converted.conversion_version = CONVERSION_VERSION - 1;
    converted.insert(conn).unwrap();
}

#[test]
fn test_index_and_search() {
    let (_dir, engine) = create_test_engine();

    let pkg = PackageSearchDoc {
        name: "nginx".to_string(),
        version: "1.24.0".to_string(),
        release: None,
        distro: "fedora".to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("High performance HTTP server and reverse proxy".to_string()),
        requirement_terms: Some("openssl pcre2 zlib".to_string()),
        size: 1_200_000,
        converted: true,
        source_kind: None,
    };
    engine.index_package(&pkg).unwrap();

    let pkg2 = PackageSearchDoc {
        name: "curl".to_string(),
        version: "8.5.0".to_string(),
        release: None,
        distro: "fedora".to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("Command line tool for transferring data".to_string()),
        requirement_terms: Some("openssl nghttp2 zlib".to_string()),
        size: 500_000,
        converted: false,
        source_kind: None,
    };
    engine.index_package(&pkg2).unwrap();

    // Search for nginx
    let results = engine.search("nginx", None, 10).unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].name, "nginx");
    assert_eq!(results[0].distro, "fedora");
    assert!(results[0].converted);

    // Search for HTTP - should find nginx via description
    let results = engine.search("HTTP server", None, 10).unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].name, "nginx");

    // Search with distro filter
    let results = engine.search("nginx", Some("fedora"), 10).unwrap();
    assert!(!results.is_empty());

    let results = engine.search("nginx", Some("arch"), 10).unwrap();
    assert!(results.is_empty());
}

#[test]
fn search_rebuild_preserves_native_release_identity_and_converted_false() {
    let (_dir, engine) = create_test_engine();
    let fixture = ActiveCatalogFixture::new();
    fixture.activate("fedora-44", 1, Vec::new());
    let conn = fixture.connection();
    seed_native_publication(
        &conn,
        "fedora",
        "hello",
        "1.0.0",
        "1",
        "noarch",
        "/tmp/hello-1.ccs",
    );
    seed_native_publication(
        &conn,
        "fedora",
        "hello",
        "1.0.0",
        "2",
        "noarch",
        "/tmp/hello-2.ccs",
    );
    fixture.activate_universe(1);
    let crate::server::public_universe::PublicUniverseLoadOutcome::Current(universe) =
        PublicUniverseSnapshot::load(fixture.db_path()).unwrap()
    else {
        panic!("fixture public universe is not current")
    };

    engine
        .rebuild_from_universe(fixture.db_path(), fixture.authority(), &universe)
        .unwrap();
    let results = engine.search("hello", Some("fedora"), 10).unwrap();

    assert_eq!(
        results
            .iter()
            .filter(|result| result.name == "hello")
            .count(),
        2
    );
    assert!(results.iter().all(|result| !result.converted));
}

#[test]
fn failed_candidate_reader_reload_preserves_previous_search_projection() {
    let fixture = ActiveCatalogFixture::new();
    fixture.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "baseline-package",
            "1.0",
            "1",
            Some("x86_64"),
            42,
            "baseline-source",
        )],
    );
    fixture.activate_universe(1);
    let crate::server::public_universe::PublicUniverseLoadOutcome::Current(universe) =
        PublicUniverseSnapshot::load(fixture.db_path()).unwrap()
    else {
        panic!("fixture public universe is not current")
    };
    let identity = universe.identity().clone();
    let (_dir, engine) = create_test_engine();
    engine
        .rebuild_from_universe(fixture.db_path(), fixture.authority(), &universe)
        .unwrap();
    assert_eq!(
        engine
            .search_public_universe(&identity, "baseline-package", Some("fedora"), 10)
            .unwrap()
            .len(),
        1
    );

    let conn = fixture.connection();
    seed_native_publication(
        &conn,
        "fedora",
        "candidate-only-package",
        "2.0",
        "1",
        "x86_64",
        "/tmp/candidate-only.ccs",
    );
    drop(conn);
    let error = engine
        .rebuild_from_universe_with_reader_reload(
            fixture.db_path(),
            fixture.authority(),
            &universe,
            |_| anyhow::bail!("injected candidate reader reload failure"),
        )
        .expect_err("candidate reader reload should fail");
    assert!(error.to_string().contains("injected candidate reader"));

    assert_eq!(
        engine
            .search_public_universe(&identity, "baseline-package", Some("fedora"), 10)
            .unwrap()
            .len(),
        1,
        "the previously authorized reader must remain live"
    );
    assert!(
        engine
            .search_public_universe(&identity, "candidate-only-package", Some("fedora"), 10,)
            .unwrap()
            .is_empty(),
        "a failed candidate reader must never become searchable"
    );
}

#[test]
fn test_suggest() {
    let (_dir, engine) = create_test_engine();

    for name in &["nginx", "nginx-module-njs", "nmap", "nodejs", "nano"] {
        let pkg = PackageSearchDoc {
            name: (*name).to_string(),
            version: "1.0.0".to_string(),
            release: None,
            distro: "fedora".to_string(),
            architecture: Some("x86_64".to_string()),
            description: None,
            requirement_terms: None,
            size: 0,
            converted: false,
            source_kind: None,
        };
        engine.index_package(&pkg).unwrap();
    }

    // Prefix "ngi" should match nginx*
    let suggestions = engine.suggest("ngi", 10).unwrap();
    assert!(!suggestions.is_empty());
    assert!(suggestions.iter().all(|s| s.starts_with("ngi")));

    // Prefix "n" should match multiple
    let suggestions = engine.suggest("n", 10).unwrap();
    assert!(suggestions.len() >= 2);

    // Empty prefix returns nothing
    let suggestions = engine.suggest("", 10).unwrap();
    assert!(suggestions.is_empty());
}

#[test]
fn test_update_existing_package() {
    let (_dir, engine) = create_test_engine();

    let pkg = PackageSearchDoc {
        name: "vim".to_string(),
        version: "9.0".to_string(),
        release: None,
        distro: "arch".to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("Vi Improved".to_string()),
        requirement_terms: None,
        size: 2_000_000,
        converted: false,
        source_kind: None,
    };
    engine.index_package(&pkg).unwrap();

    // Update with the same full identity
    let pkg_updated = PackageSearchDoc {
        name: "vim".to_string(),
        version: "9.0".to_string(),
        release: None,
        distro: "arch".to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("Vi Improved - text editor".to_string()),
        requirement_terms: None,
        size: 2_100_000,
        converted: true,
        source_kind: None,
    };
    engine.index_package(&pkg_updated).unwrap();

    let results = engine.search("vim", None, 10).unwrap();
    // Should have the updated document for the exact identity.
    assert!(!results.is_empty());
    assert!(results[0].converted);
}

#[test]
fn search_rebuild_marks_stale_rows_unconverted() {
    let fixture = ActiveCatalogFixture::new();
    let profile_revision = fixture.activate(
        "fedora-44",
        9,
        vec![package(
            "fedora-44",
            "gtk3",
            "3.24.0",
            "1",
            Some("x86_64"),
            1024,
            "gtk3-source",
        )],
    );
    let conn = fixture.connection();
    insert_stale_conversion(&conn, "fedora-44", &profile_revision, "gtk3", "3.24.0");
    drop(conn);
    fixture.activate_universe(1);
    let crate::server::public_universe::PublicUniverseLoadOutcome::Current(universe) =
        PublicUniverseSnapshot::load(fixture.db_path()).unwrap()
    else {
        panic!("fixture public universe is not current")
    };

    let (_dir, engine) = create_test_engine();
    engine
        .rebuild_from_universe(fixture.db_path(), fixture.authority(), &universe)
        .unwrap();

    let results = engine.search("gtk3", Some("fedora"), 10).unwrap();

    assert_eq!(results.len(), 1);
    assert!(!results[0].converted);
}

#[test]
fn test_regex_escape() {
    assert_eq!(regex_escape("hello"), "hello");
    assert_eq!(regex_escape("lib++"), "lib\\+\\+");
    assert_eq!(regex_escape("foo.bar"), "foo\\.bar");
    assert_eq!(regex_escape("test[0]"), "test\\[0\\]");
}
