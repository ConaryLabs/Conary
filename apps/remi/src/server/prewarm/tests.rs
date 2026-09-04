// apps/remi/src/server/prewarm/tests.rs

use super::*;
use crate::server::catalog_authority::test_support::{
    ActiveCatalogFixture, package as catalog_package,
};
use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};

#[test]
fn test_prewarm_result_serialization() {
    let result = PrewarmResult {
        packages_processed: 10,
        packages_converted: 8,
        packages_skipped: 1,
        packages_failed: 1,
        total_bytes: 1024 * 1024,
        converted: vec!["nginx-1.24.0".to_string(), "curl-8.0.0".to_string()],
        failed: vec![PrewarmFailure {
            package: "broken-1.0.0".to_string(),
            failure: ConversionFailure::Publication {
                detail: "Download failed".to_string(),
            },
        }],
    };

    let json = serde_json::to_string_pretty(&result).unwrap();
    assert!(json.contains("nginx-1.24.0"));
    assert!(json.contains("packages_converted"));

    let parsed: PrewarmResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.packages_converted, 8);
}

#[test]
fn test_popularity_data_parsing() {
    let json = r#"[
        {"name": "nginx", "score": 1000},
        {"name": "curl", "score": 800},
        {"name": "vim", "score": 500}
    ]"#;

    let data: Vec<PackagePopularity> = serde_json::from_str(json).unwrap();
    assert_eq!(data.len(), 3);
    assert_eq!(data[0].name, "nginx");
    assert_eq!(data[0].score, 1000);
}

#[test]
fn test_merge_popularity_upstream_only() {
    use conary_core::db::schema;
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();

    // Write a temporary popularity file
    let pop_file = NamedTempFile::new().unwrap();
    let pop_data = r#"[
        {"name": "nginx", "score": 1000},
        {"name": "curl", "score": 800}
    ]"#;
    std::fs::write(pop_file.path(), pop_data).unwrap();

    let result = merge_popularity(&conn, Some(pop_file.path().to_str().unwrap()));
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].name, "nginx");
    assert_eq!(result[0].score, 1000);
    assert_eq!(result[1].name, "curl");
    assert_eq!(result[1].score, 800);
}

#[test]
fn prewarm_selection_reads_the_active_catalog_without_operational_packages() {
    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "catalog-only",
            "1.0",
            "",
            Some("x86_64"),
            3,
            "catalog-selection",
        )],
    );
    let conn = fixture.connection();
    let pinned = fixture
        .authority()
        .open_active_profile("fedora-44")
        .unwrap();
    assert_eq!(pinned.profile_revision_sha256(), revision);

    let config = PrewarmConfig {
        db_path: fixture.db_path().display().to_string(),
        chunk_dir: fixture
            .db_path()
            .with_extension("chunks")
            .display()
            .to_string(),
        cache_dir: fixture
            .db_path()
            .with_extension("cache")
            .display()
            .to_string(),
        repository_keys_dir: None,
        distro: "fedora".to_string(),
        max_packages: 10,
        popularity_file: None,
        pattern: None,
        dry_run: false,
    };
    let packages = get_packages_to_convert(&pinned, &conn, &config).unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name, "catalog-only");
}

#[test]
fn prewarm_rebuilds_stale_rows_and_skips_current_rows() {
    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate(
        "fedora-44",
        1,
        vec![
            catalog_package("fedora-44", "pkg", "1.0", "", Some("x86_64"), 3, "one"),
            catalog_package("fedora-44", "pkg", "2.0", "", Some("x86_64"), 3, "two"),
        ],
    );
    let conn = fixture.connection();

    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut stale = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        revision.clone(),
        "pkg".to_string(),
        "1.0".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:pkg-1.0-source".to_string(),
        &transport,
        3,
        "sha256:pkg-1.0-content".to_string(),
        "/tmp/pkg-1.0.ccs".to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    stale.conversion_version = CONVERSION_VERSION - 1;
    stale.insert_with_conversion_pin(&conn, 1).unwrap();

    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut current = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        revision.clone(),
        "pkg".to_string(),
        "2.0".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:pkg-2.0-source".to_string(),
        &transport,
        3,
        "sha256:pkg-2.0-content".to_string(),
        "/tmp/pkg-2.0.ccs".to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    current.insert_with_conversion_pin(&conn, 1).unwrap();

    assert_eq!(
        existing_conversion_state(&conn, "pkg", "1.0", Some("x86_64"), &revision).unwrap(),
        ExistingConversionState::MissingOrStale
    );
    assert_eq!(
        existing_conversion_state(&conn, "pkg", "2.0", Some("x86_64"), &revision).unwrap(),
        ExistingConversionState::Current
    );
}

#[test]
fn prewarm_cache_hits_require_the_exact_durable_conversion_pin() {
    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "pkg",
            "1.0",
            "",
            Some("x86_64"),
            3,
            "pin",
        )],
    );
    let conn = fixture.connection();
    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        revision.clone(),
        "pkg".to_string(),
        "1.0".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:pkg-source".to_string(),
        &transport,
        3,
        "sha256:pkg-content".to_string(),
        "/tmp/pkg.ccs".to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    let converted_id = converted.insert_with_conversion_pin(&conn, 1).unwrap();
    conn.execute(
        "DELETE FROM remi_profile_revision_pins WHERE owner_kind = 'conversion' AND owner_identity = ?1",
        [converted_id.to_string()],
    )
    .unwrap();

    let error =
        existing_conversion_state(&conn, "pkg", "1.0", Some("x86_64"), &revision).unwrap_err();
    assert!(
        error.to_string().contains("no exact profile-revision pin"),
        "{error}"
    );
}

#[test]
fn installed_conversion_identity_is_not_a_prewarm_cache_key() {
    use conary_core::db::models::{ConvertedPackage, Trove, TroveType};

    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "pkg",
            "1.0",
            "",
            Some("x86_64"),
            3,
            "installed",
        )],
    );
    let conn = fixture.connection();

    let mut trove = Trove::new(
        "pkg".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut installed = ConvertedPackage::new_installed(
        trove_id,
        "rpm".to_string(),
        "sha256:installed".to_string(),
    );
    installed.insert(&conn).unwrap();

    assert_eq!(
        existing_conversion_state(&conn, "pkg", "1.0", Some("x86_64"), &revision).unwrap(),
        ExistingConversionState::MissingOrStale
    );
}

#[test]
fn prewarm_conversion_lookup_propagates_database_errors() {
    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "pkg",
            "1.0",
            "",
            Some("x86_64"),
            3,
            "database-error",
        )],
    );
    let conn = fixture.connection();
    conn.execute("DROP TABLE converted_packages", []).unwrap();

    let error =
        existing_conversion_state(&conn, "pkg", "1.0", Some("x86_64"), &revision).unwrap_err();
    assert!(error.to_string().contains("converted_packages"), "{error}");
}

#[test]
fn test_merge_popularity_local_only() {
    use conary_core::db::models::{DownloadCount, DownloadStat};
    use conary_core::db::schema;
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();

    // Insert some download stats
    let events = vec![
        DownloadStat::new("fedora-44".into(), "vim".into()),
        DownloadStat::new("fedora-44".into(), "vim".into()),
        DownloadStat::new("fedora-44".into(), "vim".into()),
        DownloadStat::new("fedora-44".into(), "git".into()),
    ];
    DownloadStat::insert_batch(&conn, &events).unwrap();
    DownloadCount::refresh_aggregates(&conn).unwrap();

    let result = merge_popularity(&conn, None);
    assert_eq!(result.len(), 2);
    // vim has 3 downloads * 10 = 30 score, git has 1 * 10 = 10
    assert_eq!(result[0].name, "vim");
    assert_eq!(result[0].score, 30);
    assert_eq!(result[1].name, "git");
    assert_eq!(result[1].score, 10);
}

#[test]
fn test_merge_popularity_combined() {
    use conary_core::db::models::{DownloadCount, DownloadStat};
    use conary_core::db::schema;
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();

    // Upstream: nginx=1000, curl=800
    let pop_file = NamedTempFile::new().unwrap();
    let pop_data = r#"[
        {"name": "nginx", "score": 1000},
        {"name": "curl", "score": 800}
    ]"#;
    std::fs::write(pop_file.path(), pop_data).unwrap();

    // Local: curl downloaded 5 times (5*10=50 boost), vim 2 times (2*10=20)
    let events = vec![
        DownloadStat::new("fedora-44".into(), "curl".into()),
        DownloadStat::new("fedora-44".into(), "curl".into()),
        DownloadStat::new("fedora-44".into(), "curl".into()),
        DownloadStat::new("fedora-44".into(), "curl".into()),
        DownloadStat::new("fedora-44".into(), "curl".into()),
        DownloadStat::new("fedora-44".into(), "vim".into()),
        DownloadStat::new("fedora-44".into(), "vim".into()),
    ];
    DownloadStat::insert_batch(&conn, &events).unwrap();
    DownloadCount::refresh_aggregates(&conn).unwrap();

    let result = merge_popularity(&conn, Some(pop_file.path().to_str().unwrap()));

    // Expected: nginx=1000, curl=800+50=850, vim=20
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].name, "nginx");
    assert_eq!(result[0].score, 1000);
    assert_eq!(result[1].name, "curl");
    assert_eq!(result[1].score, 850);
    assert_eq!(result[2].name, "vim");
    assert_eq!(result[2].score, 20);
}

#[test]
fn test_merge_popularity_no_data() {
    use conary_core::db::schema;
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();

    let result = merge_popularity(&conn, None);
    assert!(result.is_empty());
}
