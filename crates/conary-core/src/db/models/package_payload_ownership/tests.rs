// conary-core/src/db/models/package_payload_ownership/tests.rs

use super::*;
use crate::db::models::{PayloadClaimAnchorPolicy, TroveType};
use crate::db::testing::create_test_db;
use crate::payload::{PayloadIdentity, PayloadNode, PayloadTimestamp};
use crate::repository::versioning::VersionScheme;
use std::collections::BTreeMap;

fn insert_trove(conn: &Connection, name: &str) -> i64 {
    Trove::new(
        name.to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    )
    .insert(conn)
    .unwrap()
}

fn directory(mode: u32, mtime: i64) -> ResolvedPayloadNode {
    ResolvedPayloadNode {
        source: PayloadNode {
            kind: PayloadNodeKind::Directory,
            mode: libc::S_IFDIR | mode,
            user: PayloadIdentity::Numeric { id: 0 },
            group: PayloadIdentity::Numeric { id: 0 },
            mtime: PayloadTimestamp {
                seconds: mtime,
                nanoseconds: 0,
            },
            xattrs: BTreeMap::new(),
        },
        uid: 0,
        gid: 0,
    }
}

fn symlink(target: &str) -> ResolvedPayloadNode {
    let mut node = PayloadNode::regular(0o777);
    node.kind = PayloadNodeKind::Symlink {
        target: target.to_string(),
    };
    node.mode = libc::S_IFLNK | 0o777;
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn insert_anchor(conn: &Connection, path: &str, node: ResolvedPayloadNode, trove_id: i64) -> i64 {
    let mut anchor = FileEntry::new(path.to_string(), node, None, trove_id);
    anchor.insert(conn).unwrap()
}

fn insert_symlink_payload_claim(
    conn: &Connection,
    path: &str,
    trove_id: i64,
    node: ResolvedPayloadNode,
) {
    PayloadClaim::new_directory(path.to_string(), trove_id, node)
        .unwrap()
        .with_anchor_policy(PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory)
        .insert(conn)
        .unwrap();
}

#[test]
fn non_anchor_package_projects_its_exact_payload_claim() {
    let (_temp, conn) = create_test_db();
    let anchor_owner = insert_trove(&conn, "anchor");
    let claimant = insert_trove(&conn, "claimant");
    let anchor_node = directory(0o755, 1);
    let claimed_node = directory(0o700, 99);
    let materialized_file_id = insert_anchor(&conn, "/shared", anchor_node, anchor_owner);
    PayloadClaim::new_directory("/shared".to_string(), claimant, claimed_node.clone())
        .unwrap()
        .insert(&conn)
        .unwrap();

    let ownership = PackagePayloadOwnership::load(&conn, claimant).unwrap();

    assert_eq!(ownership.entries().len(), 1);
    assert_eq!(ownership.entries()[0].path, "/shared");
    assert_eq!(ownership.entries()[0].node, claimed_node);
    assert_eq!(
        ownership.entries()[0].materialized_file_id,
        Some(materialized_file_id)
    );
    assert_eq!(ownership.lifecycle_paths(), &["/shared".to_string()]);
    assert!(ownership.materialized_removal_paths().is_empty());
}

#[test]
fn removing_symlink_anchor_owner_first_reanchors_without_rewriting_leaf() {
    let (_temp, conn) = create_test_db();
    let symlink_owner = insert_trove(&conn, "symlink-owner");
    let directory_owner = insert_trove(&conn, "directory-owner");
    let leaf = symlink("/real");
    insert_anchor(&conn, "/shared", leaf.clone(), symlink_owner);
    insert_symlink_payload_claim(&conn, "/shared", directory_owner, directory(0o755, 2));

    let symlink_projection = PackagePayloadOwnership::load(&conn, symlink_owner).unwrap();
    assert_eq!(symlink_projection.entries().len(), 1);
    assert!(matches!(
        symlink_projection.entries()[0].node.source.kind,
        PayloadNodeKind::Symlink { .. }
    ));
    assert!(symlink_projection.materialized_removal_paths().is_empty());

    Trove::delete(&conn, symlink_owner).unwrap();

    let reanchored = FileEntry::find_by_path(&conn, "/shared").unwrap().unwrap();
    assert_eq!(reanchored.trove_id, directory_owner);
    assert_eq!(reanchored.node, leaf);
    assert_eq!(
        PackagePayloadOwnership::load(&conn, directory_owner)
            .unwrap()
            .materialized_removal_paths(),
        &["/shared".to_string()]
    );

    Trove::delete(&conn, directory_owner).unwrap();
    assert!(FileEntry::find_by_path(&conn, "/shared").unwrap().is_none());
}

#[test]
fn removing_payload_claimant_first_retains_symlink_owner_and_leaf() {
    let (_temp, conn) = create_test_db();
    let symlink_owner = insert_trove(&conn, "symlink-owner");
    let directory_owner = insert_trove(&conn, "directory-owner");
    let leaf = symlink("/real");
    insert_anchor(&conn, "/shared", leaf.clone(), symlink_owner);
    insert_symlink_payload_claim(&conn, "/shared", directory_owner, directory(0o755, 2));

    Trove::delete(&conn, directory_owner).unwrap();

    let retained = FileEntry::find_by_path(&conn, "/shared").unwrap().unwrap();
    assert_eq!(retained.trove_id, symlink_owner);
    assert_eq!(retained.node, leaf);
    let claims = PayloadClaim::find_by_path(&conn, "/shared").unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].trove_id, symlink_owner);
    assert!(matches!(
        claims[0].node.source.kind,
        PayloadNodeKind::Symlink { .. }
    ));
    assert_eq!(
        PackagePayloadOwnership::load(&conn, symlink_owner)
            .unwrap()
            .materialized_removal_paths(),
        &["/shared".to_string()]
    );
}

#[test]
fn same_trove_symlink_anchor_and_payload_claim_project_once() {
    let (_temp, conn) = create_test_db();
    let package = insert_trove(&conn, "package");
    let materialized_file_id = insert_anchor(&conn, "/shared", symlink("/real"), package);
    let claim = directory(0o755, 2);
    insert_symlink_payload_claim(&conn, "/shared", package, claim.clone());

    let ownership = PackagePayloadOwnership::load(&conn, package).unwrap();

    assert_eq!(ownership.entries().len(), 1);
    assert_eq!(ownership.entries()[0].node, claim);
    assert_eq!(
        ownership.entries()[0].materialized_file_id,
        Some(materialized_file_id)
    );
    assert_eq!(
        ownership.materialized_removal_paths(),
        &["/shared".to_string()]
    );
}

#[test]
fn removing_through_symlink_claim_before_target_owner_releases_only_the_target_edge() {
    let (_temp, conn) = create_test_db();
    let target_owner = insert_trove(&conn, "target-owner");
    let symlink_owner = insert_trove(&conn, "symlink-owner");
    let claimant = insert_trove(&conn, "rpm-claimant");
    let target = directory(0o755, 1);
    insert_anchor(&conn, "/real", target, target_owner);
    insert_anchor(&conn, "/shared", symlink("/real"), symlink_owner);
    PayloadClaim::new_directory("/shared".to_string(), claimant, directory(0o700, 2))
        .unwrap()
        .with_anchor_policy(PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory)
        .with_materialization_target(Some("/real".to_string()))
        .insert(&conn)
        .unwrap();

    Trove::delete(&conn, claimant).unwrap();

    let retained_target = FileEntry::find_by_path(&conn, "/real").unwrap().unwrap();
    assert_eq!(retained_target.trove_id, target_owner);
    assert!(
        PayloadClaim::find_retaining_path(&conn, "/real")
            .unwrap()
            .iter()
            .all(|claim| claim.trove_id == target_owner)
    );
    assert_eq!(
        PackagePayloadOwnership::load(&conn, target_owner)
            .unwrap()
            .materialized_removal_paths(),
        &["/real".to_string()]
    );

    Trove::delete(&conn, target_owner).unwrap();
    assert!(FileEntry::find_by_path(&conn, "/real").unwrap().is_none());
    assert!(FileEntry::find_by_path(&conn, "/shared").unwrap().is_some());
}
