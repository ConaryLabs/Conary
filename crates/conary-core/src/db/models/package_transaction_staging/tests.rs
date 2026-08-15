// conary-core/src/db/models/package_transaction_staging/tests.rs

use super::*;
use crate::db::models::{Changeset, Trove, TroveType};
use crate::db::testing::create_test_db;
use crate::payload::{PayloadNode, PayloadSharingPolicy};
use crate::repository::versioning::VersionScheme;
use rusqlite::Connection;

fn regular_entry(path: &str, trove_id: i64, bytes: &[u8]) -> FileEntry {
    FileEntry::new(
        path.to_string(),
        ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
        Some(PayloadContentAuthority {
            sha256: crate::hash::sha256(bytes),
            size: u64::try_from(bytes.len()).unwrap(),
        }),
        trove_id,
    )
}

fn insert_trove(conn: &Connection, name: &str) -> i64 {
    let mut trove = Trove::new(
        name.to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    trove.insert(conn).unwrap()
}

#[test]
fn staged_payload_reconciles_with_fixed_query_shapes() {
    let (_temp, mut conn) = create_test_db();
    let trove_id = insert_trove(&conn, "fixture");
    let mut changeset = Changeset::new("stage fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    let mut staging = PackageTransactionStaging::begin(&tx).unwrap();
    for index in 0..100 {
        staging
            .stage_payload(&StagedPayloadRow {
                entry: regular_entry(&format!("/usr/share/fixture/{index:03}"), trove_id, b"x"),
                package_name: "fixture".to_string(),
                component_name: None,
                directory_materialization: ExistingDirectoryMaterialization::ApplyIncoming,
                disposition: StagedAnchorDisposition::Replace,
                selected_root_node: None,
                materialization_target_path: None,
                history: Some((changeset_id, "add")),
            })
            .unwrap();
    }
    let outcomes = staging.validate_and_reconcile().unwrap();
    assert_eq!(outcomes.len(), 100);
    let work = staging.finish().unwrap();
    assert_eq!(work.rows_loaded, 200);
    assert_eq!(work.load_statement_executions, 200);
    assert!(work.query_shapes <= 24, "unexpected query shapes: {work:?}");
    assert!(work.total_statement_executions() <= 225);
    tx.commit().unwrap();

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 100);
}

#[test]
fn staged_payload_failure_rolls_back_canonical_rows() {
    let (_temp, mut conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let mut existing = regular_entry("/usr/bin/shared", first, b"first");
    existing.insert(&conn).unwrap();

    let tx = conn.transaction().unwrap();
    let mut staging = PackageTransactionStaging::begin(&tx).unwrap();
    staging
        .stage_payload(&StagedPayloadRow {
            entry: regular_entry("/usr/bin/shared", second, b"second"),
            package_name: "second".to_string(),
            component_name: None,
            directory_materialization: ExistingDirectoryMaterialization::ApplyIncoming,
            disposition: StagedAnchorDisposition::Replace,
            selected_root_node: None,
            materialization_target_path: None,
            history: None,
        })
        .unwrap();
    let error = staging.validate_and_reconcile().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot replace claim from package first"),
        "unexpected staging error: {error}"
    );
    drop(staging);
    tx.rollback().unwrap();

    let current = FileEntry::find_by_path(&conn, "/usr/bin/shared")
        .unwrap()
        .unwrap();
    assert_eq!(current.trove_id, first);
    assert_eq!(
        current.content.unwrap().sha256,
        crate::hash::sha256(b"first")
    );
}

#[test]
fn compatible_rpm_claim_shares_one_anchor() {
    let (_temp, mut conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let mut existing = regular_entry("/usr/bin/shared", first, b"same")
        .with_claim_policy(PayloadSharingPolicy::Rpm);
    existing.insert(&conn).unwrap();

    let tx = conn.transaction().unwrap();
    let mut staging = PackageTransactionStaging::begin(&tx).unwrap();
    staging
        .stage_payload(&StagedPayloadRow {
            entry: regular_entry("/usr/bin/shared", second, b"same")
                .with_claim_policy(PayloadSharingPolicy::Rpm),
            package_name: "second".to_string(),
            component_name: None,
            directory_materialization: ExistingDirectoryMaterialization::ApplyIncoming,
            disposition: StagedAnchorDisposition::Share,
            selected_root_node: None,
            materialization_target_path: None,
            history: None,
        })
        .unwrap();
    let outcomes = staging.validate_and_reconcile().unwrap();
    assert!(!outcomes["/usr/bin/shared"].materialized);
    staging.finish().unwrap();
    tx.commit().unwrap();

    assert_eq!(
        PayloadClaim::find_by_path(&conn, "/usr/bin/shared")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        FileEntry::find_by_path(&conn, "/usr/bin/shared")
            .unwrap()
            .unwrap()
            .trove_id,
        first
    );
}
