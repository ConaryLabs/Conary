// conary-core/src/db/models/package_transaction_staging/tests.rs

use super::*;
use crate::db::models::{Changeset, Trove, TroveType};
use crate::db::testing::create_test_db;
use crate::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadSharingPolicy,
    PayloadTimestamp,
};
use crate::repository::versioning::VersionScheme;
use rusqlite::Connection;
use std::collections::BTreeMap;

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

fn directory_entry(path: &str, trove_id: i64) -> FileEntry {
    FileEntry::new(
        path.to_string(),
        ResolvedPayloadNode {
            source: PayloadNode {
                kind: PayloadNodeKind::Directory,
                mode: libc::S_IFDIR | 0o755,
                user: PayloadIdentity::Numeric { id: 0 },
                group: PayloadIdentity::Numeric { id: 0 },
                mtime: PayloadTimestamp::UNIX_EPOCH,
                xattrs: BTreeMap::new(),
            },
            uid: 0,
            gid: 0,
        },
        None,
        trove_id,
    )
}

fn hardlink_entry(path: &str, target: &str, trove_id: i64) -> FileEntry {
    let mut source = PayloadNode::regular(0o644);
    source.kind = PayloadNodeKind::Hardlink {
        target: target.to_string(),
        identity: "fixture:hardlink-group".to_string(),
    };
    FileEntry::new(
        path.to_string(),
        ResolvedPayloadNode::from_numeric_source(source).unwrap(),
        None,
        trove_id,
    )
}

fn staged_payload(
    entry: FileEntry,
    package_name: &str,
    component_name: Option<&str>,
    changeset_id: i64,
) -> StagedPayloadRow {
    StagedPayloadRow {
        entry,
        package_name: package_name.to_string(),
        component_name: component_name.map(str::to_string),
        directory_materialization: ExistingDirectoryMaterialization::ApplyIncoming,
        disposition: StagedAnchorDisposition::Replace,
        selected_root_node: None,
        materialization_target_path: None,
        history: Some((changeset_id, "add")),
    }
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

fn tiny_file_work(path_count: usize) -> PackageTransactionSqlWork {
    let (_temp, mut conn) = create_test_db();
    let trove_id = insert_trove(&conn, "tiny-fixture");
    let mut changeset = Changeset::new("tiny fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    let mut staging = PackageTransactionStaging::begin(&tx).unwrap();
    for index in 0..path_count {
        staging
            .stage_payload(&staged_payload(
                regular_entry(
                    &format!("/usr/share/tiny-fixture/{index:05}"),
                    trove_id,
                    b"x",
                ),
                "tiny-fixture",
                None,
                changeset_id,
            ))
            .unwrap();
    }
    staging.validate_and_reconcile().unwrap();
    let work = staging.finish().unwrap();
    tx.rollback().unwrap();
    work
}

fn query_rows(conn: &Connection, sql: &str) -> Vec<String> {
    conn.prepare(sql)
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
}

fn canonical_snapshot(conn: &Connection) -> Vec<Vec<String>> {
    [
        "SELECT json_array(parent_trove_id, name, description, is_installed)
         FROM components ORDER BY parent_trove_id, name",
        "SELECT json_array(path, payload_node_json, content_sha256, content_size, trove_id, component_id)
         FROM files ORDER BY path",
        "SELECT json_array(path, trove_id, component_id, sharing_policy, anchor_policy,
                           materialization_target_path, payload_node_json, content_sha256, content_size)
         FROM payload_claims ORDER BY path, trove_id",
        "SELECT json_array(path, trove_id, original_hash, current_hash, noreplace, ghost,
                           materialized, remove_on_upgrade, status, source)
         FROM config_files ORDER BY path",
        "SELECT json_array(changeset_id, path, sha256_hash, action)
         FROM file_history ORDER BY changeset_id, path, action",
    ]
    .into_iter()
    .map(|sql| query_rows(conn, sql))
    .collect()
}

fn reconcile_order_fixture(reverse: bool) -> Vec<Vec<String>> {
    let (_temp, mut conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let mut changeset = Changeset::new("order fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    let mut regular = regular_entry("/usr/lib/libfixture.so", first, b"shared");
    regular.node.source.kind = PayloadNodeKind::Regular {
        hardlink_identity: Some("fixture:hardlink-group".to_string()),
    };
    let mut rows = vec![
        staged_payload(
            directory_entry("/usr/share/shared", first)
                .with_claim_policy(PayloadSharingPolicy::Rpm),
            "first",
            Some("runtime"),
            changeset_id,
        ),
        staged_payload(
            directory_entry("/usr/share/shared", second)
                .with_claim_policy(PayloadSharingPolicy::Rpm),
            "second",
            Some("runtime"),
            changeset_id,
        ),
        staged_payload(regular, "first", Some("runtime"), changeset_id),
        staged_payload(
            hardlink_entry(
                "/usr/lib/libfixture.alias",
                "/usr/lib/libfixture.so",
                second,
            ),
            "second",
            Some("runtime"),
            changeset_id,
        ),
        staged_payload(
            regular_entry("/etc/fixture.conf", second, b"setting=true\n"),
            "second",
            Some("runtime"),
            changeset_id,
        ),
    ];
    if reverse {
        rows.reverse();
    }

    let tx = conn.transaction().unwrap();
    let mut staging = PackageTransactionStaging::begin(&tx).unwrap();
    staging
        .stage_component(first, "runtime", Some("runtime files"), true)
        .unwrap();
    staging
        .stage_component(second, "runtime", Some("runtime files"), true)
        .unwrap();
    for row in rows {
        staging.stage_payload(&row).unwrap();
    }
    staging
        .stage_config(&StagedConfigRow {
            config: ConfigFile::new(
                "/etc/fixture.conf".to_string(),
                second,
                crate::hash::sha256(b"setting=true\n"),
            ),
        })
        .unwrap();
    staging.validate_and_reconcile().unwrap();
    staging.finish().unwrap();
    tx.commit().unwrap();
    canonical_snapshot(&conn)
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
    {
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
    }
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

#[test]
fn many_tiny_files_keep_query_shapes_fixed() {
    let small = tiny_file_work(100);
    let large = tiny_file_work(2_500);

    assert_eq!(small.query_shapes, large.query_shapes);
    assert_eq!(
        small.validation_query_executions,
        large.validation_query_executions
    );
    assert_eq!(
        small.reconciliation_statement_executions,
        large.reconciliation_statement_executions
    );
    assert_eq!(large.rows_loaded - small.rows_loaded, 2 * (2_500 - 100));
    assert_eq!(
        large.total_statement_executions() - small.total_statement_executions(),
        2 * (2_500 - 100)
    );
}

#[test]
fn canonical_state_is_deterministic_across_staging_order() {
    assert_eq!(
        reconcile_order_fixture(false),
        reconcile_order_fixture(true)
    );
}

#[test]
fn duplicate_staged_payload_fails_before_canonical_rows_change() {
    let (_temp, mut conn) = create_test_db();
    let trove_id = insert_trove(&conn, "duplicate");
    let mut changeset = Changeset::new("duplicate fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    let row = staged_payload(
        regular_entry("/usr/bin/duplicate", trove_id, b"duplicate"),
        "duplicate",
        None,
        changeset_id,
    );

    let tx = conn.transaction().unwrap();
    {
        let mut staging = PackageTransactionStaging::begin(&tx).unwrap();
        staging.stage_payload(&row).unwrap();
        assert!(staging.stage_payload(&row).is_err());
    }
    tx.rollback().unwrap();

    assert!(
        FileEntry::find_by_path(&conn, "/usr/bin/duplicate")
            .unwrap()
            .is_none()
    );
}

#[test]
fn reconciliation_phase_failure_rolls_back_prior_canonical_writes() {
    let (_temp, mut conn) = create_test_db();
    let trove_id = insert_trove(&conn, "broken-hardlink");
    let mut changeset = Changeset::new("broken hardlink fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();

    let tx = conn.transaction().unwrap();
    {
        let mut staging = PackageTransactionStaging::begin(&tx).unwrap();
        staging
            .stage_payload(&staged_payload(
                hardlink_entry("/usr/bin/broken-edge", "/usr/bin/missing-target", trove_id),
                "broken-hardlink",
                None,
                changeset_id,
            ))
            .unwrap();
        let error = staging.validate_and_reconcile().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("has no regular materialization target /usr/bin/missing-target"),
            "unexpected staging error: {error}"
        );
    }
    tx.rollback().unwrap();

    let canonical_rows: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM files)
                    + (SELECT COUNT(*) FROM payload_claims)
                    + (SELECT COUNT(*) FROM file_history)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(canonical_rows, 0);
}
