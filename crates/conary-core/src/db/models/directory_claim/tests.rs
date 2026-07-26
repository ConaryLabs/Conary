// conary-core/src/db/models/directory_claim/tests.rs

use super::*;
use crate::db::models::{Component, Trove, TroveType};
use crate::db::testing::create_test_db;
use crate::payload::{PayloadIdentity, PayloadNode, PayloadTimestamp};
use crate::repository::versioning::VersionScheme;
use std::collections::BTreeMap;

fn insert_trove(conn: &Connection, name: &str) -> i64 {
    Trove::new(
        name.to_string(),
        "1".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    )
    .insert(conn)
    .unwrap()
}

fn insert_component(conn: &Connection, trove_id: i64, name: &str) -> i64 {
    Component::new(trove_id, name.to_string())
        .insert(conn)
        .unwrap()
}

fn directory(mode: u32, uid: u64, gid: u64, mtime: i64) -> ResolvedPayloadNode {
    ResolvedPayloadNode {
        source: PayloadNode {
            kind: PayloadNodeKind::Directory,
            mode: libc::S_IFDIR | mode,
            user: PayloadIdentity::Numeric { id: uid },
            group: PayloadIdentity::Numeric { id: gid },
            mtime: PayloadTimestamp {
                seconds: mtime,
                nanoseconds: 0,
            },
            xattrs: BTreeMap::new(),
        },
        uid,
        gid,
    }
}

fn symlink(target: &str, uid: u64, gid: u64, mtime: i64) -> ResolvedPayloadNode {
    let mut node = PayloadNode::regular(0o777);
    node.kind = PayloadNodeKind::Symlink {
        target: target.to_string(),
    };
    node.mode = libc::S_IFLNK | 0o777;
    node.user = PayloadIdentity::Numeric { id: uid };
    node.group = PayloadIdentity::Numeric { id: gid };
    node.mtime = PayloadTimestamp {
        seconds: mtime,
        nanoseconds: 0,
    };
    ResolvedPayloadNode {
        source: node,
        uid,
        gid,
    }
}

fn insert_anchor(
    conn: &Connection,
    path: &str,
    trove_id: i64,
    node: &ResolvedPayloadNode,
) -> FileEntry {
    let mut file = FileEntry::new(path.to_string(), node.clone(), None, trove_id);
    file.insert(conn).unwrap();
    file
}

#[test]
fn exact_directory_claims_are_many_to_many_and_keep_source_metadata() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let anchor_node = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/etc", first, &anchor_node);
    let second_node = directory(0o755, 0, 0, 99);

    DirectoryClaim::new("/etc".to_string(), first, anchor_node.clone())
        .unwrap()
        .insert(&conn)
        .unwrap();
    DirectoryClaim::new("/etc".to_string(), second, second_node.clone())
        .unwrap()
        .insert(&conn)
        .unwrap();

    let claims = DirectoryClaim::find_by_path(&conn, "/etc").unwrap();
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0].node, anchor_node);
    assert_eq!(claims[1].node, second_node);
}

#[test]
fn typed_materialization_target_reanchors_and_guards_the_target_directory() {
    let (_temp, conn) = create_test_db();
    let target_owner = insert_trove(&conn, "target-owner");
    let symlink_owner = insert_trove(&conn, "symlink-owner");
    let claimant = insert_trove(&conn, "claimant");
    let target_node = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/real", target_owner, &target_node);
    let symlink_node = symlink("/real", 0, 0, 1);
    insert_anchor(&conn, "/shared", symlink_owner, &symlink_node);

    DirectoryClaim::new("/shared".to_string(), claimant, directory(0o700, 0, 0, 2))
        .unwrap()
        .with_anchor_policy(DirectoryClaimAnchorPolicy::DirectoryOrSymlinkToDirectory)
        .with_materialization_target(Some("/real".to_string()))
        .insert(&conn)
        .unwrap();

    let retainers = DirectoryClaim::find_retaining_path(&conn, "/real").unwrap();
    assert_eq!(
        retainers
            .iter()
            .map(|claim| (claim.path.as_str(), claim.trove_id))
            .collect::<Vec<_>>(),
        vec![("/real", target_owner), ("/shared", claimant)]
    );
    let target_json = super::super::file_entry::canonical_node_json(&symlink_node).unwrap();
    assert!(
        conn.execute(
            "UPDATE files SET payload_node_json = ?1 WHERE path = '/real'",
            [target_json],
        )
        .unwrap_err()
        .to_string()
        .contains("materialized directory anchor violates")
    );

    Trove::delete(&conn, target_owner).unwrap();
    assert_eq!(
        FileEntry::find_by_path(&conn, "/real")
            .unwrap()
            .unwrap()
            .trove_id,
        claimant
    );
    Trove::delete(&conn, claimant).unwrap();
    assert!(FileEntry::find_by_path(&conn, "/real").unwrap().is_none());
}

#[test]
fn conflicting_directory_metadata_is_retained_as_independent_claim_authority() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let anchor_node = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/etc", first, &anchor_node);
    DirectoryClaim::new("/etc".to_string(), first, anchor_node.clone())
        .unwrap()
        .insert(&conn)
        .unwrap();

    let second_node = directory(0o700, 1, 2, 1);
    DirectoryClaim::new("/etc".to_string(), second, second_node.clone())
        .unwrap()
        .insert(&conn)
        .unwrap();

    let claims = DirectoryClaim::find_by_path(&conn, "/etc").unwrap();
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[1].node, second_node);
    assert_eq!(
        FileEntry::find_by_path(&conn, "/etc")
            .unwrap()
            .unwrap()
            .node,
        anchor_node
    );
}

#[test]
fn deleting_anchor_transfers_to_lowest_remaining_claim_without_rewriting_metadata() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let third = insert_trove(&conn, "third");
    let anchor_node = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/etc", first, &anchor_node);
    for (trove_id, mtime) in [(first, 1), (third, 3), (second, 2)] {
        DirectoryClaim::new("/etc".to_string(), trove_id, directory(0o755, 0, 0, mtime))
            .unwrap()
            .insert(&conn)
            .unwrap();
    }
    let node_json_before: String = conn
        .query_row(
            "SELECT payload_node_json FROM files WHERE path = '/etc'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    conn.execute("DELETE FROM troves WHERE id = ?1", [first])
        .unwrap();

    let anchor = FileEntry::find_by_path(&conn, "/etc").unwrap().unwrap();
    let node_json_after: String = conn
        .query_row(
            "SELECT payload_node_json FROM files WHERE path = '/etc'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(anchor.trove_id, second);
    assert_eq!(anchor.node, anchor_node);
    assert_eq!(node_json_after, node_json_before);
    assert_eq!(
        DirectoryClaim::find_by_path(&conn, "/etc")
            .unwrap()
            .iter()
            .map(|claim| claim.trove_id)
            .collect::<Vec<_>>(),
        vec![second, third]
    );

    Trove::delete(&conn, second).unwrap();
    assert_eq!(
        FileEntry::find_by_path(&conn, "/etc")
            .unwrap()
            .unwrap()
            .trove_id,
        third
    );
    Trove::delete(&conn, third).unwrap();
    assert!(FileEntry::find_by_path(&conn, "/etc").unwrap().is_none());
}

#[test]
fn deleting_non_anchor_claim_keeps_the_materialized_anchor() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let node = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/etc", first, &node);
    for trove_id in [first, second] {
        DirectoryClaim::new("/etc".to_string(), trove_id, node.clone())
            .unwrap()
            .insert(&conn)
            .unwrap();
    }

    Trove::delete(&conn, second).unwrap();

    assert_eq!(
        FileEntry::find_by_path(&conn, "/etc")
            .unwrap()
            .unwrap()
            .trove_id,
        first
    );
    assert_eq!(
        DirectoryClaim::find_by_path(&conn, "/etc").unwrap().len(),
        1
    );
}

#[test]
fn schema_rejects_orphan_non_directory_and_duplicate_claims() {
    let (_temp, conn) = create_test_db();
    let package = insert_trove(&conn, "package");
    let directory = directory(0o755, 0, 0, 1);
    let directory_json = super::super::file_entry::canonical_node_json(&directory).unwrap();

    assert!(
        conn.execute(
            "INSERT INTO directory_claims (path, trove_id, payload_node_json)
             VALUES ('/missing', ?1, ?2)",
            params![package, &directory_json],
        )
        .is_err()
    );

    insert_anchor(&conn, "/etc", package, &directory);
    assert!(
        conn.execute(
            "INSERT INTO directory_claims (path, trove_id, payload_node_json)
             VALUES ('/etc', ?1, ?2)",
            params![package, &directory_json],
        )
        .is_err()
    );

    let other = insert_trove(&conn, "other");
    let regular = ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap();
    let regular_json = super::super::file_entry::canonical_node_json(&regular).unwrap();
    assert!(
        conn.execute(
            "INSERT INTO directory_claims (path, trove_id, payload_node_json)
             VALUES ('/etc', ?1, ?2)",
            params![other, regular_json],
        )
        .is_err()
    );
}

#[test]
fn file_entry_shared_insert_preserves_anchor_and_records_incoming_claim() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let anchor_node = directory(0o755, 0, 0, 1);
    let anchor = insert_anchor(&conn, "/etc", first, &anchor_node);
    let mut incoming = FileEntry::new("/etc".to_string(), directory(0o755, 0, 0, 99), None, second);

    let returned_id = incoming
        .insert_or_replace(
            &conn,
            super::super::file_entry::ExistingDirectoryMaterialization::ApplyIncoming,
        )
        .unwrap();

    assert_eq!(returned_id, anchor.id.unwrap());
    let materialized = FileEntry::find_by_path(&conn, "/etc").unwrap().unwrap();
    assert_eq!(materialized.trove_id, first);
    assert_eq!(materialized.node, directory(0o755, 0, 0, 99));
    assert_eq!(
        DirectoryClaim::find_by_path(&conn, "/etc")
            .unwrap()
            .iter()
            .map(|claim| claim.trove_id)
            .collect::<Vec<_>>(),
        vec![first, second]
    );
}

#[test]
fn latest_directory_install_updates_materialized_metadata_without_erasing_peer_claims() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let third = insert_trove(&conn, "third");
    let node = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/etc", first, &node);
    DirectoryClaim::new("/etc".to_string(), second, node)
        .unwrap()
        .insert(&conn)
        .unwrap();
    let mut replacement =
        FileEntry::new("/etc".to_string(), directory(0o700, 0, 0, 1), None, third);

    replacement
        .insert_or_replace(
            &conn,
            super::super::file_entry::ExistingDirectoryMaterialization::ApplyIncoming,
        )
        .unwrap();

    assert_eq!(
        FileEntry::find_by_path(&conn, "/etc")
            .unwrap()
            .unwrap()
            .node
            .source
            .mode,
        libc::S_IFDIR | 0o700
    );
    assert_eq!(
        DirectoryClaim::find_by_path(&conn, "/etc").unwrap().len(),
        3
    );
}

#[test]
fn anchor_delete_transfers_exact_component_and_preserves_materialized_node_bytes() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let first_component = insert_component(&conn, first, "first-runtime");
    let second_component = insert_component(&conn, second, "second-runtime");
    let first_node = directory(0o755, 0, 0, 1);
    let mut anchor = FileEntry::new("/etc".to_string(), first_node, None, first);
    anchor.component_id = Some(first_component);
    anchor.insert(&conn).unwrap();
    let second_node = directory(0o700, 42, 43, 9);
    let mut incoming = FileEntry::new("/etc".to_string(), second_node.clone(), None, second);
    incoming.component_id = Some(second_component);
    incoming
        .insert_or_replace(
            &conn,
            super::super::file_entry::ExistingDirectoryMaterialization::ApplyIncoming,
        )
        .unwrap();
    let node_json_before: String = conn
        .query_row(
            "SELECT payload_node_json FROM files WHERE path = '/etc'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    Trove::delete(&conn, first).unwrap();

    let materialized = FileEntry::find_by_path(&conn, "/etc").unwrap().unwrap();
    let node_json_after: String = conn
        .query_row(
            "SELECT payload_node_json FROM files WHERE path = '/etc'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(materialized.trove_id, second);
    assert_eq!(materialized.component_id, Some(second_component));
    assert_eq!(materialized.node, second_node);
    assert_eq!(node_json_after, node_json_before);
}

#[test]
fn direct_file_delete_refuses_to_cascade_peer_directory_claims() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let node = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/etc", first, &node);
    DirectoryClaim::new("/etc".to_string(), second, node)
        .unwrap()
        .insert(&conn)
        .unwrap();

    let error = FileEntry::delete(&conn, "/etc").unwrap_err().to_string();

    assert!(
        error.contains("while peer package claims remain"),
        "{error}"
    );
    assert!(
        conn.execute("DELETE FROM files WHERE path = '/etc'", [])
            .is_err(),
        "schema must reject raw deletion too"
    );
    assert!(FileEntry::find_by_path(&conn, "/etc").unwrap().is_some());
    assert_eq!(
        DirectoryClaim::find_by_path(&conn, "/etc").unwrap().len(),
        2
    );
}

#[test]
fn typed_claim_delete_reanchors_inside_the_caller_transaction() {
    let (_temp, mut conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let first_component = insert_component(&conn, first, "first-runtime");
    let second_component = insert_component(&conn, second, "second-runtime");
    let mut anchor = FileEntry::new("/etc".to_string(), directory(0o755, 0, 0, 1), None, first);
    anchor.component_id = Some(first_component);
    anchor.insert(&conn).unwrap();
    DirectoryClaim::new("/etc".to_string(), second, directory(0o700, 1, 2, 3))
        .unwrap()
        .with_component(Some(second_component))
        .insert(&conn)
        .unwrap();

    let tx = conn.transaction().unwrap();
    DirectoryClaim::delete(&tx, "/etc", first).unwrap();
    tx.commit().unwrap();

    let materialized = FileEntry::find_by_path(&conn, "/etc").unwrap().unwrap();
    assert_eq!(materialized.trove_id, second);
    assert_eq!(materialized.component_id, Some(second_component));
    assert_eq!(
        DirectoryClaim::find_by_path(&conn, "/etc").unwrap().len(),
        1
    );
}

#[test]
fn preserve_existing_records_the_incoming_claim_without_rewriting_materialized_state() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let materialized_node = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/etc", first, &materialized_node);
    let claimed_node = directory(0o700, 42, 43, 99);
    let mut incoming = FileEntry::new("/etc".to_string(), claimed_node.clone(), None, second);

    incoming
        .insert_or_replace(
            &conn,
            super::super::file_entry::ExistingDirectoryMaterialization::PreserveExistingDirectory,
        )
        .unwrap();

    assert_eq!(
        FileEntry::find_by_path(&conn, "/etc")
            .unwrap()
            .unwrap()
            .node,
        materialized_node
    );
    let claims = DirectoryClaim::find_by_path(&conn, "/etc").unwrap();
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[1].trove_id, second);
    assert_eq!(claims[1].node, claimed_node);
}

#[test]
fn bare_connection_directory_insert_is_atomic_when_claim_persistence_fails() {
    let (_temp, conn) = create_test_db();
    let owner = insert_trove(&conn, "owner");
    conn.execute_batch(
        "CREATE TRIGGER inject_directory_claim_insert_failure
         BEFORE INSERT ON directory_claims
         BEGIN
             SELECT RAISE(ABORT, 'injected directory claim failure');
         END;",
    )
    .unwrap();
    let mut file = FileEntry::new(
        "/atomic".to_string(),
        directory(0o755, 0, 0, 1),
        None,
        owner,
    );

    assert!(file.insert(&conn).is_err());
    assert!(FileEntry::find_by_path(&conn, "/atomic").unwrap().is_none());
    assert!(
        DirectoryClaim::find_by_path(&conn, "/atomic")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn shared_directory_update_rolls_back_the_new_claim_when_materialization_fails() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let original = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/etc", first, &original);
    conn.execute_batch(
        "CREATE TRIGGER inject_directory_materialization_failure
         BEFORE UPDATE OF payload_node_json ON files
         WHEN OLD.path = '/etc'
         BEGIN
             SELECT RAISE(ABORT, 'injected materialization failure');
         END;",
    )
    .unwrap();
    let mut incoming = FileEntry::new("/etc".to_string(), directory(0o700, 1, 2, 3), None, second);

    assert!(
        incoming
            .insert_or_replace(
                &conn,
                super::super::file_entry::ExistingDirectoryMaterialization::ApplyIncoming,
            )
            .is_err()
    );
    assert_eq!(
        FileEntry::find_by_path(&conn, "/etc")
            .unwrap()
            .unwrap()
            .node,
        original
    );
    assert_eq!(
        DirectoryClaim::find_by_path(&conn, "/etc")
            .unwrap()
            .iter()
            .map(|claim| claim.trove_id)
            .collect::<Vec<_>>(),
        vec![first]
    );
}

#[test]
fn files_schema_rejects_a_component_from_another_trove() {
    let (_temp, conn) = create_test_db();
    let owner = insert_trove(&conn, "owner");
    let peer = insert_trove(&conn, "peer");
    let peer_component = insert_component(&conn, peer, "peer-runtime");
    let mut file = FileEntry::new(
        "/usr/bin/owned".to_string(),
        ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o755)).unwrap(),
        None,
        owner,
    );
    file.component_id = Some(peer_component);

    assert!(file.insert(&conn).is_err());
    assert!(
        FileEntry::find_by_path(&conn, "/usr/bin/owned")
            .unwrap()
            .is_none()
    );
}

#[test]
fn materialized_directory_restore_persists_exact_canonical_bytes_and_component() {
    let (_temp, mut conn) = create_test_db();
    let owner = insert_trove(&conn, "owner");
    let component = insert_component(&conn, owner, "runtime");
    let node = directory(0o711, 42, 43, 123);
    let expected_json = super::super::file_entry::canonical_node_json(&node).unwrap();

    let tx = conn.transaction().unwrap();
    FileEntry::restore_materialized_directory_anchor(
        &tx,
        "/restored",
        &node,
        DirectoryClaimAnchorPolicy::Directory,
        owner,
        Some(component),
        "2026-07-25 12:00:00",
    )
    .unwrap();
    DirectoryClaim::new("/restored".to_string(), owner, node.clone())
        .unwrap()
        .with_component(Some(component))
        .insert(&tx)
        .unwrap();
    tx.commit().unwrap();

    let raw_json: String = conn
        .query_row(
            "SELECT payload_node_json FROM files WHERE path = '/restored'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let restored = FileEntry::find_by_path(&conn, "/restored")
        .unwrap()
        .unwrap();
    assert_eq!(raw_json, expected_json);
    assert_eq!(restored.node, node);
    assert_eq!(restored.trove_id, owner);
    assert_eq!(restored.component_id, Some(component));
    assert_eq!(
        restored.installed_at.as_deref(),
        Some("2026-07-25 12:00:00")
    );
}

#[test]
fn selected_root_reconciliation_never_broadens_existing_claim_policy() {
    let (_temp, mut conn) = create_test_db();
    let owner = insert_trove(&conn, "owner");
    let incoming = insert_trove(&conn, "incoming");
    let original = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/shared", owner, &original);
    DirectoryClaim::new("/shared".to_string(), incoming, directory(0o700, 1, 2, 3))
        .unwrap()
        .insert(&conn)
        .unwrap();

    let tx = conn.transaction().unwrap();
    let error = FileEntry::reconcile_directory_materialization(
        &tx,
        "/shared",
        &symlink("/real", 0, 0, 4),
        super::super::file_entry::ExistingDirectoryMaterialization::
            PreserveExistingDirectoryOrSymlinkToDirectory,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("incompatible with claim"),
        "{error}"
    );
    tx.rollback().unwrap();

    let claims = DirectoryClaim::find_by_path(&conn, "/shared").unwrap();
    assert!(
        claims
            .iter()
            .all(|claim| claim.anchor_policy == DirectoryClaimAnchorPolicy::Directory)
    );
    assert_eq!(
        FileEntry::find_by_path(&conn, "/shared")
            .unwrap()
            .unwrap()
            .node,
        original
    );
}

#[test]
fn materialized_directory_restore_accepts_an_existing_symlink_anchor() {
    let (_temp, mut conn) = create_test_db();
    let owner = insert_trove(&conn, "owner");
    let original = symlink("/old-target", 0, 0, 1);
    let mut anchor = FileEntry::new("/shared".to_string(), original, None, owner);
    anchor.insert(&conn).unwrap();
    let restored = symlink("/restored-target", 42, 43, 9);

    let tx = conn.transaction().unwrap();
    FileEntry::restore_materialized_directory_anchor(
        &tx,
        "/shared",
        &restored,
        DirectoryClaimAnchorPolicy::DirectoryOrSymlinkToDirectory,
        owner,
        None,
        "2026-07-25 13:00:00",
    )
    .unwrap();
    tx.commit().unwrap();

    assert_eq!(
        FileEntry::find_by_path(&conn, "/shared")
            .unwrap()
            .unwrap()
            .node,
        restored
    );
}

#[test]
fn schema_guards_typed_claim_policies_against_raw_anchor_drift() {
    let (_temp, conn) = create_test_db();
    let owner = insert_trove(&conn, "owner");
    let original = directory(0o755, 0, 0, 1);
    insert_anchor(&conn, "/shared", owner, &original);
    let symlink_json =
        super::super::file_entry::canonical_node_json(&symlink("/real", 0, 0, 2)).unwrap();

    assert!(
        conn.execute(
            "UPDATE files SET payload_node_json = ?1 WHERE path = '/shared'",
            [symlink_json],
        )
        .is_err(),
        "raw SQL must not move a directory-only claim onto a symlink anchor"
    );

    conn.execute(
        "UPDATE directory_claims
         SET anchor_policy = 'directory-or-symlink-to-directory'
         WHERE path = '/shared' AND trove_id = ?1",
        [owner],
    )
    .unwrap();
    let symlink_json =
        super::super::file_entry::canonical_node_json(&symlink("/real", 0, 0, 2)).unwrap();
    conn.execute(
        "UPDATE files SET payload_node_json = ?1 WHERE path = '/shared'",
        [symlink_json],
    )
    .unwrap();

    assert!(
        conn.execute(
            "UPDATE directory_claims
             SET anchor_policy = 'directory'
             WHERE path = '/shared' AND trove_id = ?1",
            [owner],
        )
        .is_err(),
        "raw SQL must not narrow a claim below its current anchor kind"
    );
}
