// conary-core/src/db/models/remi_catalog/tests.rs

use super::*;
use crate::db::models::{
    NativeSourceEcosystem, NativeSourceStream, Repository, RepositoryPolicyScope,
    RepositorySourcePolicy, RepositoryUpdateMode,
};
use crate::db::schema::ensure_current;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};

const OWNER_ONE: &str = "00000000-0000-4000-8000-000000000001";
const OWNER_TWO: &str = "00000000-0000-4000-8000-000000000002";
const RUN_ONE: &str = "10000000-0000-4000-8000-000000000001";
const RUN_TWO: &str = "10000000-0000-4000-8000-000000000002";

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn resource_digest(byte: char) -> String {
    crate::hash::sha256(format!("{{\"resource\":\"{byte}\"}}").as_bytes())
}

fn resource(
    sha: char,
    artifact: char,
    kind: RemiCatalogResourceKind,
    durable: bool,
) -> RemiCatalogResource {
    RemiCatalogResource {
        resource_sha256: resource_digest(sha),
        kind,
        source_profile: "fedora-44".to_string(),
        artifact_sha256: digest(artifact),
        artifact_size: 4096,
        logical_digest_sha256: digest('c'),
        manifest_json: format!("{{\"resource\":\"{sha}\"}}"),
        physical_attestation: RemiCatalogPhysicalAttestation::test_for_catalog_size(4096),
        durable,
        created_at: 100,
    }
}

fn member(profile: char, source: char, ordinal: i64) -> RemiProfileRevisionMember {
    RemiProfileRevisionMember {
        profile_revision_sha256: resource_digest(profile),
        ordinal,
        source_snapshot_sha256: resource_digest(source),
        source_identity: "fixture-source".to_string(),
        repository_identity: "fixture-repository".to_string(),
        stream_kind: "release".to_string(),
        stream_identity: "fixture".to_string(),
        role: ProfileSourceRole::Base,
        precedence: ordinal,
        required: true,
    }
}

fn setup() -> (Connection, Repository) {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let mut repo = Repository::new(
        "fixture-repository".to_string(),
        "https://fixture.test".to_string(),
    );
    repo.source_profile = Some("fedora-44".to_string());
    repo.profile_member_role = Some(ProfileSourceRole::Base);
    repo.profile_member_required = true;
    repo.set_parser_config(RepositoryParserConfig::Rpm {
        architecture: "x86_64".to_string(),
    })
    .unwrap();
    repo.set_trust_policy(RepositoryTrustPolicy::Rpm {
        metadata: RpmMetadataAuthority::Metalink {
            url: "https://fixture.test/metalink".to_string(),
        },
        package_keys: vec![
            OpenPgpTrustRoot::new("https://fixture.test/key".to_string(), "A".repeat(40)).unwrap(),
        ],
    })
    .unwrap();
    repo.set_native_source_policy(
        RepositorySourcePolicy::new(
            "fixture-source",
            RepositoryPolicyScope::repository("fixture-repository").unwrap(),
            NativeSourceEcosystem::Rpm,
            NativeSourceStream::release("fixture").unwrap(),
            RepositoryUpdateMode::Follow,
        )
        .unwrap(),
        "fixture-repository",
        None,
    )
    .unwrap();
    repo.id = Some(repo.insert(&conn).unwrap());
    (conn, repo)
}

fn insert_run(conn: &Connection, repo: &Repository, run_id: &str, owner: &str, epoch: i64) {
    let repository_id = repo.id.unwrap();
    let (profile, source) = if run_id == RUN_ONE {
        ('d', 'e')
    } else {
        ('f', '0')
    };
    conn.execute(
        "INSERT INTO repository_sync_runs (
                 run_id, source_profile, owner_instance_uuid, fencing_epoch,
                 candidate_profile_digest, state, started_at, heartbeat_at,
                 lease_expires_at, finished_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'candidate', 100, 100, 100, 100)",
        params![
            run_id,
            repo.source_profile.as_deref(),
            owner,
            epoch,
            resource_digest(profile),
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repository_sync_scopes (
             source_profile, fencing_epoch, current_run_id
             ) VALUES (?1, ?2, ?3)
             ON CONFLICT(source_profile) DO UPDATE SET
                 fencing_epoch = excluded.fencing_epoch,
                 current_run_id = excluded.current_run_id",
        params![repo.source_profile.as_deref(), epoch, run_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repository_sync_run_members (
                 run_id, ordinal, repository_id, source_identity,
                 repository_identity, stream_kind, stream_identity, role,
                 precedence, required, candidate_source_snapshot_sha256
             ) VALUES (?1, 0, ?2, ?3, ?4, 'release', 'fixture',
                       'base', 0, 1, ?5)",
        params![
            run_id,
            repository_id,
            "fixture-source",
            "fixture-repository",
            resource_digest(source),
        ],
    )
    .unwrap();
}

fn activation(
    epoch: i64,
    run_id: &str,
    owner: &str,
    profile: char,
) -> RemiProfileRevisionActivation {
    RemiProfileRevisionActivation {
        source_profile: "fedora-44".to_string(),
        profile_revision_sha256: resource_digest(profile),
        artifact_sha256: digest(if profile == 'd' { 'b' } else { 'e' }),
        artifact_size: 4096,
        logical_digest_sha256: digest('c'),
        run_id: run_id.to_string(),
        owner_instance_uuid: owner.to_string(),
        fencing_epoch: epoch,
    }
}

fn install_catalog(conn: &Connection, profile: char, source: char, durable: bool) {
    resource(
        source,
        source,
        RemiCatalogResourceKind::SourceSnapshot,
        durable,
    )
    .insert(conn)
    .unwrap();
    resource(
        profile,
        if profile == 'd' { 'b' } else { 'e' },
        RemiCatalogResourceKind::ProfileRevision,
        durable,
    )
    .insert(conn)
    .unwrap();
    member(profile, source, 0).insert(conn).unwrap();
}

#[test]
fn activation_requires_exact_owner_and_durable_resources() {
    let (conn, repo) = setup();
    insert_run(&conn, &repo, RUN_ONE, OWNER_ONE, 1);
    install_catalog(&conn, 'd', 'e', false);

    let error = activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("lacks exact durable catalog metadata")
    );
    assert!(
        RemiActiveProfileRevision::find(&conn, "fedora-44")
            .unwrap()
            .is_none()
    );

    // Durability is immutable metadata: replacing the resource is the
    // only valid way to register a post-fsync artifact.
    conn.execute(
        "DELETE FROM remi_catalog_resources WHERE resource_sha256 = ?1",
        [resource_digest('d')],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM remi_catalog_resources WHERE resource_sha256 = ?1",
        [resource_digest('e')],
    )
    .unwrap();
    install_catalog(&conn, 'd', 'e', true);

    let wrong_owner =
        activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_TWO, 'd'), 200)
            .unwrap_err();
    assert!(wrong_owner.to_string().contains("lost run"));
    assert!(
        RemiActiveProfileRevision::find(&conn, "fedora-44")
            .unwrap()
            .is_none()
    );

    let outcome =
        activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200).unwrap();
    assert!(matches!(
        outcome,
        RemiProfileActivationOutcome::Activated(_)
    ));
}

#[test]
fn activation_rejects_a_live_ready_run_until_candidate_completion() {
    let (conn, repo) = setup();
    insert_run(&conn, &repo, RUN_ONE, OWNER_ONE, 1);
    install_catalog(&conn, 'd', 'e', true);
    conn.execute(
        "UPDATE repository_sync_runs
             SET state = 'ready_to_publish', finished_at = NULL,
                 heartbeat_at = 100, lease_expires_at = 1000
             WHERE run_id = ?1",
        [RUN_ONE],
    )
    .unwrap();

    let error = activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200)
        .unwrap_err();
    assert!(error.to_string().contains("lost run"));
    assert!(
        RemiActiveProfileRevision::find(&conn, "fedora-44")
            .unwrap()
            .is_none()
    );
}

#[test]
fn activation_accepts_latest_successful_candidate_after_a_failed_successor() {
    let (conn, repo) = setup();
    insert_run(&conn, &repo, RUN_ONE, OWNER_ONE, 1);
    install_catalog(&conn, 'd', 'e', true);
    insert_run(&conn, &repo, RUN_TWO, OWNER_TWO, 2);
    conn.execute(
        "UPDATE repository_sync_runs
             SET state = 'abandoned', failure_stage = 'fetching_objects',
                 failure_category = 'transport', failure_evidence = 'body ended early'
             WHERE run_id = ?1",
        [RUN_TWO],
    )
    .unwrap();

    let outcome =
        activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200).unwrap();

    assert!(matches!(
        outcome,
        RemiProfileActivationOutcome::Activated(_)
    ));
    assert_eq!(
        RemiActiveProfileRevision::find(&conn, "fedora-44")
            .unwrap()
            .unwrap()
            .activation_run_id,
        RUN_ONE
    );
}

#[test]
fn activation_rejects_repository_precedence_changed_after_run_start() {
    let (conn, repo) = setup();
    insert_run(&conn, &repo, RUN_ONE, OWNER_ONE, 1);
    install_catalog(&conn, 'd', 'e', true);
    conn.execute(
        "UPDATE repositories SET priority = priority + 1 WHERE id = ?1",
        [repo.id.unwrap()],
    )
    .unwrap();

    let error = activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200)
        .unwrap_err();

    assert!(error.to_string().contains("repository binding changed"));
    assert!(
        RemiActiveProfileRevision::find(&conn, "fedora-44")
            .unwrap()
            .is_none()
    );
}

#[test]
fn stale_and_replayed_activation_leave_pointer_unchanged() {
    let (conn, repo) = setup();
    insert_run(&conn, &repo, RUN_ONE, OWNER_ONE, 1);
    install_catalog(&conn, 'd', 'e', true);
    let first =
        activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200).unwrap();
    let first = match first {
        RemiProfileActivationOutcome::Activated(value) => value,
        RemiProfileActivationOutcome::AlreadyActive(_) => panic!("first activation replayed"),
    };

    let replay =
        activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200).unwrap();
    assert!(matches!(
        replay,
        RemiProfileActivationOutcome::AlreadyActive(_)
    ));
    assert_eq!(
        RemiActiveProfileRevision::find(&conn, "fedora-44").unwrap(),
        Some(first.clone())
    );

    insert_run(&conn, &repo, RUN_TWO, OWNER_TWO, 2);
    install_catalog(&conn, 'f', '0', true);
    assert_eq!(
        RemiActiveProfileRevision::find(&conn, "fedora-44").unwrap(),
        Some(first.clone())
    );
    let second =
        activate_profile_revision_at(&conn, &activation(2, RUN_TWO, OWNER_TWO, 'f'), 200).unwrap();
    assert!(matches!(second, RemiProfileActivationOutcome::Activated(_)));
    let after_second = RemiActiveProfileRevision::find(&conn, "fedora-44")
        .unwrap()
        .unwrap();
    assert_eq!(after_second.fencing_epoch, 2);

    let stale = activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200)
        .unwrap_err();
    assert!(stale.to_string().contains("lost run"));
    assert_eq!(
        RemiActiveProfileRevision::find(&conn, "fedora-44").unwrap(),
        Some(after_second)
    );
}

#[test]
fn retiring_active_pointer_preserves_immutable_resources() {
    let (conn, repo) = setup();
    insert_run(&conn, &repo, RUN_ONE, OWNER_ONE, 1);
    install_catalog(&conn, 'd', 'e', true);
    activate_profile_revision_at(&conn, &activation(1, RUN_ONE, OWNER_ONE, 'd'), 200).unwrap();

    assert!(RemiActiveProfileRevision::retire(&conn, "fedora-44").unwrap());
    assert!(
        RemiActiveProfileRevision::find(&conn, "fedora-44")
            .unwrap()
            .is_none()
    );
    assert!(!RemiActiveProfileRevision::retire(&conn, "fedora-44").unwrap());
    assert!(
        RemiCatalogResource::find_by_sha256(&conn, &resource_digest('d'))
            .unwrap()
            .is_some()
    );
    assert!(
        RemiCatalogResource::find_by_sha256(&conn, &resource_digest('e'))
            .unwrap()
            .is_some()
    );
}

#[test]
fn pins_are_exact_durable_roots_and_cannot_be_repointed() {
    let (conn, _repo) = setup();
    install_catalog(&conn, 'd', 'e', true);
    let pin = RemiProfileRevisionPin {
        pin_id: "conversion-1".to_string(),
        source_profile: "fedora-44".to_string(),
        profile_revision_sha256: resource_digest('d'),
        owner_kind: RemiRevisionPinKind::Conversion,
        owner_identity: "conversion-work-1".to_string(),
        runtime_session_id: None,
        pinned_at: 200,
    };
    pin.insert(&conn).unwrap();
    assert_eq!(
        RemiProfileRevisionPin::find(&conn, "conversion-1").unwrap(),
        Some(pin.clone())
    );
    assert_eq!(
        RemiProfileRevisionPin::list_for_revision(&conn, "fedora-44", &resource_digest('d'),)
            .unwrap(),
        vec![pin]
    );

    let error = conn
        .execute(
            "UPDATE remi_profile_revision_pins
                 SET profile_revision_sha256 = ?1 WHERE pin_id = 'conversion-1'",
            [digest('f')],
        )
        .unwrap_err();
    assert!(error.to_string().contains("cannot be repointed"));
    assert!(RemiProfileRevisionPin::release(&conn, "conversion-1").unwrap());
    assert!(!RemiProfileRevisionPin::release(&conn, "conversion-1").unwrap());
}

#[test]
fn members_are_returned_in_declared_ordinal_order() {
    let (conn, _repo) = setup();
    resource('b', 'b', RemiCatalogResourceKind::SourceSnapshot, true)
        .insert(&conn)
        .unwrap();
    resource('c', 'c', RemiCatalogResourceKind::SourceSnapshot, true)
        .insert(&conn)
        .unwrap();
    resource('a', 'b', RemiCatalogResourceKind::ProfileRevision, true)
        .insert(&conn)
        .unwrap();
    let mut second = member('a', 'c', 1);
    second.repository_identity = "fixture-updates".to_string();
    second.insert(&conn).unwrap();
    member('a', 'b', 0).insert(&conn).unwrap();
    let members =
        RemiProfileRevisionMember::list_for_revision(&conn, &resource_digest('a')).unwrap();
    assert_eq!(
        members
            .iter()
            .map(|member| member.ordinal)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(members[0].source_snapshot_sha256, resource_digest('b'));
    assert_eq!(members[1].source_snapshot_sha256, resource_digest('c'));

    let update_error = conn
        .execute(
            "UPDATE remi_profile_revision_members SET precedence = 99
                 WHERE profile_revision_sha256 = ?1 AND ordinal = 0",
            [resource_digest('a')],
        )
        .unwrap_err();
    assert!(update_error.to_string().contains("cannot be updated"));
    let delete_error = conn
        .execute(
            "DELETE FROM remi_profile_revision_members
                 WHERE profile_revision_sha256 = ?1 AND ordinal = 0",
            [resource_digest('a')],
        )
        .unwrap_err();
    assert!(delete_error.to_string().contains("cannot be deleted"));

    conn.execute(
        "DELETE FROM remi_catalog_resources WHERE resource_sha256 = ?1",
        [resource_digest('a')],
    )
    .unwrap();
    assert!(
        RemiProfileRevisionMember::list_for_revision(&conn, &resource_digest('a'))
            .unwrap()
            .is_empty()
    );
}
