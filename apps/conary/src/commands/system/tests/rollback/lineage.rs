// apps/conary/src/commands/system/tests/rollback/lineage.rs

use super::*;
use conary_core::db::models::{ChangesetKind, GenerationPublication};
use std::collections::BTreeSet;

#[tokio::test]
async fn sequential_rollback_ignores_applied_compensation_rows() {
    let (_temp_dir, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(Path::new(&db_path), 1);
    let conn = conary_core::db::open(&db_path).unwrap();
    let first = record_additive_changeset_with_system_authority(
        &conn,
        &db_path,
        RollbackSystemAuthority::default(),
        "first-lineage-mutation",
    );
    let second = record_additive_changeset_with_system_authority(
        &conn,
        &db_path,
        RollbackSystemAuthority::default(),
        "second-lineage-mutation",
    );
    drop(conn);

    cmd_rollback(second, &db_path).await.unwrap();
    cmd_rollback(first, &db_path).await.unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    for changeset_id in [first, second] {
        let changeset = Changeset::find_by_id(&conn, changeset_id).unwrap().unwrap();
        assert_eq!(changeset.kind, ChangesetKind::Mutation);
        assert_eq!(changeset.status, ChangesetStatus::RolledBack);
        assert!(changeset.reversed_by_changeset_id.is_some());
    }
    let rollback_rows = Changeset::list_all(&conn)
        .unwrap()
        .into_iter()
        .filter(|changeset| changeset.kind == ChangesetKind::Rollback)
        .collect::<Vec<_>>();
    assert_eq!(rollback_rows.len(), 2);
    assert_eq!(
        rollback_rows
            .iter()
            .map(|changeset| changeset.reverts_changeset_id.unwrap())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([first, second])
    );
    assert!(
        rollback_rows
            .iter()
            .all(|changeset| changeset.status == ChangesetStatus::Applied)
    );
}

#[tokio::test]
async fn precommit_failure_leaves_forward_mutation_retryable() {
    let (_temp_dir, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(Path::new(&db_path), 1);
    let conn = conary_core::db::open(&db_path).unwrap();
    let forward = record_additive_changeset_with_system_authority(
        &conn,
        &db_path,
        RollbackSystemAuthority::default(),
        "retryable-lineage-mutation",
    );
    drop(conn);

    let error = cmd_rollback_with_forced_precommit_failure(forward, &db_path)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("forced rollback failure after publication candidate persistence"),
        "{error}"
    );

    let conn = conary_core::db::open(&db_path).unwrap();
    let target = Changeset::find_by_id(&conn, forward).unwrap().unwrap();
    assert_eq!(target.kind, ChangesetKind::Mutation);
    assert_eq!(target.status, ChangesetStatus::Applied);
    assert_eq!(target.reversed_by_changeset_id, None);
    assert!(
        Changeset::list_all(&conn)
            .unwrap()
            .into_iter()
            .all(|changeset| changeset.kind != ChangesetKind::Rollback)
    );
    assert!(
        Trove::list_all(&conn)
            .unwrap()
            .iter()
            .any(|trove| trove.installed_by_changeset_id == Some(forward))
    );
    assert!(
        GenerationPublication::pending_recoverable(&conn)
            .unwrap()
            .is_empty()
    );
    drop(conn);

    cmd_rollback(forward, &db_path).await.unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(
        Changeset::find_by_id(&conn, forward)
            .unwrap()
            .unwrap()
            .status,
        ChangesetStatus::RolledBack
    );
}

#[tokio::test]
async fn crash_before_sqlite_commit_replaces_reused_candidate_id() {
    let (_temp_dir, db_path) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(Path::new(&db_path), 1);
    let conn = conary_core::db::open(&db_path).unwrap();
    let forward = record_additive_changeset_with_system_authority(
        &conn,
        &db_path,
        RollbackSystemAuthority::default(),
        "crash-retry-lineage-mutation",
    );
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path));
    let mut engine = conary_core::transaction::TransactionEngine::new(
        conary_core::transaction::TransactionConfig::for_runtime_root(&runtime_root),
    )
    .unwrap();
    engine.begin().unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let mut abandoned_rollback =
        Changeset::new_rollback("interrupted compensation".to_string(), forward);
    let abandoned_rollback_id = abandoned_rollback.insert(&tx).unwrap();
    let orphan_debt = crate::commands::generation::publication::record_selected_root_state(
        &tx,
        &crate::commands::generation::publication::PublicationRequest {
            db_path: &db_path,
            summary: "interrupted compensation",
            trigger_changeset_id: Some(abandoned_rollback_id),
            tx_uuid: None,
            config_transaction:
                conary_core::config_transaction::GenerationConfigTransaction::default(),
        },
    )
    .unwrap();
    let orphan_candidate = crate::commands::generation::selected_root::publication_candidate_dir(
        &runtime_root,
        &orphan_debt,
    )
    .unwrap();
    crate::commands::generation::selected_root::persist_captured_publication_candidate(
        &runtime_root,
        &orphan_debt,
        &active_rollback_root(&db_path),
    )
    .unwrap();
    assert!(orphan_candidate.is_dir());

    // A process death here rolls SQLite back but cannot remove the already
    // durable candidate directory.
    drop(tx);
    drop(engine);
    drop(conn);

    cmd_rollback(forward, &db_path)
        .await
        .expect("retry must replace the candidate left at the reused debt ID");
    let conn = conary_core::db::open(&db_path).unwrap();
    let target = Changeset::find_by_id(&conn, forward).unwrap().unwrap();
    assert_eq!(target.status, ChangesetStatus::RolledBack);
    let rollback = Changeset::list_all(&conn)
        .unwrap()
        .into_iter()
        .find(|changeset| changeset.kind == ChangesetKind::Rollback)
        .unwrap();
    assert_eq!(rollback.reverts_changeset_id, Some(forward));
}
