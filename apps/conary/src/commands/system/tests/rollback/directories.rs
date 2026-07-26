// apps/conary/src/commands/system/tests/rollback/directories.rs

use super::*;
use conary_core::db::models::{
    DirectoryClaim, DirectoryClaimAnchorPolicy, ExistingDirectoryMaterialization,
};
use conary_core::generation::root_manifest::GenerationRootEntry;

type RawMaterializedAuthority = (
    String,
    i64,
    Option<i64>,
    String,
    Vec<(i64, Option<i64>, String, String, String)>,
);

fn raw_materialized_authority(conn: &rusqlite::Connection, path: &str) -> RawMaterializedAuthority {
    let (node, trove_id, component_id, installed_at) = conn
        .query_row(
            "SELECT payload_node_json, trove_id, component_id, installed_at
             FROM files WHERE path = ?1",
            [path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let claims = conn
        .prepare(
            "SELECT trove_id, component_id, anchor_policy, payload_node_json, claimed_at
             FROM directory_claims WHERE path = ?1 ORDER BY trove_id",
        )
        .unwrap()
        .query_map([path], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    (node, trove_id, component_id, installed_at, claims)
}

fn selected_root_with_materialization(
    db_path: &str,
    path: &str,
    node: ResolvedPayloadNode,
) -> CapturedSelectedRoot {
    let mut root = active_rollback_root(db_path);
    if matches!(node.source.kind, PayloadNodeKind::Symlink { .. }) {
        for directory in ["/usr", "/usr/lib"] {
            root.generation.entries.push(GenerationRootEntry {
                path: directory.to_string(),
                node: resolved_node(PayloadNodeKind::Directory, libc::S_IFDIR | 0o755),
                content: None,
            });
        }
    }
    root.generation.entries.push(GenerationRootEntry {
        path: path.to_string(),
        node,
        content: None,
    });
    root.generation
        .entries
        .sort_by(|left, right| left.path.cmp(&right.path));
    root.generation.validate().unwrap();
    root
}

async fn assert_additive_shared_directory_rollback(
    fixture: &str,
    symlink_anchor: bool,
    apply_through_symlink: bool,
    additions: &[(&str, u32)],
) {
    let (_temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(Path::new(&db_path_str), 1);
    let conn = conary_core::db::open(&db_path_str).unwrap();

    let path = if symlink_anchor { "/lib" } else { "/shared" };
    let materialized_node = if symlink_anchor {
        resolved_node(
            PayloadNodeKind::Symlink {
                target: "usr/lib".to_string(),
            },
            libc::S_IFLNK | 0o777,
        )
    } else {
        resolved_node(PayloadNodeKind::Directory, libc::S_IFDIR | 0o711)
    };
    let claim_node = resolved_node(PayloadNodeKind::Directory, libc::S_IFDIR | 0o755);
    let anchor_policy = if symlink_anchor {
        DirectoryClaimAnchorPolicy::DirectoryOrSymlinkToDirectory
    } else {
        DirectoryClaimAnchorPolicy::Directory
    };
    let materialization = if symlink_anchor {
        ExistingDirectoryMaterialization::PreserveExistingDirectoryOrSymlinkToDirectory
    } else {
        ExistingDirectoryMaterialization::ApplyIncoming
    };

    let mut anchor = Trove::new(
        format!("{fixture}-anchor"),
        "1.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let anchor_id = anchor.insert(&conn).unwrap();
    let mut anchor_file =
        FileEntry::new(path.to_string(), materialized_node.clone(), None, anchor_id);
    anchor_file.insert(&conn).unwrap();
    let claim_owner_id = if symlink_anchor {
        let mut claim_owner = Trove::new(
            format!("{fixture}-claim-owner"),
            "1.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        claim_owner.insert(&conn).unwrap()
    } else {
        anchor_id
    };
    DirectoryClaim::new(path.to_string(), claim_owner_id, claim_node.clone())
        .unwrap()
        .with_anchor_policy(anchor_policy)
        .insert(&conn)
        .unwrap();
    if symlink_anchor {
        assert!(
            DirectoryClaim::find_by_path(&conn, path)
                .unwrap()
                .iter()
                .all(|claim| claim.trove_id != anchor_id),
            "a peer-owned verified symlink anchor does not need its own directory claim"
        );
    }

    let rollback_root =
        selected_root_with_materialization(&db_path_str, path, materialized_node.clone());
    let built_prestate = crate::commands::composefs_ops::build_captured_generation_for_publication(
        &conn,
        &db_path_str,
        "shared-directory prestate",
        &[],
        rollback_root.clone(),
    )
    .unwrap();
    crate::commands::composefs_ops::publish_generation_link(
        &db_path_str,
        built_prestate.generation_number,
    )
    .unwrap();
    crate::commands::composefs_ops::mark_generation_state_active(
        &conn,
        built_prestate.generation_number,
    )
    .unwrap();
    let rollback_root = active_rollback_root(&db_path_str);
    let exact_file = FileEntry::find_by_path(&conn, path).unwrap().unwrap();
    let before = raw_materialized_authority(&conn, path);
    let rollback_materialized =
        crate::commands::capture_materialized_directory_snapshots(&conn, [exact_file]).unwrap();
    assert_eq!(rollback_materialized.len(), 1);
    let rollback_system = RollbackSystemAuthority::capture(&conn).unwrap();

    let mut changeset = Changeset::new(format!("Install {fixture} additions"));
    let changeset_id = changeset.insert(&conn).unwrap();
    let metadata = metadata_with_removed_troves(
        Vec::new(),
        rollback_materialized,
        rollback_root.clone(),
        rollback_system,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&metadata).unwrap()
            ["rollback_materialized_directories"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "batch capture must deduplicate one pre-mutation materialization"
    );
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![metadata, changeset_id],
    )
    .unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();

    let mut added_ids = Vec::new();
    for (name, mode) in additions {
        let trove_id = insert_test_trove(&conn, changeset_id, name, "1.0", &[]);
        added_ids.push(trove_id);
        let mut incoming = FileEntry::new(
            path.to_string(),
            resolved_node(PayloadNodeKind::Directory, libc::S_IFDIR | *mode),
            None,
            trove_id,
        );
        incoming.insert_or_replace(&conn, materialization).unwrap();
    }
    assert_eq!(
        DirectoryClaim::find_by_path(&conn, path).unwrap().len(),
        additions.len() + 1
    );
    if !symlink_anchor || apply_through_symlink {
        let mut forward_root = rollback_root.clone();
        let forward_path = if apply_through_symlink {
            "/usr/lib"
        } else {
            path
        };
        let forward_mode = additions.last().unwrap().1;
        forward_root
            .generation
            .entries
            .iter_mut()
            .find(|entry| entry.path == forward_path)
            .unwrap()
            .node = resolved_node(PayloadNodeKind::Directory, libc::S_IFDIR | forward_mode);
        forward_root.generation.validate().unwrap();
        let built_forward =
            crate::commands::composefs_ops::build_captured_generation_for_publication(
                &conn,
                &db_path_str,
                "shared-directory forward state",
                &[],
                forward_root,
            )
            .unwrap();
        crate::commands::composefs_ops::publish_generation_link(
            &db_path_str,
            built_forward.generation_number,
        )
        .unwrap();
        crate::commands::composefs_ops::mark_generation_state_active(
            &conn,
            built_forward.generation_number,
        )
        .unwrap();
    }
    drop(conn);

    cmd_rollback(changeset_id, &db_path_str).await.unwrap();

    let conn = conary_core::db::open(&db_path_str).unwrap();
    for trove_id in added_ids {
        assert!(Trove::find_by_id(&conn, trove_id).unwrap().is_none());
    }
    assert_eq!(
        raw_materialized_authority(&conn, path),
        before,
        "rollback must reproduce raw materialized-node and claim authority"
    );
    if symlink_anchor {
        assert!(matches!(
            FileEntry::find_by_path(&conn, path)
                .unwrap()
                .unwrap()
                .node
                .source
                .kind,
            PayloadNodeKind::Symlink { .. }
        ));
    }
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path_str));
    let generation = conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap()
        .unwrap();
    let artifact = conary_core::generation::artifact::load_generation_artifact(
        &runtime_root.generation_path(generation),
    )
    .unwrap();
    assert_eq!(artifact.generation_root, rollback_root.generation);
    assert_eq!(artifact.mutable_state, rollback_root.state);
}

#[tokio::test]
async fn additive_install_shared_directory_rollback_is_exact() {
    assert_additive_shared_directory_rollback(
        "single-shared-directory",
        false,
        false,
        &[("single-shared-directory-added", 0o700)],
    )
    .await;
}

#[tokio::test]
async fn additive_batch_shared_directory_capture_is_unioned_once_and_rollback_is_exact() {
    assert_additive_shared_directory_rollback(
        "batch-shared-directory",
        false,
        false,
        &[
            ("batch-shared-directory-b", 0o700),
            ("batch-shared-directory-c", 0o775),
        ],
    )
    .await;
}

#[tokio::test]
async fn additive_deb_directory_claim_rollback_preserves_verified_symlink_materialization() {
    assert_additive_shared_directory_rollback(
        "deb-symlink-directory",
        true,
        false,
        &[("deb-symlink-directory-added", 0o755)],
    )
    .await;
}

#[tokio::test]
async fn additive_rpm_apply_through_symlink_rollback_restores_exact_target_metadata() {
    assert_additive_shared_directory_rollback(
        "rpm-symlink-directory",
        true,
        true,
        &[("rpm-symlink-directory-added", 0o700)],
    )
    .await;
}
