// conary-core/src/db/models/payload_claim/sharing_tests.rs

use super::*;
use crate::db::models::{
    ExistingDirectoryMaterialization, PackagePayloadOwnership, Trove, TroveType,
};
use crate::db::testing::create_test_db;
use crate::payload::{PayloadNode, PayloadNodeKind, ResolvedPayloadNode};
use crate::repository::versioning::VersionScheme;

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

fn regular(bytes: &[u8]) -> (ResolvedPayloadNode, PayloadContentAuthority) {
    (
        ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
        PayloadContentAuthority {
            sha256: crate::hash::sha256(bytes),
            size: bytes.len() as u64,
        },
    )
}

fn hardlink(target: &str, identity: &str) -> ResolvedPayloadNode {
    let mut node = PayloadNode::regular(0o644);
    node.kind = PayloadNodeKind::Hardlink {
        target: target.to_string(),
        identity: identity.to_string(),
    };
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

#[test]
fn replacing_one_claimed_materialization_preserves_anchor_and_claim_history() {
    let (_temp, conn) = create_test_db();
    let owner = insert_trove(&conn, "owner");
    let path = "/usr/bin/tool";
    let (old_node, old_content) = regular(b"old");
    let mut entry = FileEntry::new(path.to_string(), old_node, Some(old_content), owner);
    entry.insert(&conn).unwrap();
    let old_anchor = FileEntry::find_by_path(&conn, path).unwrap().unwrap();
    let old_claim = PayloadClaim::find_by_path(&conn, path).unwrap().remove(0);

    let (mut replacement_node, replacement_content) = regular(b"new exact bytes");
    replacement_node.source.mode = libc::S_IFREG | 0o755;
    assert!(
        FileEntry::replace_claimed_selected_root_materialization(
            &conn,
            path,
            owner,
            &replacement_node,
            Some(&replacement_content),
        )
        .unwrap()
    );

    let anchor = FileEntry::find_by_path(&conn, path).unwrap().unwrap();
    assert_eq!(anchor.id, old_anchor.id);
    assert_eq!(anchor.trove_id, owner);
    assert_eq!(anchor.installed_at, old_anchor.installed_at);
    assert_eq!(anchor.node, replacement_node);
    assert_eq!(anchor.content, Some(replacement_content.clone()));
    let claim = PayloadClaim::find_by_path(&conn, path).unwrap().remove(0);
    assert_eq!(claim.claimed_at, old_claim.claimed_at);
    assert_eq!(claim.node, replacement_node);
    assert_eq!(claim.content, Some(replacement_content));
}

#[test]
fn replacing_one_shared_claim_rejects_peer_incompatibility_atomically() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let path = "/usr/share/licenses/shared";
    let (node, content) = regular(b"shared");
    let mut first_entry =
        FileEntry::new(path.to_string(), node.clone(), Some(content.clone()), first)
            .with_claim_policy(PayloadSharingPolicy::Rpm);
    first_entry.insert(&conn).unwrap();
    let mut second_entry = FileEntry::new(
        path.to_string(),
        node.clone(),
        Some(content.clone()),
        second,
    )
    .with_claim_policy(PayloadSharingPolicy::Rpm);
    second_entry
        .insert_or_replace(&conn, ExistingDirectoryMaterialization::ApplyIncoming)
        .unwrap();
    let anchor_before = FileEntry::find_by_path(&conn, path).unwrap().unwrap();
    let claims_before = PayloadClaim::find_by_path(&conn, path).unwrap();
    let (_, altered_content) = regular(b"altered");

    let error = FileEntry::replace_claimed_selected_root_materialization(
        &conn,
        path,
        first,
        &node,
        Some(&altered_content),
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("incompatible with trove"),
        "{error}"
    );
    assert_eq!(
        FileEntry::find_by_path(&conn, path).unwrap().unwrap(),
        anchor_before
    );
    assert_eq!(
        PayloadClaim::find_by_path(&conn, path).unwrap(),
        claims_before
    );
}

#[test]
fn upgrading_one_shared_rpm_owner_preserves_the_peer_and_replaces_only_its_claim() {
    let (_temp, conn) = create_test_db();
    let peer = insert_trove(&conn, "peer");
    let old = insert_trove(&conn, "package-old");
    let replacement = insert_trove(&conn, "package-new");
    let path = "/usr/share/licenses/shared";
    let (node, content) = regular(b"shared");
    let mut peer_entry =
        FileEntry::new(path.to_string(), node.clone(), Some(content.clone()), peer)
            .with_claim_policy(PayloadSharingPolicy::Rpm);
    peer_entry.insert(&conn).unwrap();
    let mut old_entry = FileEntry::new(path.to_string(), node.clone(), Some(content.clone()), old)
        .with_claim_policy(PayloadSharingPolicy::Rpm);
    old_entry
        .insert_or_replace(&conn, ExistingDirectoryMaterialization::ApplyIncoming)
        .unwrap();

    let (_, changed_content) = regular(b"altered");
    let mut incompatible_replacement = FileEntry::new(
        path.to_string(),
        node.clone(),
        Some(changed_content),
        replacement,
    )
    .with_claim_policy(PayloadSharingPolicy::Rpm);
    let error = incompatible_replacement
        .insert_or_replace(&conn, ExistingDirectoryMaterialization::ApplyIncoming)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot replace shared payload materialization"),
        "{error}"
    );

    let mut replacement_entry = FileEntry::new(path.to_string(), node, Some(content), replacement)
        .with_claim_policy(PayloadSharingPolicy::Rpm);
    replacement_entry
        .insert_or_replace(&conn, ExistingDirectoryMaterialization::ApplyIncoming)
        .unwrap();
    Trove::delete(&conn, old).unwrap();

    assert_eq!(
        PayloadClaim::find_by_path(&conn, path)
            .unwrap()
            .into_iter()
            .map(|claim| claim.trove_id)
            .collect::<Vec<_>>(),
        vec![peer, replacement]
    );
    assert_eq!(
        FileEntry::find_by_path(&conn, path)
            .unwrap()
            .unwrap()
            .trove_id,
        peer
    );
    for trove_id in [peer, replacement] {
        let ownership = PackagePayloadOwnership::load(&conn, trove_id).unwrap();
        assert!(ownership.materialized_removal_paths().is_empty());
    }
}

#[test]
fn compatible_rpm_symlink_and_device_claims_share_typed_materializations() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");

    let mut first_link = PayloadNode::regular(0o777);
    first_link.kind = PayloadNodeKind::Symlink {
        target: "../lib/target".to_string(),
    };
    first_link.mode = libc::S_IFLNK | 0o777;
    let mut second_link = first_link.clone();
    second_link.mode = libc::S_IFLNK;

    let mut first_device = PayloadNode::regular(0o600);
    first_device.kind = PayloadNodeKind::CharacterDevice {
        major: 10,
        minor: 200,
    };
    first_device.mode = libc::S_IFCHR | 0o600;
    let second_device = first_device.clone();

    for (path, first_node, second_node) in [
        ("/usr/lib/shared-link", first_link, second_link),
        ("/usr/lib/shared-device", first_device, second_device),
    ] {
        let mut first_entry = FileEntry::new(
            path.to_string(),
            ResolvedPayloadNode::from_numeric_source(first_node).unwrap(),
            None,
            first,
        )
        .with_claim_policy(PayloadSharingPolicy::Rpm);
        let anchor_id = first_entry.insert(&conn).unwrap();
        let mut second_entry = FileEntry::new(
            path.to_string(),
            ResolvedPayloadNode::from_numeric_source(second_node).unwrap(),
            None,
            second,
        )
        .with_claim_policy(PayloadSharingPolicy::Rpm);
        assert_eq!(
            second_entry
                .insert_or_replace(&conn, ExistingDirectoryMaterialization::ApplyIncoming)
                .unwrap(),
            anchor_id
        );
        assert_eq!(PayloadClaim::find_by_path(&conn, path).unwrap().len(), 2);
        assert_eq!(
            FileEntry::find_by_path(&conn, path)
                .unwrap()
                .unwrap()
                .trove_id,
            first
        );
    }
}

#[test]
fn shared_rpm_target_extends_one_canonical_materialized_hardlink_graph() {
    let (_temp, conn) = create_test_db();
    let first = insert_trove(&conn, "first");
    let second = insert_trove(&conn, "second");
    let target = "/usr/share/hardlink-target";
    let (mut first_node, content) = regular(b"shared");
    first_node.source.kind = PayloadNodeKind::Regular {
        hardlink_identity: Some("rpm:1:7".to_string()),
    };
    let mut first_target =
        FileEntry::new(target.to_string(), first_node, Some(content.clone()), first)
            .with_claim_policy(PayloadSharingPolicy::Rpm);
    first_target.insert(&conn).unwrap();
    let mut first_edge = FileEntry::new(
        "/usr/share/first-edge".to_string(),
        hardlink(target, "rpm:1:7"),
        None,
        first,
    )
    .with_claim_policy(PayloadSharingPolicy::Rpm);
    first_edge.insert(&conn).unwrap();

    let (mut second_node, _) = regular(b"shared");
    second_node.source.kind = PayloadNodeKind::Regular {
        hardlink_identity: Some("rpm:9:42".to_string()),
    };
    let mut second_target = FileEntry::new(target.to_string(), second_node, Some(content), second)
        .with_claim_policy(PayloadSharingPolicy::Rpm);
    second_target
        .insert_or_replace(&conn, ExistingDirectoryMaterialization::ApplyIncoming)
        .unwrap();
    let mut second_edge = FileEntry::new(
        "/usr/share/second-edge".to_string(),
        hardlink(target, "rpm:9:42"),
        None,
        second,
    )
    .with_claim_policy(PayloadSharingPolicy::Rpm);
    second_edge.insert(&conn).unwrap();

    FileEntry::reconcile_hardlink_materialization(&conn, target).unwrap();

    let identity = format!("path:{target}");
    assert_eq!(
        FileEntry::find_by_path(&conn, target)
            .unwrap()
            .unwrap()
            .node
            .source
            .kind,
        PayloadNodeKind::Regular {
            hardlink_identity: Some(identity.clone())
        }
    );
    for path in ["/usr/share/first-edge", "/usr/share/second-edge"] {
        assert_eq!(
            FileEntry::find_by_path(&conn, path)
                .unwrap()
                .unwrap()
                .node
                .source
                .kind,
            PayloadNodeKind::Hardlink {
                target: target.to_string(),
                identity: identity.clone()
            }
        );
    }
    assert_eq!(
        PayloadClaim::find_by_path(&conn, target)
            .unwrap()
            .into_iter()
            .map(|claim| claim.node.source.kind)
            .collect::<Vec<_>>(),
        vec![
            PayloadNodeKind::Regular {
                hardlink_identity: Some("rpm:1:7".to_string())
            },
            PayloadNodeKind::Regular {
                hardlink_identity: Some("rpm:9:42".to_string())
            }
        ],
        "source-native claim identities remain independent of materialization"
    );
    for trove_id in [first, second] {
        PackagePayloadOwnership::load(&conn, trove_id).unwrap();
    }

    Trove::delete(&conn, first).unwrap();
    assert_eq!(
        PackagePayloadOwnership::load(&conn, second)
            .unwrap()
            .lifecycle_paths(),
        &[
            "/usr/share/hardlink-target".to_string(),
            "/usr/share/second-edge".to_string()
        ]
    );
}
