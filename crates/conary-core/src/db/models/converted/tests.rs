// conary-core/src/db/models/converted/tests.rs

use super::*;
use crate::ccs::convert::ScriptletBundleSummary;
use crate::db::models::{
    RemiCatalogResource, RemiCatalogResourceKind, RemiProfileRevisionPin, RemiRevisionPinKind,
    Trove, TroveType,
};
use crate::db::testing::create_test_db;
use rusqlite::Connection;

const FEDORA_REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn seed_profile_resource(conn: &Connection, profile: &str, manifest_json: &str) -> String {
    let revision = crate::hash::sha256(manifest_json.as_bytes());
    RemiCatalogResource {
        resource_sha256: revision.clone(),
        kind: RemiCatalogResourceKind::ProfileRevision,
        source_profile: profile.to_string(),
        artifact_sha256: crate::hash::sha256(format!("artifact-{revision}").as_bytes()),
        artifact_size: 1,
        logical_digest_sha256: crate::hash::sha256(format!("logical-{revision}").as_bytes()),
        manifest_json: manifest_json.to_string(),
        durable: true,
        created_at: 1,
    }
    .insert(conn)
    .unwrap();
    revision
}

fn server_package(checksum: &str, chunk: &str) -> ConvertedPackage {
    let transport = crate::ccs::transport::test_transport(&[chunk.to_string()]);
    ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        FEDORA_REVISION.to_string(),
        "fixture".to_string(),
        "1.0-1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        checksum.to_string(),
        &transport,
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

    let found =
        ConvertedPackage::find_repository_by_checksum(&conn, FEDORA_REVISION, "sha256:source")
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
        ConvertedPackage::find_current_conversions(&conn, FEDORA_REVISION, Some("fixture"))
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
        ConvertedPackage::find_current_conversions(&conn, FEDORA_REVISION, Some("fixture"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn stale_conversion_revision_is_reconciled_without_operational_metadata() {
    let (_temp, conn) = create_test_db();
    seed_server_package_source(&conn, "sha256:source");
    let mut converted = server_package("sha256:source", "sha256:chunk");
    converted.insert(&conn).unwrap();
    assert!(converted.repository_conversion_is_current().unwrap());
    converted.conversion_version = CONVERSION_VERSION - 1;
    conn.execute(
        "UPDATE converted_packages SET conversion_version = ?1 WHERE id = ?2",
        rusqlite::params![converted.conversion_version, converted.id.unwrap()],
    )
    .unwrap();
    // Mutable RepositoryProvide rows are deliberately not conversion
    // authority; only the exact revision and conversion algorithm matter.
    assert!(!converted.repository_conversion_is_current().unwrap());
    assert_eq!(
        ConvertedPackage::reconcile_repository_conversions(&conn, FEDORA_REVISION).unwrap(),
        1
    );
    assert!(
        ConvertedPackage::find_repository_by_checksum(&conn, FEDORA_REVISION, "sha256:source")
            .unwrap()
            .is_none()
    );
}

#[test]
fn sha1_repository_checksum_is_bound_to_the_immutable_profile_revision() {
    let (_temp, conn) = create_test_db();
    let source_checksum = "sha1:1826421aded2a344b7864ffff2fae2430778b1f0";
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
        [source_checksum],
    )
    .unwrap();
    let transport = crate::ccs::transport::test_transport(&["sha256:chunk".to_string()]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        FEDORA_REVISION.to_string(),
        "fixture".to_string(),
        "1.0-1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        source_checksum.to_string(),
        &transport,
        42,
        "sha256:content".to_string(),
        "/tmp/fixture.ccs".to_string(),
        EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    converted.insert(&conn).unwrap();

    assert!(converted.repository_conversion_is_current().unwrap());
    assert_eq!(
        ConvertedPackage::find_current_conversions(&conn, FEDORA_REVISION, Some("fixture"))
            .unwrap()
            .len(),
        1
    );

    conn.execute(
        "UPDATE repository_packages SET checksum = ?1 WHERE id = 1",
        ["sha256:1826421aded2a344b7864ffff2fae2430778b1f0"],
    )
    .unwrap();
    assert!(converted.repository_conversion_is_current().unwrap());
}

#[test]
fn repository_artifact_exposes_only_complete_serving_state() {
    let converted = server_package("sha256:source", "sha256:chunk");
    let artifact = converted.repository_artifact().unwrap();

    assert_eq!(artifact.package_name, "fixture");
    assert_eq!(artifact.package_version, "1.0-1");
    assert_eq!(artifact.source_profile, "fedora-44");
    assert_eq!(artifact.profile_revision_sha256, FEDORA_REVISION);
    assert_eq!(artifact.package_architecture, "x86_64");
    assert_eq!(
        artifact
            .transport
            .objects
            .iter()
            .map(|object| object.sha256.as_str())
            .collect::<Vec<_>>(),
        ["sha256:chunk"]
    );
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
fn installed_conversion_profile_revision_is_null() {
    let (_temp, conn) = create_test_db();
    let mut converted = installed_package(&conn, "rpm", "sha256:installed-revision-null");
    converted.insert(&conn).unwrap();

    let stored: Option<String> = conn
        .query_row(
            "SELECT profile_revision_sha256 FROM converted_packages WHERE id = ?1",
            [converted.id.unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, None);
}

#[test]
fn repository_conversion_requires_exact_lowercase_profile_revision() {
    let (_temp, conn) = create_test_db();
    let mut converted = server_package("sha256:revision-required", "sha256:chunk");
    converted.profile_revision_sha256 = None;
    let error = converted.insert(&conn).unwrap_err().to_string();
    assert!(error.contains("missing profile_revision_sha256"), "{error}");

    for invalid in [
        "",
        "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0",
    ] {
        let mut converted = server_package("sha256:revision-invalid", "sha256:chunk");
        converted.profile_revision_sha256 = Some(invalid.to_string());
        let error = converted.insert(&conn).unwrap_err().to_string();
        assert!(
            error.contains("exactly 64 lowercase hexadecimal characters")
                || (invalid.is_empty() && error.contains("missing profile_revision_sha256")),
            "invalid revision {invalid:?}: {error}"
        );
    }
}

#[test]
fn repository_provides_digest_is_optional_diagnostic_state() {
    let (_temp, conn) = create_test_db();
    let mut converted = server_package("sha256:diagnostic-optional", "sha256:chunk");
    converted.repository_provides_digest = None;
    converted.insert(&conn).unwrap();

    let found = ConvertedPackage::find_repository_by_checksum(
        &conn,
        FEDORA_REVISION,
        "sha256:diagnostic-optional",
    )
    .unwrap()
    .unwrap();
    assert_eq!(found.repository_provides_digest, None);
    assert!(found.repository_artifact().is_ok());
    assert!(found.repository_conversion_is_current().unwrap());
}

#[test]
fn conversion_row_and_exact_revision_pin_share_atomic_lifecycle() {
    let (_temp, conn) = create_test_db();
    let manifest_json = "{}";
    let profile_revision_sha256 = crate::hash::sha256(manifest_json.as_bytes());
    RemiCatalogResource {
        resource_sha256: profile_revision_sha256.clone(),
        kind: RemiCatalogResourceKind::ProfileRevision,
        source_profile: "fedora-44".to_string(),
        artifact_sha256: "c".repeat(64),
        artifact_size: 1,
        logical_digest_sha256: "d".repeat(64),
        manifest_json: manifest_json.to_string(),
        durable: true,
        created_at: 1,
    }
    .insert(&conn)
    .unwrap();

    let mut converted = server_package("sha256:pinned", "sha256:chunk");
    converted.profile_revision_sha256 = Some(profile_revision_sha256.clone());
    let id = converted.insert_with_conversion_pin(&conn, 2).unwrap();
    let pin = ConvertedPackage::require_conversion_pin(&conn, id).unwrap();
    assert_eq!(pin.owner_kind, RemiRevisionPinKind::Conversion);
    assert_eq!(pin.owner_identity, id.to_string());
    assert_eq!(pin.profile_revision_sha256, profile_revision_sha256);
    assert_eq!(pin.source_profile, "fedora-44");

    assert!(ConvertedPackage::delete_with_conversion_pin(&conn, id).unwrap());
    assert!(ConvertedPackage::find_by_id(&conn, id).unwrap().is_none());
    assert!(
        crate::db::models::RemiProfileRevisionPin::find(
            &conn,
            &ConvertedPackage::conversion_pin_id(id),
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn conversion_pin_insert_rolls_back_row_when_exact_revision_is_not_durable() {
    let (_temp, conn) = create_test_db();
    let mut converted = server_package("sha256:pin-rollback", "sha256:chunk");
    let id_result = converted.insert_with_conversion_pin(&conn, 2);
    assert!(id_result.is_err());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM converted_packages", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

#[test]
fn repository_conversion_without_its_exact_pin_is_corruption() {
    let (_temp, conn) = create_test_db();
    let revision = seed_profile_resource(&conn, "fedora-44", r#"{"revision":"a"}"#);
    let mut converted = server_package("sha256:missing-pin", "sha256:chunk");
    converted.profile_revision_sha256 = Some(revision);
    let id = converted.insert(&conn).unwrap();

    let error = ConvertedPackage::require_conversion_pin(&conn, id)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("has no exact profile-revision pin"),
        "{error}"
    );
}

#[test]
fn repository_conversion_rejects_a_pin_for_another_revision() {
    let (_temp, conn) = create_test_db();
    let row_revision = seed_profile_resource(&conn, "fedora-44", r#"{"revision":"a"}"#);
    let other_revision = seed_profile_resource(&conn, "fedora-44", r#"{"revision":"b"}"#);
    let mut converted = server_package("sha256:mismatched-pin", "sha256:chunk");
    converted.profile_revision_sha256 = Some(row_revision);
    let id = converted.insert(&conn).unwrap();
    RemiProfileRevisionPin {
        pin_id: ConvertedPackage::conversion_pin_id(id),
        source_profile: "fedora-44".to_string(),
        profile_revision_sha256: other_revision,
        owner_kind: RemiRevisionPinKind::Conversion,
        owner_identity: ConvertedPackage::conversion_pin_owner_identity(id),
        runtime_session_id: None,
        pinned_at: 2,
    }
    .insert(&conn)
    .unwrap();

    let error = ConvertedPackage::require_conversion_pin(&conn, id)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("mismatched profile-revision pin identity"),
        "{error}"
    );
}

#[test]
fn reconciling_a_stale_conversion_removes_its_exact_pin() {
    let (_temp, conn) = create_test_db();
    let manifest_json = "{}";
    let revision = crate::hash::sha256(manifest_json.as_bytes());
    RemiCatalogResource {
        resource_sha256: revision.clone(),
        kind: RemiCatalogResourceKind::ProfileRevision,
        source_profile: "fedora-44".to_string(),
        artifact_sha256: "c".repeat(64),
        artifact_size: 1,
        logical_digest_sha256: "d".repeat(64),
        manifest_json: manifest_json.to_string(),
        durable: true,
        created_at: 1,
    }
    .insert(&conn)
    .unwrap();
    let mut converted = server_package("sha256:stale-pinned", "sha256:chunk");
    converted.profile_revision_sha256 = Some(revision.clone());
    let id = converted.insert_with_conversion_pin(&conn, 2).unwrap();
    conn.execute(
        "UPDATE converted_packages SET conversion_version = ?1 WHERE id = ?2",
        rusqlite::params![CONVERSION_VERSION - 1, id],
    )
    .unwrap();

    assert_eq!(
        ConvertedPackage::reconcile_repository_conversions(&conn, &revision).unwrap(),
        1
    );
    assert!(ConvertedPackage::find_by_id(&conn, id).unwrap().is_none());
    assert!(
        RemiProfileRevisionPin::find(&conn, &ConvertedPackage::conversion_pin_id(id))
            .unwrap()
            .is_none()
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
fn repository_artifact_rejects_corrupt_transport_json() {
    let (_temp, conn) = create_test_db();
    let mut converted = server_package("sha256:source", "sha256:chunk");
    converted.insert(&conn).unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    conn.execute(
        "UPDATE converted_packages SET transport_json = '{bad' WHERE id = ?1",
        [converted.id.unwrap()],
    )
    .unwrap();

    let found =
        ConvertedPackage::find_repository_by_checksum(&conn, FEDORA_REVISION, "sha256:source")
            .unwrap()
            .unwrap();
    let error = found.repository_artifact().unwrap_err().to_string();
    assert!(error.contains("malformed transport_json"), "{error}");
}

#[test]
fn chunk_conversion_state_uses_current_typed_rows() {
    let (_temp, conn) = create_test_db();
    seed_server_package_source(&conn, "sha256:source");
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
    seed_server_package_source(&conn, "sha256:source");
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
fn checksum_identity_is_scoped_by_artifact_kind_and_profile_revision() {
    let (_temp, conn) = create_test_db();
    let checksum = "sha256:shared";

    installed_package(&conn, "rpm", checksum)
        .insert(&conn)
        .unwrap();
    server_package(checksum, "sha256:fedora")
        .insert(&conn)
        .unwrap();
    assert!(
        server_package(checksum, "sha256:duplicate")
            .insert(&conn)
            .is_err()
    );
    let mut arch = server_package(checksum, "sha256:arch");
    arch.source_profile = Some("arch".to_string());
    arch.profile_revision_sha256 =
        Some("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string());
    arch.insert(&conn).unwrap();

    assert!(
        ConvertedPackage::find_installed_by_checksum(&conn, checksum)
            .unwrap()
            .is_some()
    );
    assert!(
        ConvertedPackage::find_repository_by_checksum(&conn, FEDORA_REVISION, checksum)
            .unwrap()
            .is_some()
    );
    assert!(
        ConvertedPackage::find_repository_by_checksum(
            &conn,
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            checksum,
        )
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
