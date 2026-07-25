// apps/conary/tests/enhancement_integration.rs
//! Integration proof that retroactive enrichment records exact provenance only.

use conary_core::ccs::enhancement::{
    EnhancementRunner, EnhancementType, EnhancementWindowStatus, check_enhancement_window,
};
use conary_core::db;
use rusqlite::params;
use tempfile::NamedTempFile;

fn create_test_package(name: &str) -> (NamedTempFile, rusqlite::Connection, i64, i64) {
    let database = NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(database.path()).unwrap();
    db::schema::ensure_current(&conn).unwrap();

    conn.execute(
        "INSERT INTO troves (
             name, version, type, architecture, install_source, install_reason, version_scheme
         ) VALUES (
             ?1, '1.0.0', 'package', 'x86_64', 'file', 'explicit', 'conary'
         )",
        [name],
    )
    .unwrap();
    let trove_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO converted_packages (
             artifact_kind, trove_id, original_format, original_checksum,
             enhancement_status, enhancement_version
         ) VALUES ('installed', ?1, 'rpm', ?2, 'pending', 0)",
        params![trove_id, format!("checksum-{name}")],
    )
    .unwrap();
    let converted_id = conn.last_insert_rowid();

    (database, conn, trove_id, converted_id)
}

#[test]
fn only_exact_provenance_enrichment_is_available() {
    assert_eq!(EnhancementType::all(), &[EnhancementType::Provenance]);
    assert_eq!(
        EnhancementType::parse_name("provenance"),
        Some(EnhancementType::Provenance)
    );
    assert_eq!(EnhancementType::parse_name("capabilities"), None);
    assert_eq!(EnhancementType::parse_name("subpackages"), None);
}

#[test]
fn provenance_enrichment_does_not_infer_policy_or_relationships() {
    let (_database, conn, trove_id, converted_id) = create_test_package("nginx-devel");

    let result = EnhancementRunner::new(&conn).enhance(trove_id).unwrap();
    assert_eq!(result.applied, vec![EnhancementType::Provenance]);
    assert!(result.failed.is_empty());

    let (status, evidence): (String, Option<String>) = conn
        .query_row(
            "SELECT enhancement_status, extracted_provenance_json
             FROM converted_packages WHERE id = ?1",
            [converted_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "complete");
    let evidence = evidence.expect("exact conversion provenance is recorded");
    assert!(evidence.contains("\"original_format\":\"rpm\""));
    assert!(evidence.contains("\"package_name\":\"nginx-devel\""));

    let capability_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM capabilities WHERE trove_id = ?1",
            [trove_id],
            |row| row.get(0),
        )
        .unwrap();
    let relationship_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM subpackage_relationships
             WHERE subpackage_name = 'nginx-devel'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let dependency_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM package_requirement_groups WHERE trove_id = ?1",
            [trove_id],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(capability_count, 0, "heuristics must not create policy");
    assert_eq!(
        relationship_count, 0,
        "name suffixes must not create package relationships"
    );
    assert_eq!(
        dependency_count, 0,
        "name suffixes must not create dependencies"
    );
    assert!(matches!(
        check_enhancement_window(&conn, trove_id),
        EnhancementWindowStatus::Complete
    ));
}
