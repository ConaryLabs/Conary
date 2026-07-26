// apps/conary/src/commands/live_root/tests.rs

use super::*;
use conary_core::payload::{
    PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp, ResolvedPayloadNode,
};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use tempfile::TempDir;
use uuid::Uuid;

fn resolved_node(kind: PayloadNodeKind, mode: u32) -> ResolvedPayloadNode {
    ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind,
        mode,
        user: PayloadIdentity::Numeric { id: 0 },
        group: PayloadIdentity::Numeric { id: 0 },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: BTreeMap::new(),
    })
    .unwrap()
}

fn live_regular(path: &str, content: &[u8], mode: u32) -> LiveRootFile {
    LiveRootFile {
        path: path.to_string(),
        content: LiveRootContent::from_in_memory_bytes(content),
        node: resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            libc::S_IFREG | (mode & 0o7777),
        ),
    }
}

fn live_symlink(path: &str, target: &str, mode: u32) -> LiveRootFile {
    LiveRootFile {
        path: path.to_string(),
        content: LiveRootContent::absent(),
        node: resolved_node(
            PayloadNodeKind::Symlink {
                target: target.to_string(),
            },
            libc::S_IFLNK | (mode & 0o7777),
        ),
    }
}

#[test]
fn target_path_rejects_parent_dir_escape() {
    let root = TempDir::new().unwrap();
    let err = target_path(root.path(), "/usr/../escape")
        .unwrap_err()
        .to_string();

    assert!(err.contains("escapes the target root"));
}

#[test]
fn target_path_rejects_root_empty_and_current_dir_paths() {
    let root = TempDir::new().unwrap();

    for package_path in ["", "/", ".", "/."] {
        let err = target_path(root.path(), package_path)
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("must name a file or directory below the target root"),
            "{package_path:?} returned {err}"
        );
    }
}

#[test]
fn rename_and_sync_moves_file_and_leaves_target_parent_consistent() {
    let temp = tempfile::TempDir::new().unwrap();
    let source = temp.path().join("source");
    let target_dir = temp.path().join("target");
    std::fs::create_dir(&target_dir).unwrap();
    let target = target_dir.join("file");
    std::fs::write(&source, b"ok").unwrap();

    rename_and_sync(&source, &target).unwrap();

    assert!(!source.exists());
    assert_eq!(std::fs::read(&target).unwrap(), b"ok");
}

#[test]
fn remove_file_and_sync_removes_target() {
    let temp = tempfile::TempDir::new().unwrap();
    let target = temp.path().join("file");
    std::fs::write(&target, b"ok").unwrap();

    remove_file_and_sync(&target).unwrap();

    assert!(!target.exists());
}

#[test]
fn install_rejects_symlink_parent_without_writing_outside_root() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, root.join("usr")).unwrap();

    let mut tx = LiveRootTransaction::begin(
        &runtime,
        &root,
        Uuid::new_v4().to_string(),
        "install fixture",
    )
    .unwrap();
    let err = tx
        .apply_install_files(&[live_regular("/usr/bin/fixture", b"fixture", 0o755)])
        .unwrap_err()
        .to_string();

    assert!(err.contains("unsafe parent"));
    assert!(!outside.join("bin/fixture").exists());
}

#[test]
fn remove_rejects_symlink_parent_without_removing_outside_root() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(outside.join("bin")).unwrap();
    fs::write(outside.join("bin/fixture"), "outside").unwrap();
    symlink(&outside, root.join("usr")).unwrap();

    let mut tx = LiveRootTransaction::begin(
        &runtime,
        &root,
        Uuid::new_v4().to_string(),
        "remove fixture",
    )
    .unwrap();
    let err = tx
        .apply_remove_paths(&["/usr/bin/fixture".to_string()])
        .unwrap_err()
        .to_string();

    assert!(err.contains("unsafe parent"));
    assert_eq!(
        fs::read_to_string(outside.join("bin/fixture")).unwrap(),
        "outside"
    );
}

#[test]
fn begin_rejects_empty_or_path_like_transaction_ids() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&root).unwrap();

    for tx_uuid in ["", ".", "..", "../escape", "nested/id"] {
        let err = match LiveRootTransaction::begin(&runtime, &root, tx_uuid.to_string(), "install")
        {
            Ok(_) => panic!("accepted invalid transaction id {tx_uuid:?}"),
            Err(error) => error.to_string(),
        };

        assert!(err.contains("invalid live-root transaction id"));
    }
}

#[test]
fn install_writes_regular_file_and_symlink() {
    const TEST_NAME: &str = "commands::live_root::tests::install_writes_regular_file_and_symlink";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&root).unwrap();
    let mut tx = LiveRootTransaction::begin(
        &runtime,
        &root,
        Uuid::new_v4().to_string(),
        "install fixture",
    )
    .unwrap();

    let stats = tx
        .apply_install_files(&[
            live_regular("/usr/bin/fixture", b"fixture", 0o755),
            live_symlink("/usr/bin/fixture-link", "fixture", 0o777),
        ])
        .unwrap();
    tx.commit().unwrap();

    assert_eq!(stats.files_written, 2);
    assert_eq!(
        fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
        "fixture"
    );
    assert_eq!(
        fs::read_link(root.join("usr/bin/fixture-link")).unwrap(),
        PathBuf::from("fixture")
    );
    assert_eq!(
        fs::metadata(root.join("usr/bin/fixture"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}

#[test]
fn install_rejects_replacing_existing_directory_target() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("usr")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    let mut tx = LiveRootTransaction::begin(
        &runtime,
        &root,
        Uuid::new_v4().to_string(),
        "install fixture",
    )
    .unwrap();

    let err = tx
        .apply_install_files(&[live_regular("/usr", b"not a directory", 0o755)])
        .unwrap_err()
        .to_string();

    assert!(err.contains("refuses to replace existing directory"));
    assert!(root.join("usr").is_dir());
}

#[test]
fn rollback_restores_replaced_file() {
    const TEST_NAME: &str = "commands::live_root::tests::rollback_restores_replaced_file";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(root.join("usr/bin/fixture"), "old").unwrap();

    let mut tx = LiveRootTransaction::begin(
        &runtime,
        &root,
        Uuid::new_v4().to_string(),
        "install fixture",
    )
    .unwrap();
    tx.apply_install_files(&[live_regular("/usr/bin/fixture", b"new", 0o755)])
        .unwrap();
    tx.rollback().unwrap();

    assert_eq!(
        fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
        "old"
    );
}

#[test]
fn rollback_restores_original_file_after_multiple_graph_mutations() {
    const TEST_NAME: &str = "commands::live_root::tests::rollback_restores_original_file_after_multiple_graph_mutations";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(root.join("usr/bin/fixture"), "original").unwrap();

    let mut tx = LiveRootTransaction::begin(
        &runtime,
        &root,
        Uuid::new_v4().to_string(),
        "multi-package graph",
    )
    .unwrap();
    for content in [b"package-a".as_slice(), b"package-b".as_slice()] {
        tx.apply_install_files(&[live_regular("/usr/bin/fixture", content, 0o755)])
            .unwrap();
    }
    tx.rollback().unwrap();

    assert_eq!(
        fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
        "original"
    );
}

#[test]
fn remove_deletes_files_and_empty_dirs() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("usr/share/fixture")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(root.join("usr/share/fixture/readme"), "fixture").unwrap();

    let mut tx = LiveRootTransaction::begin(
        &runtime,
        &root,
        Uuid::new_v4().to_string(),
        "remove fixture",
    )
    .unwrap();
    let stats = tx
        .apply_remove_paths(&[
            "/usr/share/fixture/readme".to_string(),
            "/usr/share/fixture/".to_string(),
        ])
        .unwrap();
    tx.commit().unwrap();

    assert_eq!(stats.files_removed, 1);
    assert_eq!(stats.dirs_removed, 1);
    assert!(!root.join("usr/share/fixture").exists());
}

#[test]
fn rollback_restores_removed_empty_dirs() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("usr/share/fixture")).unwrap();
    fs::create_dir_all(&runtime).unwrap();

    let mut tx = LiveRootTransaction::begin(
        &runtime,
        &root,
        Uuid::new_v4().to_string(),
        "remove fixture",
    )
    .unwrap();
    tx.apply_remove_paths(&["/usr/share/fixture".to_string()])
        .unwrap();
    tx.rollback().unwrap();

    assert!(root.join("usr/share/fixture").is_dir());
}

#[test]
fn recovery_restores_in_progress_removed_file_from_persisted_journal() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(root.join("usr/bin/fixture"), "old").unwrap();

    let tx_uuid = Uuid::new_v4().to_string();
    let mut tx =
        LiveRootTransaction::begin(&runtime, &root, tx_uuid.clone(), "remove fixture").unwrap();
    tx.apply_remove_paths(&["/usr/bin/fixture".to_string()])
        .unwrap();
    std::mem::forget(tx);

    assert!(!root.join("usr/bin/fixture").exists());
    recover_pending_journals(&runtime, &root).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
        "old"
    );
    assert!(
        !runtime
            .join("live-root-journals")
            .join(format!("{tx_uuid}.json"))
            .exists()
    );
}

#[test]
fn recovery_does_not_rollback_commit_pending_journal() {
    const TEST_NAME: &str =
        "commands::live_root::tests::recovery_does_not_rollback_commit_pending_journal";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&root).unwrap();

    let tx_uuid = Uuid::new_v4().to_string();
    let mut tx =
        LiveRootTransaction::begin(&runtime, &root, tx_uuid.clone(), "install fixture").unwrap();
    tx.apply_install_files(&[live_regular("/usr/bin/fixture", b"fixture", 0o755)])
        .unwrap();
    tx.mark_committed_for_recovery().unwrap();
    std::mem::forget(tx);

    recover_pending_journals(&runtime, &root).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
        "fixture"
    );
    assert!(
        !runtime
            .join("live-root-journals")
            .join(format!("{tx_uuid}.json"))
            .exists()
    );
}

#[test]
fn recovery_rolls_back_in_progress_journal_without_changeset() {
    const TEST_NAME: &str =
        "commands::live_root::tests::recovery_rolls_back_in_progress_journal_without_changeset";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(root.join("usr/bin/fixture"), "old").unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    let tx_uuid = Uuid::new_v4().to_string();
    let mut tx =
        LiveRootTransaction::begin(&runtime, &root, tx_uuid.clone(), "install fixture").unwrap();
    tx.apply_install_files(&[live_regular("/usr/bin/fixture", b"new", 0o755)])
        .unwrap();
    std::mem::forget(tx);

    recover_pending_journals_with_changesets(&runtime, &root, &conn).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
        "old"
    );
    assert!(
        !runtime
            .join("live-root-journals")
            .join(format!("{tx_uuid}.json"))
            .exists()
    );
}

#[test]
fn recovery_does_not_rollback_applied_changeset_journal() {
    const TEST_NAME: &str =
        "commands::live_root::tests::recovery_does_not_rollback_applied_changeset_journal";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    use conary_core::db::models::{Changeset, ChangesetStatus};

    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(root.join("usr/bin/fixture"), "old").unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();

    let tx_uuid = Uuid::new_v4().to_string();
    let mut tx =
        LiveRootTransaction::begin(&runtime, &root, tx_uuid.clone(), "install fixture").unwrap();
    tx.apply_install_files(&[live_regular("/usr/bin/fixture", b"new", 0o755)])
        .unwrap();
    let mut changeset = Changeset::with_tx_uuid("Install fixture".to_string(), tx_uuid.clone());
    changeset.insert(&conn).unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    std::mem::forget(tx);

    recover_pending_journals_with_changesets(&runtime, &root, &conn).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
        "new"
    );
    assert!(
        !runtime
            .join("live-root-journals")
            .join(format!("{tx_uuid}.json"))
            .exists()
    );
    assert!(
        !runtime
            .join("live-root-journals")
            .join(format!("{tx_uuid}.backups"))
            .exists()
    );
}

#[test]
fn recovery_restores_removed_file_and_empty_parent_dir_from_persisted_journal() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("usr/share/pkg")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(root.join("usr/share/pkg/readme"), "fixture").unwrap();

    let tx_uuid = Uuid::new_v4().to_string();
    let mut tx =
        LiveRootTransaction::begin(&runtime, &root, tx_uuid.clone(), "remove fixture").unwrap();
    tx.apply_remove_paths(&[
        "/usr/share/pkg/readme".to_string(),
        "/usr/share/pkg".to_string(),
    ])
    .unwrap();
    std::mem::forget(tx);

    assert!(!root.join("usr/share/pkg").exists());
    recover_pending_journals(&runtime, &root).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("usr/share/pkg/readme")).unwrap(),
        "fixture"
    );
    assert!(
        !runtime
            .join("live-root-journals")
            .join(format!("{tx_uuid}.json"))
            .exists()
    );
}

#[test]
fn recovery_rejects_malformed_journal_transaction_id() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    let journal_dir = runtime.join("live-root-journals");
    fs::create_dir_all(&journal_dir).unwrap();
    fs::create_dir_all(&root).unwrap();
    let journal = LiveRootJournal {
        schema: JOURNAL_SCHEMA.to_string(),
        tx_uuid: "../escape".to_string(),
        operation: "remove fixture".to_string(),
        state: "pending".to_string(),
        backups: Vec::new(),
        created_paths: Vec::new(),
        removed_dirs: Vec::new(),
        modified_directories: Vec::new(),
    };
    fs::write(
        journal_dir.join("safe.json"),
        serde_json::to_vec_pretty(&journal).unwrap(),
    )
    .unwrap();

    let err = recover_pending_journals(&runtime, &root)
        .unwrap_err()
        .to_string();

    assert!(err.contains("invalid live-root transaction id"));
}

#[test]
fn recovery_rejects_journal_transaction_id_mismatched_with_filename() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    let journal_dir = runtime.join("live-root-journals");
    fs::create_dir_all(&journal_dir).unwrap();
    fs::create_dir_all(&root).unwrap();
    let filename_tx_uuid = Uuid::new_v4().to_string();
    let journal_tx_uuid = Uuid::new_v4().to_string();
    let journal = LiveRootJournal {
        schema: JOURNAL_SCHEMA.to_string(),
        tx_uuid: journal_tx_uuid,
        operation: "remove fixture".to_string(),
        state: "pending".to_string(),
        backups: Vec::new(),
        created_paths: Vec::new(),
        removed_dirs: Vec::new(),
        modified_directories: Vec::new(),
    };
    fs::write(
        journal_dir.join(format!("{filename_tx_uuid}.json")),
        serde_json::to_vec_pretty(&journal).unwrap(),
    )
    .unwrap();

    let err = recover_pending_journals(&runtime, &root)
        .unwrap_err()
        .to_string();

    assert!(err.contains("does not match journal filename"));
}

#[test]
fn recovery_rejects_backup_path_outside_transaction_backup_dir() {
    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    let outside = temp.path().join("outside-backup");
    let journal_dir = runtime.join("live-root-journals");
    fs::create_dir_all(&journal_dir).unwrap();
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::write(&outside, "outside").unwrap();
    let tx_uuid = Uuid::new_v4().to_string();
    let journal_path = journal_dir.join(format!("{tx_uuid}.json"));
    let journal = LiveRootJournal {
        schema: JOURNAL_SCHEMA.to_string(),
        tx_uuid: tx_uuid.clone(),
        operation: "remove fixture".to_string(),
        state: "in_progress".to_string(),
        backups: vec![BackupRecord {
            path: root.join("usr/bin/fixture").to_string_lossy().into_owned(),
            backup_path: outside.to_string_lossy().into_owned(),
        }],
        created_paths: Vec::new(),
        removed_dirs: Vec::new(),
        modified_directories: Vec::new(),
    };
    fs::write(&journal_path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();

    let err = recover_pending_journals(&runtime, &root)
        .unwrap_err()
        .to_string();

    assert!(err.contains("invalid live-root backup path"));
    assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
    assert!(!root.join("usr/bin/fixture").exists());
}

#[test]
fn commit_removes_backup_directory() {
    const TEST_NAME: &str = "commands::live_root::tests::commit_removes_backup_directory";
    if !crate::commands::test_helpers::run_exact_test_in_user_mount_namespace(TEST_NAME) {
        return;
    }

    let temp = TempDir::new().unwrap();
    let runtime = temp.path().join("runtime");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("usr/bin")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(root.join("usr/bin/fixture"), "old").unwrap();

    let tx_uuid = Uuid::new_v4().to_string();
    let mut tx =
        LiveRootTransaction::begin(&runtime, &root, tx_uuid.clone(), "install fixture").unwrap();
    tx.apply_install_files(&[live_regular("/usr/bin/fixture", b"new", 0o755)])
        .unwrap();
    tx.commit().unwrap();

    assert!(
        !runtime
            .join("live-root-journals")
            .join(format!("{tx_uuid}.backups"))
            .exists()
    );
}
