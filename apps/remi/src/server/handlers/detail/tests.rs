// apps/remi/src/server/handlers/detail/tests.rs

use super::*;
use crate::server::catalog_authority::test_support::{
    ActiveCatalogFixture, package as catalog_package,
};
use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};
use conary_core::repository::catalog::CatalogPackageRecordV1;

fn package(name: &str, version: &str, architecture: &str, marker: &str) -> CatalogPackageRecordV1 {
    let mut package = catalog_package(
        "fedora-44",
        name,
        version,
        "",
        Some(architecture),
        3,
        marker,
    );
    package.description = Some(format!("catalog description {marker}"));
    package.metadata = Some(
        serde_json::json!({
            "license": "MIT",
            "homepage": format!("https://example.invalid/{marker}")
        })
        .to_string(),
    );
    package
}

fn insert_converted(
    conn: &Connection,
    profile_revision_sha256: &str,
    name: &str,
    version: &str,
    architecture: &str,
    conversion_version: i32,
) {
    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        profile_revision_sha256.to_string(),
        name.to_string(),
        version.to_string(),
        architecture.to_string(),
        "rpm".to_string(),
        format!("sha256:source-{name}-{version}"),
        &transport,
        3,
        format!("sha256:content-{name}-{version}"),
        format!("/tmp/{name}-{version}.ccs"),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    converted.conversion_version = conversion_version;
    converted.insert_with_conversion_pin(conn, 1).unwrap();
}

fn insert_stale_conversion(
    conn: &Connection,
    profile_revision_sha256: &str,
    name: &str,
    version: &str,
    architecture: &str,
) {
    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        profile_revision_sha256.to_string(),
        name.to_string(),
        version.to_string(),
        architecture.to_string(),
        "rpm".to_string(),
        format!("sha256:source-{name}-{version}"),
        &transport,
        3,
        format!("sha256:content-{name}-{version}"),
        format!("/tmp/{name}-{version}.ccs"),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    converted.conversion_version = CONVERSION_VERSION - 1;
    converted.insert_with_conversion_pin(conn, 1).unwrap();
}

fn public_universe(fixture: &ActiveCatalogFixture) -> PublicUniverseSnapshot {
    fixture.activate_universe(1);
    PublicUniverseSnapshot::load(fixture.db_path())
        .unwrap()
        .unwrap()
}

#[test]
fn package_detail_ignores_stale_converted_rows() {
    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate(
        "fedora-44",
        1,
        vec![package("pkg", "1.0", "x86_64", "stale")],
    );
    let conn = fixture.connection();
    insert_converted(
        &conn,
        &revision,
        "pkg",
        "1.0",
        "x86_64",
        CONVERSION_VERSION - 1,
    );
    let universe = public_universe(&fixture);
    let selection = universe.profile("fedora-44").unwrap();

    let detail = query_package_detail(
        fixture.authority(),
        fixture.db_path(),
        "fedora",
        "pkg",
        selection,
    )
    .unwrap()
    .unwrap();

    assert!(!detail.converted);
    assert!(detail.versions.iter().all(|version| !version.converted));
}

#[test]
fn package_versions_require_matching_architecture_for_converted_status() {
    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate(
        "fedora-44",
        1,
        vec![package("pkg", "1.0", "aarch64", "catalog")],
    );
    let conn = fixture.connection();
    insert_converted(&conn, &revision, "pkg", "1.0", "x86_64", CONVERSION_VERSION);
    let universe = public_universe(&fixture);
    let selection = universe.profile("fedora-44").unwrap();

    let versions = query_versions(
        fixture.authority(),
        fixture.db_path(),
        "fedora",
        "pkg",
        selection,
    )
    .unwrap();

    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].architecture.as_deref(), Some("aarch64"));
    assert!(!versions[0].converted);
}

#[test]
fn overview_ignores_stale_converted_rows() {
    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate(
        "fedora-44",
        1,
        vec![
            package("stale", "1.0", "x86_64", "stale"),
            package("current", "1.0", "x86_64", "current"),
        ],
    );
    let conn = fixture.connection();
    insert_converted(
        &conn,
        &revision,
        "stale",
        "1.0",
        "x86_64",
        CONVERSION_VERSION - 1,
    );
    insert_converted(
        &conn,
        &revision,
        "current",
        "1.0",
        "x86_64",
        CONVERSION_VERSION,
    );
    let universe = public_universe(&fixture);
    let overview = query_overview(fixture.authority(), fixture.db_path(), &universe).unwrap();

    assert_eq!(overview.total_converted, 1);
}

#[test]
fn recent_packages_use_analytics_order_and_catalog_payload_without_package_rows() {
    let fixture = ActiveCatalogFixture::new();
    fixture.activate(
        "fedora-44",
        1,
        vec![
            package("recent-new", "1.0", "x86_64", "new-catalog"),
            package("recent-old", "1.0", "x86_64", "old-catalog"),
        ],
    );
    let conn = fixture.connection();
    conn.execute(
        "INSERT INTO download_stats (
             source_profile, package_name, package_version, downloaded_at
         ) VALUES ('fedora-44', 'recent-new', '1.0', '2026-08-22 02:00:00')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO download_stats (
             source_profile, package_name, package_version, downloaded_at
         ) VALUES ('fedora-44', 'recent-old', '1.0', '2026-08-22 01:00:00')",
        [],
    )
    .unwrap();
    let universe = public_universe(&fixture);

    let recent = query_recent(
        fixture.authority(),
        fixture.db_path(),
        &universe,
        Some("fedora"),
        10,
    )
    .unwrap();

    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].name, "recent-new");
    assert_eq!(
        recent[0].description.as_deref(),
        Some("catalog description new-catalog")
    );
    assert_eq!(recent[1].name, "recent-old");
    assert_eq!(
        recent[1].description.as_deref(),
        Some("catalog description old-catalog")
    );
}

#[test]
fn popular_packages_use_analytics_order_and_catalog_payload_without_package_rows() {
    let fixture = ActiveCatalogFixture::new();
    fixture.activate(
        "fedora-44",
        1,
        vec![
            package("popular-high", "1.0", "x86_64", "high-catalog"),
            package("popular-low", "1.0", "x86_64", "low-catalog"),
        ],
    );
    let conn = fixture.connection();
    conn.execute(
        "INSERT INTO download_counts (source_profile, package_name, total_count)
         VALUES ('fedora-44', 'popular-high', 20)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO download_counts (source_profile, package_name, total_count)
         VALUES ('fedora-44', 'popular-low', 5)",
        [],
    )
    .unwrap();
    let universe = public_universe(&fixture);

    let popular = query_popular(
        fixture.authority(),
        fixture.db_path(),
        &universe,
        Some("fedora"),
        10,
    )
    .unwrap();

    assert_eq!(popular.len(), 2);
    assert_eq!(popular[0].name, "popular-high");
    assert_eq!(popular[0].download_count, 20);
    assert_eq!(
        popular[0].description.as_deref(),
        Some("catalog description high-catalog")
    );
    assert_eq!(popular[1].name, "popular-low");
    assert_eq!(popular[1].download_count, 5);
    assert_eq!(
        popular[1].description.as_deref(),
        Some("catalog description low-catalog")
    );
}

#[test]
fn package_detail_counts_only_current_conversions() {
    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate(
        "fedora-44",
        1,
        vec![
            package("pkg", "1.0", "x86_64", "one"),
            package("pkg", "2.0", "x86_64", "two"),
        ],
    );
    let conn = fixture.connection();

    insert_converted(&conn, &revision, "pkg", "1.0", "x86_64", CONVERSION_VERSION);
    insert_stale_conversion(&conn, &revision, "pkg", "2.0", "x86_64");
    let universe = public_universe(&fixture);
    let selection = universe.profile("fedora-44").unwrap();

    let detail = query_package_detail(
        fixture.authority(),
        fixture.db_path(),
        "fedora",
        "pkg",
        selection,
    )
    .unwrap()
    .unwrap();
    let versions = query_versions(
        fixture.authority(),
        fixture.db_path(),
        "fedora",
        "pkg",
        selection,
    )
    .unwrap();
    let overview = query_overview(fixture.authority(), fixture.db_path(), &universe).unwrap();

    assert!(detail.converted);
    assert_eq!(overview.total_converted, 1);
    assert!(
        versions
            .iter()
            .any(|version| version.version == "1.0" && version.converted)
    );
    assert!(
        versions
            .iter()
            .any(|version| version.version == "2.0" && !version.converted)
    );
}
