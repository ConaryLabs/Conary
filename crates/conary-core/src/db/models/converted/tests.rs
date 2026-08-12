// conary-core/src/db/models/converted/tests.rs

use super::*;
use crate::ccs::convert::ScriptletBundleSummary;
use crate::db::models::{RepositoryProvide, Trove, TroveType};
use crate::db::testing::create_test_db;

fn server_package(checksum: &str, chunk: &str) -> ConvertedPackage {
    ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        "fixture".to_string(),
        "1.0-1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        checksum.to_string(),
        &[chunk.to_string()],
        42,
        "sha256:content".to_string(),
        "/tmp/fixture.ccs".to_string(),
        EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    )
}

fn seed_server_package_source(conn: &Connection, checksum: &str) {
    conn.execute(
        "INSERT INTO repositories (name, url, source_profile)
         VALUES ('fedora-source', 'https://example.test', 'fedora-44')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, architecture, checksum, size, download_url, version_scheme)
         VALUES (1, 'fixture', '1.0-1', 'x86_64', ?1, 42,
                 'https://example.test/fixture.rpm', 'rpm')",
        [checksum],
    )
    .unwrap();
}

fn installed_package(
    conn: &Connection,
    original_format: &str,
    original_checksum: &str,
) -> ConvertedPackage {
    let mut trove = Trove::new(
        format!("installed-{}", conn.last_insert_rowid()),
        "1.0.0".to_string(),
        TroveType::Package,
        crate::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(conn).unwrap();
    ConvertedPackage::new_installed(
        trove_id,
        original_format.to_string(),
        original_checksum.to_string(),
    )
}

#[test]
fn converted_package_defaults_to_current_native_free_contract() {
    let converted =
        ConvertedPackage::new_installed(1, "rpm".to_string(), "sha256:source".to_string());

    assert_eq!(converted.scriptlet_fidelity, "native-free");
    assert_eq!(
        converted.scriptlet_summary().unwrap(),
        ScriptletBundleSummary::default()
    );
}

#[test]
fn converted_package_round_trips_lifecycle_summary() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::ensure_current(&conn).unwrap();
    let mut converted = server_package("sha256:source", "sha256:chunk");
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "native-lifecycle".to_string(),
        evidence_digest: Some(crate::hash::sha256_prefixed(b"evidence")),
    };
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.insert(&conn).unwrap();

    let found = ConvertedPackage::find_repository_by_checksum(&conn, "fedora-44", "sha256:source")
        .unwrap()
        .unwrap();
    assert_eq!(found.scriptlet_summary().unwrap(), summary);
}

#[test]
fn malformed_summary_is_explicit_corruption_error() {
    let mut converted =
        ConvertedPackage::new_installed(1, "rpm".to_string(), "sha256:source".to_string());
    converted.scriptlet_summary_json = "{not valid json".to_string();

    let error = converted.scriptlet_summary().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("malformed lifecycle summary JSON")
    );
}

#[test]
fn summary_projection_mismatch_is_explicit_corruption_error() {
    let mut converted =
        ConvertedPackage::new_installed(1, "rpm".to_string(), "sha256:source".to_string());
    converted.scriptlet_fidelity = "native-lifecycle".to_string();

    let error = converted.scriptlet_summary().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("disagrees with indexed projection columns")
    );
}

#[test]
fn insert_rejects_malformed_current_summary() {
    let (_temp, conn) = create_test_db();
    let mut converted = installed_package(&conn, "rpm", "sha256:source");
    converted.scriptlet_summary_json = "{}".to_string();

    assert!(converted.insert(&conn).is_err());
}

#[test]
fn stale_rows_are_excluded_from_current_conversion_query() {
    let (_temp, conn) = create_test_db();
    let mut converted = server_package("sha256:source", "sha256:chunk");
    converted.conversion_version = CONVERSION_VERSION - 1;
    converted.insert(&conn).unwrap();

    assert!(
        ConvertedPackage::find_current_conversions(&conn, "fedora-44", Some("fixture"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn current_typed_summary_is_returned_by_current_conversion_query() {
    let (_temp, conn) = create_test_db();
    seed_server_package_source(&conn, "sha256:source");
    let mut converted = server_package("sha256:source", "sha256:chunk");
    converted.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::find_current_conversions(&conn, "fedora-44", Some("fixture"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn repository_provide_change_invalidates_and_reconciles_conversion() {
    let (_temp, conn) = create_test_db();
    seed_server_package_source(&conn, "sha256:source");
    let mut converted = server_package("sha256:source", "sha256:chunk");
    converted.insert(&conn).unwrap();
    assert!(converted.repository_metadata_is_current(&conn).unwrap());

    let mut provide = RepositoryProvide::new(
        1,
        "/usr/bin/kernel-install".to_string(),
        None,
        "file".to_string(),
        Some("/usr/bin/kernel-install".to_string()),
        crate::repository::versioning::VersionScheme::Rpm,
    );
    provide.insert(&conn).unwrap();

    assert!(!converted.repository_metadata_is_current(&conn).unwrap());
    assert_eq!(
        ConvertedPackage::reconcile_repository_metadata(&conn, "fedora-44").unwrap(),
        1
    );
    assert!(
        ConvertedPackage::find_repository_by_checksum(&conn, "fedora-44", "sha256:source")
            .unwrap()
            .is_none()
    );
}

#[test]
fn repository_artifact_exposes_only_complete_serving_state() {
    let converted = server_package("sha256:source", "sha256:chunk");
    let artifact = converted.repository_artifact().unwrap();

    assert_eq!(artifact.package_name, "fixture");
    assert_eq!(artifact.package_version, "1.0-1");
    assert_eq!(artifact.source_profile, "fedora-44");
    assert_eq!(artifact.package_architecture, "x86_64");
    assert_eq!(artifact.chunk_hashes, vec!["sha256:chunk".to_string()]);
    assert_eq!(artifact.total_size, 42);
    assert_eq!(artifact.content_hash, "sha256:content");
    assert_eq!(artifact.ccs_path, "/tmp/fixture.ccs");
}

#[test]
fn repository_artifact_rejects_missing_or_empty_architecture() {
    let mut converted = server_package("sha256:source", "sha256:chunk");

    for architecture in [None, Some(String::new())] {
        converted.package_architecture = architecture;
        let error = converted.repository_artifact().unwrap_err().to_string();
        assert!(error.contains("missing package_architecture"), "{error}");
    }
}

#[test]
fn installed_conversion_rejects_repository_serving_fields() {
    let (_temp, conn) = create_test_db();
    let mut converted = installed_package(&conn, "rpm", "sha256:installed");
    converted.package_name = Some("must-not-serve".to_string());

    let error = converted.insert(&conn).unwrap_err().to_string();
    assert!(
        error.contains("carries repository-serving fields"),
        "{error}"
    );
}

#[test]
fn installed_conversion_round_trips_a_durable_ccs_path() {
    let (_temp, conn) = create_test_db();
    let mut converted = installed_package(&conn, "rpm", "sha256:installed-path");
    converted
        .set_installed_ccs_path("/var/lib/conary/packages/adopted/exact.ccs".to_string())
        .unwrap();
    converted.insert(&conn).unwrap();

    let found = ConvertedPackage::find_by_trove(&conn, converted.trove_id.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        found.ccs_path.as_deref(),
        Some("/var/lib/conary/packages/adopted/exact.ccs")
    );

    let mut empty = installed_package(&conn, "deb", "sha256:empty-path");
    assert!(empty.set_installed_ccs_path(String::new()).is_err());
    empty.ccs_path = Some(String::new());
    assert!(empty.insert(&conn).is_err());
}

#[test]
fn repository_artifact_rejects_corrupt_chunk_json() {
    let (_temp, conn) = create_test_db();
    let mut converted = server_package("sha256:source", "sha256:chunk");
    converted.insert(&conn).unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    conn.execute(
        "UPDATE converted_packages SET chunk_hashes_json = '{bad' WHERE id = ?1",
        [converted.id.unwrap()],
    )
    .unwrap();

    let found = ConvertedPackage::find_repository_by_checksum(&conn, "fedora-44", "sha256:source")
        .unwrap()
        .unwrap();
    let error = found.repository_artifact().unwrap_err().to_string();
    assert!(error.contains("malformed chunk_hashes_json"), "{error}");
}

#[test]
fn chunk_conversion_state_uses_current_typed_rows() {
    let (_temp, conn) = create_test_db();
    seed_server_package_source(&conn, "source");
    let mut converted = server_package("sha256:source", "sha256:chunk");
    converted.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::chunk_conversion_state(&conn, "chunk").unwrap(),
        ChunkConversionState::CurrentConversion
    );
}

#[test]
fn chunk_conversion_state_errors_on_malformed_current_summary() {
    let (_temp, conn) = create_test_db();
    seed_server_package_source(&conn, "source");
    let mut converted = server_package("sha256:source", "sha256:chunk");
    converted.insert(&conn).unwrap();
    conn.execute(
        "UPDATE converted_packages SET scriptlet_summary_json = '{malformed' WHERE id = ?1",
        [converted.id.unwrap()],
    )
    .unwrap();

    assert!(ConvertedPackage::chunk_conversion_state(&conn, "chunk").is_err());
}

#[test]
fn chunk_conversion_state_reports_stale_only_references() {
    let (_temp, conn) = create_test_db();
    let mut converted = server_package("sha256:source", "sha256:chunk");
    converted.conversion_version = CONVERSION_VERSION - 1;
    converted.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::chunk_conversion_state(&conn, "chunk").unwrap(),
        ChunkConversionState::StaleConversionOnly
    );
}

#[test]
fn chunk_conversion_state_reports_unreferenced_hashes() {
    let (_temp, conn) = create_test_db();
    let mut converted = server_package("sha256:source", "sha256:chunk");
    converted.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::chunk_conversion_state(&conn, "other").unwrap(),
        ChunkConversionState::NoConvertedReference
    );
}

#[test]
fn converted_package_crud() {
    let (_temp, conn) = create_test_db();
    let mut converted = installed_package(&conn, "rpm", "sha256:abc123def456");

    let id = converted.insert(&conn).unwrap();
    assert!(id > 0);
    let found = ConvertedPackage::find_installed_by_checksum(&conn, "sha256:abc123def456")
        .unwrap()
        .unwrap();
    assert_eq!(found.original_format, "rpm");
    assert_eq!(found.scriptlet_fidelity, "native-free");
    assert_eq!(ConvertedPackage::list_all(&conn).unwrap().len(), 1);

    ConvertedPackage::delete_installed_by_checksum(&conn, "sha256:abc123def456").unwrap();
    assert!(
        ConvertedPackage::find_installed_by_checksum(&conn, "sha256:abc123def456")
            .unwrap()
            .is_none()
    );
}

#[test]
fn needs_reconversion_tracks_current_contract_revision() {
    let mut converted =
        ConvertedPackage::new_installed(1, "deb".to_string(), "sha256:test".to_string());
    assert!(!converted.needs_reconversion());

    converted.conversion_version = CONVERSION_VERSION - 1;
    assert!(converted.needs_reconversion());
}

#[test]
fn count_by_format() {
    let (_temp, conn) = create_test_db();
    for (format, checksum) in [("rpm", "r1"), ("rpm", "r2"), ("deb", "d1")] {
        installed_package(&conn, format, &format!("sha256:{checksum}"))
            .insert(&conn)
            .unwrap();
    }

    assert_eq!(
        ConvertedPackage::count_by_format(&conn).unwrap(),
        vec![("rpm".to_string(), 2), ("deb".to_string(), 1)]
    );
}

#[test]
fn checksum_is_unique() {
    let (_temp, conn) = create_test_db();
    installed_package(&conn, "rpm", "sha256:same")
        .insert(&conn)
        .unwrap();

    assert!(
        installed_package(&conn, "deb", "sha256:same")
            .insert(&conn)
            .is_err()
    );
}

#[test]
fn checksum_identity_is_scoped_by_artifact_kind_and_source_profile() {
    let (_temp, conn) = create_test_db();
    let checksum = "sha256:shared";

    installed_package(&conn, "rpm", checksum)
        .insert(&conn)
        .unwrap();
    server_package(checksum, "sha256:fedora")
        .insert(&conn)
        .unwrap();
    let mut arch = server_package(checksum, "sha256:arch");
    arch.source_profile = Some("arch".to_string());
    arch.insert(&conn).unwrap();

    assert!(
        ConvertedPackage::find_installed_by_checksum(&conn, checksum)
            .unwrap()
            .is_some()
    );
    assert!(
        ConvertedPackage::find_repository_by_checksum(&conn, "fedora-44", checksum)
            .unwrap()
            .is_some()
    );
    assert!(
        ConvertedPackage::find_repository_by_checksum(&conn, "arch", checksum)
            .unwrap()
            .is_some()
    );
}

#[test]
fn enhancement_state_round_trips() {
    let (_temp, conn) = create_test_db();
    let mut converted = installed_package(&conn, "rpm", "sha256:enhance");
    converted.insert(&conn).unwrap();
    converted
        .set_enhancement_complete(&conn, 1, Some(r#"{"builder":"conary"}"#))
        .unwrap();

    assert!(!converted.needs_enhancement(1));
    assert!(converted.needs_enhancement(2));
    let found = ConvertedPackage::find_installed_by_checksum(&conn, "sha256:enhance")
        .unwrap()
        .unwrap();
    assert_eq!(found.enhancement_status, "complete");
    assert_eq!(found.enhancement_version, 1);
}

#[test]
fn enhancement_failure_round_trips() {
    let (_temp, conn) = create_test_db();
    let mut converted = installed_package(&conn, "deb", "sha256:fail");
    converted.insert(&conn).unwrap();
    converted
        .set_enhancement_failed(&conn, "Test error message")
        .unwrap();

    let found = ConvertedPackage::find_installed_by_checksum(&conn, "sha256:fail")
        .unwrap()
        .unwrap();
    assert_eq!(found.enhancement_status, "failed");
    assert_eq!(
        found.enhancement_error.as_deref(),
        Some("Test error message")
    );
}
