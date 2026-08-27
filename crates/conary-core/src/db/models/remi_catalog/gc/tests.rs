// crates/conary-core/src/db/models/remi_catalog/gc/tests.rs

use super::*;
use crate::db::models::{
    RemiActiveProfileRevision, RemiProfileRevisionMember, RemiProfileRevisionPin,
    RemiRevisionPinKind,
};
use crate::db::schema::ensure_current;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";

fn digest(byte: char) -> String {
    crate::hash::sha256(byte.to_string().as_bytes())
}

fn resource_digest(byte: char) -> String {
    crate::hash::sha256(format!("{{\"resource\":\"{byte}\"}}").as_bytes())
}

fn resource(byte: char, kind: RemiCatalogResourceKind) -> RemiCatalogResource {
    RemiCatalogResource {
        resource_sha256: resource_digest(byte),
        kind,
        source_profile: "fedora-44".to_string(),
        artifact_sha256: digest(byte),
        artifact_size: 4096,
        logical_digest_sha256: digest('c'),
        manifest_json: format!("{{\"resource\":\"{byte}\"}}"),
        durable: true,
        created_at: 100,
    }
}

fn member(profile: char, source: char, ordinal: i64) -> RemiProfileRevisionMember {
    RemiProfileRevisionMember {
        profile_revision_sha256: resource_digest(profile),
        ordinal,
        source_snapshot_sha256: resource_digest(source),
        source_identity: format!("fixture-source-{ordinal}"),
        repository_identity: format!("fixture-repository-{ordinal}"),
        stream_kind: "release".to_string(),
        stream_identity: "fixture".to_string(),
        role: crate::repository::supported_profiles::ProfileSourceRole::Base,
        precedence: ordinal,
        required: true,
    }
}

fn setup() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    conn
}

fn install_profile(conn: &Connection, profile: char, sources: &[char]) {
    for source in sources {
        if RemiCatalogResource::find_by_sha256(conn, &resource_digest(*source))
            .unwrap()
            .is_none()
        {
            resource(*source, RemiCatalogResourceKind::SourceSnapshot)
                .insert(conn)
                .unwrap();
        }
    }
    resource(profile, RemiCatalogResourceKind::ProfileRevision)
        .insert(conn)
        .unwrap();
    for (ordinal, source) in sources.iter().enumerate() {
        member(profile, *source, ordinal as i64)
            .insert(conn)
            .unwrap();
    }
}

fn insert_run(
    conn: &Connection,
    run_id: &str,
    input_profile: Option<char>,
    candidate_profile: Option<char>,
    finished_at: Option<i64>,
) {
    let fencing_epoch = conn
        .query_row(
            "SELECT COALESCE(MAX(fencing_epoch), 0) + 1
                 FROM repository_sync_runs WHERE source_profile = 'fedora-44'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let state = if finished_at.is_some() {
        "published"
    } else {
        "ready_to_publish"
    };
    conn.execute(
        "INSERT INTO repository_sync_runs (
                 run_id, source_profile, owner_instance_uuid, fencing_epoch,
                 input_profile_digest, candidate_profile_digest, state,
                 started_at, heartbeat_at, lease_expires_at, finished_at
             ) VALUES (?1, 'fedora-44', ?2, ?3, ?4, ?5, ?6, 100, 100, 1000, ?7)",
        params![
            run_id,
            OWNER,
            fencing_epoch,
            input_profile.map(resource_digest),
            candidate_profile.map(resource_digest),
            state,
            finished_at,
        ],
    )
    .unwrap();
}

fn insert_run_member(
    conn: &Connection,
    run_id: &str,
    ordinal: i64,
    input_source: Option<char>,
    candidate_source: Option<char>,
) {
    let repository_id = conn
        .query_row(
            "INSERT INTO repositories(name, url, source_profile)
                 VALUES (?1, 'https://fixture.test', 'fedora-44')
                 RETURNING id",
            [format!("repository-{run_id}-{ordinal}")],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO repository_sync_run_members (
                 run_id, ordinal, repository_id, source_identity,
                 repository_identity, stream_kind, stream_identity, role,
                 precedence, required, input_source_snapshot_sha256,
                 candidate_source_snapshot_sha256
             ) VALUES (?1, ?2, ?3, 'fixture-source', ?4, 'release', 'fixture',
                       'base', 0, 1, ?5, ?6)",
        params![
            run_id,
            ordinal,
            repository_id,
            format!("fixture-repository-{run_id}-{ordinal}"),
            input_source.map(resource_digest),
            candidate_source.map(resource_digest),
        ],
    )
    .unwrap();
}

fn select_current_candidate(conn: &Connection, run_id: &str) {
    conn.execute(
        "UPDATE repository_sync_runs
         SET state = 'candidate'
         WHERE run_id = ?1 AND state = 'published'",
        [run_id],
    )
    .unwrap();
    select_scope_run(conn, run_id);
}

fn select_scope_run(conn: &Connection, run_id: &str) {
    conn.execute(
        "INSERT INTO repository_sync_scopes (
             source_profile, fencing_epoch, current_run_id
         ) SELECT source_profile, fencing_epoch, run_id
           FROM repository_sync_runs WHERE run_id = ?1
         ON CONFLICT(source_profile) DO UPDATE SET
             fencing_epoch = excluded.fencing_epoch,
             current_run_id = excluded.current_run_id",
        [run_id],
    )
    .unwrap();
}

fn activate(conn: &Connection, profile: char, run_id: &str) {
    conn.execute(
        "INSERT INTO remi_active_profile_revisions (
                 source_profile, profile_revision_sha256, fencing_epoch,
                 activation_run_id, owner_instance_uuid, activated_at
             ) VALUES ('fedora-44', ?1, 1, ?2, ?3, 100)",
        params![resource_digest(profile), run_id, OWNER],
    )
    .unwrap();
}

#[test]
fn active_profile_retains_transitive_source_and_orphans_are_planned() {
    let conn = setup();
    install_profile(&conn, 'a', &['s']);
    install_profile(&conn, 'b', &['o']);
    insert_run(
        &conn,
        "10000000-0000-4000-8000-000000000001",
        None,
        Some('a'),
        Some(100),
    );
    activate(&conn, 'a', "10000000-0000-4000-8000-000000000001");

    let plan = plan_catalog_collection(&conn).unwrap();
    assert!(
        plan.reachability
            .contains_profile_revision(&resource_digest('a'))
    );
    assert!(
        plan.reachability
            .contains_source_snapshot(&resource_digest('s'))
    );
    assert!(
        plan.unreachable_profile_resources
            .iter()
            .any(|resource| resource.resource_sha256 == resource_digest('b'))
    );
    assert!(
        plan.unreachable_source_resources
            .iter()
            .any(|resource| resource.resource_sha256 == resource_digest('o'))
    );
}

#[test]
fn every_pin_kind_retains_its_exact_profile_revision() {
    let conn = setup();
    install_profile(&conn, 'p', &['s']);
    RemiProfileRevisionPin {
        pin_id: "pin-work".to_string(),
        source_profile: "fedora-44".to_string(),
        profile_revision_sha256: resource_digest('p'),
        owner_kind: RemiRevisionPinKind::Work,
        owner_identity: "work-owner".to_string(),
        runtime_session_id: None,
        pinned_at: 100,
    }
    .insert(&conn)
    .unwrap();

    let plan = plan_catalog_collection(&conn).unwrap();
    assert!(
        plan.reachability
            .contains_profile_revision(&resource_digest('p'))
    );
    assert!(
        plan.reachability
            .contains_source_snapshot(&resource_digest('s'))
    );
}

#[test]
fn live_run_profile_and_member_inputs_candidates_are_roots() {
    let conn = setup();
    install_profile(&conn, 'i', &['s']);
    install_profile(&conn, 'c', &['t']);
    resource('u', RemiCatalogResourceKind::SourceSnapshot)
        .insert(&conn)
        .unwrap();
    insert_run(
        &conn,
        "10000000-0000-4000-8000-000000000002",
        Some('i'),
        Some('c'),
        None,
    );
    insert_run_member(
        &conn,
        "10000000-0000-4000-8000-000000000002",
        0,
        Some('s'),
        Some('u'),
    );

    let plan = plan_catalog_collection(&conn).unwrap();
    for profile in ['i', 'c'] {
        assert!(
            plan.reachability
                .contains_profile_revision(&resource_digest(profile))
        );
    }
    for source in ['s', 'u'] {
        assert!(
            plan.reachability
                .contains_source_snapshot(&resource_digest(source))
        );
    }
    assert!(plan.run_candidates.iter().any(|candidate| {
        candidate.resource_kind == RemiCatalogResourceKind::ProfileRevision
            && candidate.resource_sha256 == resource_digest('c')
            && candidate.nonterminal
    }));
    assert!(plan.run_candidates.iter().any(|candidate| {
        candidate.resource_kind == RemiCatalogResourceKind::SourceSnapshot
            && candidate.resource_sha256 == resource_digest('u')
            && candidate.member_ordinal == Some(0)
            && candidate.nonterminal
    }));

    insert_run(
        &conn,
        "10000000-0000-4000-8000-000000000003",
        None,
        Some('c'),
        Some(200),
    );
    let plan = plan_catalog_collection(&conn).unwrap();
    assert!(plan.run_candidates.iter().any(|candidate| {
        candidate.run_id == "10000000-0000-4000-8000-000000000003" && !candidate.nonterminal
    }));
}

#[test]
fn latest_successful_candidate_survives_a_failed_successor_but_not_a_new_success() {
    let conn = setup();
    install_profile(&conn, 'c', &['t']);
    let stale_unrooted_plan = plan_catalog_collection(&conn).unwrap();
    assert_eq!(stale_unrooted_plan.unreachable_profile_resources.len(), 1);
    assert_eq!(stale_unrooted_plan.unreachable_source_resources.len(), 1);
    let first_run = "10000000-0000-4000-8000-000000000031";
    insert_run(&conn, first_run, None, Some('c'), Some(200));
    insert_run_member(&conn, first_run, 0, None, Some('t'));
    select_current_candidate(&conn, first_run);

    let stale_delete = delete_catalog_collection(&conn, &stale_unrooted_plan).unwrap();
    assert!(stale_delete.deleted_profile_resources.is_empty());
    assert!(stale_delete.deleted_source_resources.is_empty());

    let first = plan_catalog_collection(&conn).unwrap();
    assert!(
        first
            .reachability
            .contains_profile_revision(&resource_digest('c'))
    );
    assert!(
        first
            .reachability
            .contains_source_snapshot(&resource_digest('t'))
    );
    assert!(first.unreachable_profile_resources.is_empty());
    assert!(first.unreachable_source_resources.is_empty());

    let failed_run = "10000000-0000-4000-8000-000000000032";
    insert_run(&conn, failed_run, None, None, None);
    conn.execute(
        "UPDATE repository_sync_runs
         SET state = 'abandoned', finished_at = 300,
             failure_stage = 'fetching_objects', failure_category = 'transport',
             failure_evidence = 'fixture interrupted body'
         WHERE run_id = ?1",
        [failed_run],
    )
    .unwrap();
    select_scope_run(&conn, failed_run);

    let after_failure = plan_catalog_collection(&conn).unwrap();
    assert!(
        after_failure
            .reachability
            .contains_profile_revision(&resource_digest('c'))
    );
    assert!(
        after_failure
            .reachability
            .contains_source_snapshot(&resource_digest('t'))
    );
    let stale_delete = delete_catalog_collection(&conn, &stale_unrooted_plan).unwrap();
    assert!(stale_delete.deleted_profile_resources.is_empty());
    assert!(stale_delete.deleted_source_resources.is_empty());

    install_profile(&conn, 'd', &['u']);
    let second_run = "10000000-0000-4000-8000-000000000033";
    insert_run(&conn, second_run, None, Some('d'), Some(300));
    insert_run_member(&conn, second_run, 0, None, Some('u'));
    select_current_candidate(&conn, second_run);

    let second = plan_catalog_collection(&conn).unwrap();
    assert!(
        second
            .reachability
            .contains_profile_revision(&resource_digest('d'))
    );
    assert!(
        second
            .reachability
            .contains_source_snapshot(&resource_digest('u'))
    );
    assert!(
        second
            .unreachable_profile_resources
            .iter()
            .any(|resource| { resource.resource_sha256 == resource_digest('c') })
    );
    assert!(
        second
            .unreachable_source_resources
            .iter()
            .any(|resource| { resource.resource_sha256 == resource_digest('t') })
    );
}

#[test]
fn shared_source_survives_until_all_profile_edges_are_removed() {
    let conn = setup();
    install_profile(&conn, 'a', &['s']);
    install_profile(&conn, 'b', &['s']);
    let run_id = "10000000-0000-4000-8000-000000000004";
    insert_run(&conn, run_id, None, Some('a'), Some(100));
    activate(&conn, 'a', run_id);

    let first = plan_catalog_collection(&conn).unwrap();
    let first_deleted = delete_catalog_collection(&conn, &first).unwrap();
    assert!(
        first_deleted
            .deleted_profile_resources
            .iter()
            .any(|resource| resource.resource_sha256 == resource_digest('b'))
    );
    assert!(first_deleted.deleted_source_resources.is_empty());
    assert!(
        RemiCatalogResource::find_by_sha256(&conn, &resource_digest('s'))
            .unwrap()
            .is_some()
    );

    assert!(RemiActiveProfileRevision::retire(&conn, "fedora-44").unwrap());
    let second = plan_catalog_collection(&conn).unwrap();
    let second_deleted = delete_catalog_collection(&conn, &second).unwrap();
    assert!(
        second_deleted
            .deleted_profile_resources
            .iter()
            .any(|resource| resource.resource_sha256 == resource_digest('a'))
    );
    assert!(
        second_deleted
            .deleted_source_resources
            .iter()
            .any(|resource| resource.resource_sha256 == resource_digest('s'))
    );
}

#[test]
fn deletion_revalidates_new_roots_and_removes_profile_before_source() {
    let conn = setup();
    install_profile(&conn, 'p', &['s']);
    let plan = plan_catalog_collection(&conn).unwrap();
    assert_eq!(plan.unreachable_profile_resources.len(), 1);
    assert_eq!(plan.unreachable_source_resources.len(), 1);

    // A root can appear after planning. The immediate deletion transaction
    // must re-read it and preserve both the profile and its source edge.
    let run_id = "10000000-0000-4000-8000-000000000005";
    insert_run(&conn, run_id, None, Some('p'), None);
    activate(&conn, 'p', run_id);
    let result = delete_catalog_collection(&conn, &plan).unwrap();
    assert!(result.deleted_profile_resources.is_empty());
    assert!(result.deleted_source_resources.is_empty());

    RemiActiveProfileRevision::retire(&conn, "fedora-44").unwrap();
    conn.execute(
        "UPDATE repository_sync_runs
             SET state = 'published', finished_at = 200
             WHERE run_id = ?1",
        [run_id],
    )
    .unwrap();
    let plan = plan_catalog_collection(&conn).unwrap();
    let result = delete_catalog_collection(&conn, &plan).unwrap();
    assert_eq!(result.deleted_profile_resources.len(), 1);
    assert_eq!(result.deleted_source_resources.len(), 1);
    assert!(
        RemiCatalogResource::find_by_sha256(&conn, &resource_digest('p'))
            .unwrap()
            .is_none()
    );
    assert!(
        RemiCatalogResource::find_by_sha256(&conn, &resource_digest('s'))
            .unwrap()
            .is_none()
    );
    let intents = list_catalog_deletion_intents(&conn).unwrap();
    assert_eq!(intents.len(), 2);
    let reinsertion_error = resource('p', RemiCatalogResourceKind::ProfileRevision)
        .insert(&conn)
        .unwrap_err();
    assert!(
        reinsertion_error
            .to_string()
            .contains("incomplete deletion intent")
    );
    for intent in &intents {
        assert!(acknowledge_catalog_deletion(&conn, intent).unwrap());
    }
    assert!(list_catalog_deletion_intents(&conn).unwrap().is_empty());
}
