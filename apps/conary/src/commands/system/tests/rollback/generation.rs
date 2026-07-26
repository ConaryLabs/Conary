// apps/conary/src/commands/system/tests/rollback/generation.rs

use super::*;
use conary_core::activation::{SystemdActivationAction, SystemdActivationInvocation};
use conary_core::db::models::{
    ActivationRequest, ActivationRequestSourceKind, GenerationActivationIntent,
    GenerationPublication, GenerationPublicationStatus, NewActivationRequest,
};
use std::path::{Path, PathBuf};

#[tokio::test]
async fn rollback_commit_survives_publication_failure_and_retry_publishes_exact_root() {
    let (_temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(Path::new(&db_path_str), 1);
    let conn = conary_core::db::open(&db_path_str).unwrap();
    let original_generation = conary_core::generation::mount::current_generation(
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path_str))
            .root(),
    )
    .unwrap()
    .unwrap();
    let authority = RollbackSystemAuthority::capture(&conn).unwrap();
    let changeset_id = record_additive_changeset_with_system_authority(
        &conn,
        &db_path_str,
        authority,
        "publication-retry-addition",
    );
    let added_id = Trove::find_one_by_name(&conn, "publication-retry-addition")
        .unwrap()
        .unwrap()
        .id
        .unwrap();
    drop(conn);

    let failure = crate::commands::composefs_ops::test_forced_generation_rebuild_failure_guard(
        "forced rollback publication failure",
    );
    cmd_rollback(changeset_id, &db_path_str).await.unwrap();
    drop(failure);

    let conn = conary_core::db::open(&db_path_str).unwrap();
    assert!(Trove::find_by_id(&conn, added_id).unwrap().is_none());
    let (status, rollback_id): (String, Option<i64>) = conn
        .query_row(
            "SELECT status, reversed_by_changeset_id FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "rolled_back");
    let rollback_id = rollback_id.unwrap();
    let rollback_metadata: Option<String> = conn
        .query_row(
            "SELECT metadata FROM changesets WHERE id = ?1",
            [rollback_id],
            |row| row.get(0),
        )
        .unwrap();
    let follow_up = crate::commands::deferred_follow_up(rollback_metadata.as_deref()).unwrap();
    assert_eq!(follow_up.len(), 1);
    assert_eq!(follow_up[0].kind, "generation_publication");
    assert_eq!(follow_up[0].status, "pending");
    let debts = GenerationPublication::pending_recoverable(&conn).unwrap();
    assert_eq!(debts.len(), 1);
    assert_eq!(debts[0].trigger_changeset_id, Some(rollback_id));
    assert_eq!(debts[0].status, GenerationPublicationStatus::Failed);
    assert!(
        debts[0]
            .last_error
            .as_deref()
            .unwrap()
            .contains("forced rollback publication failure")
    );
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path_str));
    assert_eq!(
        conary_core::generation::mount::current_generation(runtime_root.root())
            .unwrap()
            .unwrap(),
        original_generation,
        "failed publication must not move the active generation"
    );

    let outcome = crate::commands::generation::publication::retry_pending_publication(
        &conn,
        &db_path_str,
        "retry exact rollback selected root",
    )
    .unwrap();
    assert!(!outcome.needs_publication);
    assert!(
        conary_core::generation::mount::current_generation(runtime_root.root())
            .unwrap()
            .unwrap()
            > original_generation
    );
    assert!(
        GenerationPublication::pending_recoverable(&conn)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn rollback_generation_excludes_forward_activation_request() {
    let (_temp_dir, db_path_str) = crate::commands::test_helpers::setup_command_test_db();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    crate::commands::test_helpers::create_active_test_generation(Path::new(&db_path_str), 1);
    let conn = conary_core::db::open(&db_path_str).unwrap();
    let forward_generation = conary_core::generation::mount::current_generation(
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path_str))
            .root(),
    )
    .unwrap()
    .unwrap();

    let authority = RollbackSystemAuthority::capture(&conn).unwrap();
    let mut changeset = Changeset::new("Install activation fixture".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    let request_id = ActivationRequest::append_batch(
        &conn,
        changeset_id,
        &[NewActivationRequest {
            source_kind: ActivationRequestSourceKind::CapturedSystemctl,
            source_package: "activation-fixture".to_string(),
            source_version: "1.0".to_string(),
            source_entry: "activation-fixture.service".to_string(),
            invocation: SystemdActivationInvocation {
                action: SystemdActivationAction::Restart,
                units: vec!["activation-fixture.service".to_string()],
                job_mode: None,
                all: false,
                no_block: false,
                no_ask_password: true,
                quiet: false,
                no_warn: false,
                wait: false,
                show_transaction: false,
            }
            .into(),
        }],
    )
    .unwrap()[0];
    conn.execute(
        "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
        params![
            metadata_with_removed_troves(
                Vec::new(),
                Vec::new(),
                active_rollback_root(&db_path_str),
                authority,
            )
            .unwrap(),
            changeset_id,
        ],
    )
    .unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();
    assert_eq!(
        GenerationActivationIntent::project_through(&conn, forward_generation, Some(changeset_id),)
            .unwrap(),
        1
    );
    insert_test_trove(&conn, changeset_id, "activation-fixture", "1.0", &[]);
    drop(conn);

    cmd_rollback(changeset_id, &db_path_str).await.unwrap();

    let conn = conary_core::db::open(&db_path_str).unwrap();
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path_str));
    let generation = conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap()
        .unwrap();
    let projected: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_activation_intents
             WHERE generation_number = ?1 AND request_id = ?2",
            params![generation, request_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        projected, 0,
        "a rolled-back forward request must not enter the compensating generation"
    );
    assert!(
        ActivationRequest::find_by_id(&conn, request_id)
            .unwrap()
            .is_some(),
        "the exact request remains as historical changeset causation"
    );
    assert!(
        GenerationActivationIntent::ready_for_generation(&conn, forward_generation)
            .unwrap()
            .is_empty(),
        "a historical forward intent from a rolled-back changeset is not executable"
    );
    let forward_status: String = conn
        .query_row(
            "SELECT status FROM generation_activation_intents
             WHERE generation_number = ?1 AND request_id = ?2",
            params![forward_generation, request_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        forward_status, "superseded",
        "the historical forward intent is retired explicitly"
    );
}
