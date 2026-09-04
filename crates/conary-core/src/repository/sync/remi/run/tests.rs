// crates/conary-core/src/repository/sync/remi/run/tests.rs

use super::*;
use crate::db::models::Repository;
use crate::db::schema::ensure_current;

const OWNER_ONE: &str = "00000000-0000-4000-8000-000000000001";
const OWNER_TWO: &str = "00000000-0000-4000-8000-000000000002";

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn test_repo(conn: &Connection, name: &str, profile: &str) -> Repository {
    let mut repo = Repository::new(name.to_string(), "https://remi.test".to_string());
    repo.source_profile = Some(profile.to_string());
    repo.id = Some(repo.insert(conn).unwrap());
    repo
}

fn member(repository_id: i64, ordinal: i64, digest: Option<&str>) -> RemiSyncRunMember {
    RemiSyncRunMember {
        ordinal,
        repository_id,
        source_identity: format!("source-{ordinal}"),
        repository_identity: format!("repository-{ordinal}"),
        stream_kind: "release".to_string(),
        stream_identity: "fixture".to_string(),
        role: ProfileSourceRole::Base,
        precedence: ordinal,
        required: true,
        input_source_snapshot_sha256: None,
        candidate_source_snapshot_sha256: digest.map(str::to_string),
    }
}

fn register_candidate_fixture(conn: &Connection, profile_digest: &str, source_digest: &str) {
    for (resource_digest, kind) in [
        (source_digest, "source_snapshot"),
        (profile_digest, "profile_revision"),
    ] {
        conn.execute(
            "INSERT INTO remi_catalog_resources (
                     resource_sha256, resource_kind, source_profile,
                     artifact_sha256, artifact_size, logical_digest_sha256,
                     manifest_json, portable_manifest_sha256,
                     portable_manifest_size, portable_chunk_size,
                     portable_chunk_count, durable, created_at
                 ) VALUES (?1, ?2, 'fedora-44', ?3, 1, ?4, '{}', ?5,
                           96, 65536, 1, 1, 1)",
            params![
                resource_digest,
                kind,
                resource_digest,
                digest('d'),
                digest('c')
            ],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO remi_profile_revision_members (
                 profile_revision_sha256, ordinal, source_snapshot_sha256,
                 source_identity, repository_identity, stream_kind,
                 stream_identity, role, precedence, required
             ) VALUES (?1, 0, ?2, 'source-0', 'repository-0', 'release',
                       'fixture', 'base', 0, 1)",
        params![profile_digest, source_digest],
    )
    .unwrap();
}

fn run_state(conn: &Connection, run: &RemiSyncRun) -> String {
    conn.query_row(
        "SELECT state FROM repository_sync_runs WHERE run_id = ?1",
        [&run.run_id],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn active_profile_lease_rejects_a_concurrent_run() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let first = begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, 100).unwrap();
    let error = begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_TWO, 101).unwrap_err();
    assert!(error.to_string().contains("owns fencing epoch 1"));
    assert_eq!(run_state(&conn, &first), "created");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM repositories", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn expired_lease_recovers_only_its_exact_profile_run() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("restart-recovery.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open_fast(&db_path).unwrap();
    let repo = test_repo(&conn, "unrelated", "ubuntu-26.04");
    let first = begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, 100).unwrap();
    drop(conn);

    let conn = crate::db::open_fast(&db_path).unwrap();
    let second = begin_profile_sync_run_at(
        &conn,
        "fedora-44",
        None,
        OWNER_TWO,
        100 + REMI_SYNC_LEASE_SECONDS,
    )
    .unwrap();
    assert_eq!(second.fencing_epoch, 2);
    assert_eq!(second.recovery_run_ids, vec![first.run_id.clone()]);
    assert_eq!(run_state(&conn, &first), "abandoned");
    assert!(
        Repository::find_by_id(&conn, repo.id.unwrap())
            .unwrap()
            .is_some()
    );
    let error = heartbeat_profile_sync_run(&conn, &first).unwrap_err();
    assert!(error.to_string().contains("lost fencing epoch 1"));
}

#[test]
fn restart_recovery_fences_only_expired_runs_and_replays_cleanup_until_acknowledged() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let expired = begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, 100).unwrap();
    let live = begin_profile_sync_run_at(&conn, "ubuntu-26.04", None, OWNER_TWO, 500).unwrap();
    let recovery_time = 100 + REMI_SYNC_LEASE_SECONDS;

    let first = recover_expired_profile_sync_runs_at(&conn, recovery_time).unwrap();
    assert_eq!(
        first,
        vec![ProfileSyncRunRecovery {
            run_id: expired.run_id.clone(),
            source_profile: expired.source_profile.clone(),
        }]
    );
    assert_eq!(run_state(&conn, &expired), "abandoned");
    assert_eq!(run_state(&conn, &live), "created");

    assert_eq!(
        recover_expired_profile_sync_runs_at(&conn, recovery_time).unwrap(),
        first
    );
    assert!(acknowledge_profile_sync_candidate_cleanup(&conn, &expired.run_id).unwrap());
    assert!(!acknowledge_profile_sync_candidate_cleanup(&conn, &expired.run_id).unwrap());
    assert!(
        recover_expired_profile_sync_runs_at(&conn, recovery_time)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn member_binding_is_exact_and_ready_requires_required_candidates() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let repo = test_repo(&conn, "source", "fedora-44");
    let run =
        begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, unix_seconds().unwrap())
            .unwrap();
    record_profile_sync_run_member(&conn, &run, &member(repo.id.unwrap(), 0, None)).unwrap();
    let error = ready_profile_sync_run(&conn, &run, &digest('a')).unwrap_err();
    assert!(error.to_string().contains("required members"));

    let candidate_digest = digest('b');
    record_profile_sync_run_member(
        &conn,
        &run,
        &member(repo.id.unwrap(), 0, Some(&candidate_digest)),
    )
    .unwrap();
    let profile_digest = digest('c');
    ready_profile_sync_run(&conn, &run, &profile_digest).unwrap();
    let (state, candidate): (String, String) = conn
        .query_row(
            "SELECT state, candidate_profile_digest
                 FROM repository_sync_runs WHERE run_id = ?1",
            [&run.run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "ready_to_publish");
    assert_eq!(candidate, profile_digest);
}

#[test]
fn completed_candidate_is_terminal_restart_safe_and_exactly_superseded() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let repo = test_repo(&conn, "source", "fedora-44");
    let now = unix_seconds().unwrap();
    let first = begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, now).unwrap();
    let source_digest = digest('a');
    record_profile_sync_run_member(
        &conn,
        &first,
        &member(repo.id.unwrap(), 0, Some(&source_digest)),
    )
    .unwrap();
    let profile_digest = digest('b');
    ready_profile_sync_run(&conn, &first, &profile_digest).unwrap();
    let error = complete_profile_sync_candidate(&conn, &first).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("lacks durable registered metadata")
    );
    assert_eq!(run_state(&conn, &first), "ready_to_publish");
    register_candidate_fixture(&conn, &profile_digest, &source_digest);
    conn.execute(
        "UPDATE repository_sync_run_members SET precedence = 1
             WHERE run_id = ?1 AND ordinal = 0",
        [&first.run_id],
    )
    .unwrap();
    let error = complete_profile_sync_candidate(&conn, &first).unwrap_err();
    assert!(error.to_string().contains("exact ordered member set"));
    assert_eq!(run_state(&conn, &first), "ready_to_publish");
    conn.execute(
        "UPDATE repository_sync_run_members SET precedence = 0
             WHERE run_id = ?1 AND ordinal = 0",
        [&first.run_id],
    )
    .unwrap();

    let completed = complete_profile_sync_candidate(&conn, &first).unwrap();
    assert_eq!(completed.source_profile, "fedora-44");
    assert_eq!(completed.profile_revision_sha256, profile_digest);
    assert_eq!(completed.run_id, first.run_id);
    assert_eq!(completed.owner_instance_uuid, OWNER_ONE);
    assert_eq!(completed.fencing_epoch, 1);
    assert_eq!(run_state(&conn, &first), "candidate");
    for table in [
        "remi_active_profile_revisions",
        "remi_active_universe_revision",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0,
            "candidate completion changed public authority in {table}"
        );
    }
    assert_eq!(
        current_profile_sync_candidate(&conn, "fedora-44").unwrap(),
        Some(completed.clone())
    );

    let recovery = recover_expired_profile_sync_runs_at(
        &conn,
        completed.completed_at + REMI_SYNC_LEASE_SECONDS,
    )
    .unwrap();
    assert_eq!(run_state(&conn, &first), "candidate");
    assert_eq!(recovery[0].run_id, first.run_id);

    let second = begin_profile_sync_run(&conn, "fedora-44", OWNER_TWO).unwrap();
    assert_eq!(second.fencing_epoch, 2);
    assert_eq!(
        current_profile_sync_candidate(&conn, "fedora-44").unwrap(),
        Some(completed.clone())
    );
    assert_eq!(run_state(&conn, &first), "candidate");

    abort_profile_sync_run(
        &conn,
        &second,
        ProfileSyncFailureStage::FetchingObjects,
        ProfileSyncFailureCategory::Transport,
        "fixture body interruption",
    )
    .unwrap();
    assert_eq!(
        current_profile_sync_candidate(&conn, "fedora-44").unwrap(),
        Some(completed)
    );

    let third = begin_profile_sync_run(&conn, "fedora-44", OWNER_ONE).unwrap();
    let next_source_digest = digest('d');
    record_profile_sync_run_member(
        &conn,
        &third,
        &member(repo.id.unwrap(), 0, Some(&next_source_digest)),
    )
    .unwrap();
    let next_profile_digest = digest('e');
    ready_profile_sync_run(&conn, &third, &next_profile_digest).unwrap();
    register_candidate_fixture(&conn, &next_profile_digest, &next_source_digest);
    let next = complete_profile_sync_candidate(&conn, &third).unwrap();
    assert_eq!(
        current_profile_sync_candidate(&conn, "fedora-44").unwrap(),
        Some(next.clone())
    );

    conn.execute(
        "UPDATE repository_sync_runs SET state = 'published' WHERE run_id = ?1",
        [&third.run_id],
    )
    .unwrap();
    assert!(
        current_profile_sync_candidate(&conn, "fedora-44")
            .unwrap()
            .is_none()
    );
}

#[test]
fn historical_run_member_does_not_block_repository_replacement() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let repo = test_repo(&conn, "source", "fedora-44");
    let run =
        begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, unix_seconds().unwrap())
            .unwrap();
    record_profile_sync_run_member(
        &conn,
        &run,
        &member(repo.id.unwrap(), 0, Some(&digest('a'))),
    )
    .unwrap();

    Repository::delete(&conn, repo.id.unwrap()).unwrap();

    assert!(
        Repository::find_by_id(&conn, repo.id.unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        conn.query_row(
            "SELECT repository_id FROM repository_sync_run_members WHERE run_id = ?1",
            [&run.run_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        repo.id.unwrap()
    );
}

#[test]
fn heartbeat_does_not_fill_or_rewrite_candidate_digest() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let repo = test_repo(&conn, "source", "fedora-44");
    let run =
        begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, unix_seconds().unwrap())
            .unwrap();
    heartbeat_profile_sync_run(&conn, &run).unwrap();
    let candidate = digest('a');
    record_profile_sync_run_member(&conn, &run, &member(repo.id.unwrap(), 0, Some(&candidate)))
        .unwrap();
    ready_profile_sync_run(&conn, &run, &candidate).unwrap();
    let now = unix_seconds().unwrap();
    let prior_expiry = now + 60;
    conn.execute(
        "UPDATE repository_sync_runs
             SET heartbeat_at = ?1, lease_expires_at = ?2
             WHERE run_id = ?3",
        params![now, prior_expiry, &run.run_id],
    )
    .unwrap();

    heartbeat_profile_sync_run(&conn, &run).unwrap();
    let (state, stored_candidate, heartbeat_at, lease_expires_at) = conn
        .query_row(
            "SELECT state, candidate_profile_digest, heartbeat_at, lease_expires_at
                 FROM repository_sync_runs WHERE run_id = ?1",
            [&run.run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(state, "ready_to_publish");
    assert_eq!(stored_candidate.as_deref(), Some(candidate.as_str()));
    assert!(heartbeat_at >= now);
    assert!(lease_expires_at > prior_expiry);
}

#[test]
fn coordinator_heartbeat_cadence_precedes_lease_expiry() {
    assert!(
        PROFILE_SYNC_HEARTBEAT_INTERVAL.as_secs() < u64::try_from(REMI_SYNC_LEASE_SECONDS).unwrap()
    );
}

#[test]
fn abort_marks_only_the_exact_owned_run_abandoned() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let run =
        begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, unix_seconds().unwrap())
            .unwrap();
    abort_profile_sync_run(
        &conn,
        &run,
        RemiSyncFailureStage::Ingesting,
        RemiSyncFailureCategory::Internal,
        "fixture failure",
    )
    .unwrap();
    assert_eq!(run_state(&conn, &run), "abandoned");
    assert!(
        abort_profile_sync_run(
            &conn,
            &run,
            RemiSyncFailureStage::Ingesting,
            RemiSyncFailureCategory::Internal,
            "replay",
        )
        .is_ok()
    );
    let forged = ProfileSyncRun {
        owner_instance_uuid: OWNER_TWO.to_string(),
        ..run
    };
    assert!(
        abort_profile_sync_run(
            &conn,
            &forged,
            RemiSyncFailureStage::Ingesting,
            RemiSyncFailureCategory::Internal,
            "wrong owner",
        )
        .is_err()
    );
}
