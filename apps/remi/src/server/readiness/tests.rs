// apps/remi/src/server/readiness/tests.rs

use super::*;
use conary_core::db::models::RemiCatalogPhysicalAttestation;
use conary_core::db::schema;
use conary_core::repository::catalog::{
    PortableManifestAttestationV1, portable_chunk_count_v1, portable_manifest_size_v1,
};

fn fixture_physical_attestation(
    catalog_size: u64,
    marker: &[u8],
) -> RemiCatalogPhysicalAttestation {
    let chunk_count = portable_chunk_count_v1(catalog_size).expect("fixture chunk count");
    RemiCatalogPhysicalAttestation::new(
        PortableManifestAttestationV1 {
            sha256: conary_core::hash::sha256(marker),
            size: portable_manifest_size_v1(chunk_count).expect("fixture portable size"),
        },
        catalog_size,
    )
    .expect("fixture physical attestation")
}

fn inputs_for(dir: &Path) -> ReadinessInputs {
    let db_path = dir.join("metadata/conary.db");
    std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create metadata dir");
    let chunk_dir = dir.join("chunks");
    let cache_dir = dir.join("cache");
    let catalog_dir = dir.join("catalogs");
    std::fs::create_dir_all(&chunk_dir).expect("create chunk dir");
    std::fs::create_dir_all(&cache_dir).expect("create cache dir");
    std::fs::create_dir_all(&catalog_dir).expect("create catalog dir");
    let database_writer = crate::server::database_writer::DatabaseWriter::default();
    let catalog_authority =
        CatalogAuthority::from_paths(db_path.clone(), catalog_dir, database_writer);
    ReadinessInputs {
        db_path,
        chunk_dir,
        cache_dir,
        min_free_bytes: 0,
        required_source_profiles: Vec::new(),
        publication: PublicationReadiness {
            repository: PublicationPhaseState::Complete,
            canonical: PublicationPhaseState::Complete,
        },
        catalog_authority,
    }
}

fn initialize_database(db_path: &Path) {
    let conn = rusqlite::Connection::open(db_path).expect("open database");
    schema::ensure_current(&conn).expect("initialize current schema");
    conary_core::db::models::RemiRuntimeSession::begin(&conn, 1)
        .expect("install readiness runtime session");
}

#[test]
fn obsolete_active_profile_is_not_ready_instead_of_unavailable() {
    use crate::server::catalog_authority::test_support::ActiveCatalogFixture;

    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate("fedora-44", 1, Vec::new());
    fixture.replace_with_obsolete_schema(&revision);

    assert!(
        !active_profile_is_populated(fixture.authority(), "fedora-44")
            .expect("classify obsolete active profile")
    );
}

fn configure_profile(db_path: &Path, profile: &str) -> i64 {
    use conary_core::db::models::Repository;

    let conn = conary_core::db::open_fast(db_path).expect("open database");
    let mut repository = Repository::new(
        format!("{profile}-readiness"),
        format!("https://example.invalid/{profile}"),
    );
    repository.source_profile = Some(profile.to_string());
    repository.insert(&conn).expect("insert repository")
}

fn insert_operational_package(db_path: &Path, repository_id: i64, profile: &str) {
    use conary_core::db::models::RepositoryPackage;
    use conary_core::repository::versioning::VersionScheme;

    let conn = conary_core::db::open_fast(db_path).expect("open database");
    let mut package = RepositoryPackage::new(
        repository_id,
        "stale-operational-package".to_string(),
        "9.9-1".to_string(),
        VersionScheme::Rpm,
        conary_core::hash::sha256(b"stale-operational-package"),
        1,
        "https://example.invalid/stale.rpm".to_string(),
    );
    package.source_profile = Some(profile.to_string());
    package
        .insert(&conn)
        .expect("insert stale operational package");
}

fn activate_profile_catalog(inputs: &ReadinessInputs, profile: &str, populated: bool) {
    use conary_core::db::models::{
        RemiCatalogResource, RemiCatalogResourceKind, RemiProfileRevisionMember,
    };
    use conary_core::repository::catalog::{
        CATALOG_FILE_NAME, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
        CatalogScopeV1, CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V3, ProfileRevisionV2,
        ProfileSourceMemberV2, SourceStreamKindV1, SourceStreamV1,
        publish_profile_catalog_bundle_verified, write_catalog_candidate,
        write_profile_catalog_manifest,
    };
    use conary_core::repository::versioning::VersionScheme;

    let source_identity = format!("source-{profile}");
    let profile_contract =
        conary_core::repository::supported_profiles::profile_by_public_id(profile)
            .expect("readiness fixture uses public profile");
    let mut declared_members = profile_contract.members().iter().collect::<Vec<_>>();
    declared_members.sort_by_key(|member| std::cmp::Reverse(member.precedence));
    let first_member = declared_members.first().expect("profile member");
    let source_resources = declared_members
        .iter()
        .map(|member| {
            let json = String::from_utf8(
                conary_core::json::canonical_json(&serde_json::json!({
                    "fixture": "readiness-source-snapshot",
                    "profile": profile,
                    "repository_identity": member.repository_identity,
                }))
                .expect("serialize readiness source resource"),
            )
            .expect("source manifest JSON is UTF-8");
            let digest = conary_core::hash::sha256(json.as_bytes());
            (json, digest)
        })
        .collect::<Vec<_>>();
    let origin = CatalogPackageOriginV1::Profile {
        member_ordinal: 0,
        source_identity: source_identity.clone(),
        repository_identity: first_member.repository_identity.clone(),
        source_snapshot_sha256: source_resources[0].1.clone(),
    };
    let packages = populated
        .then(|| CatalogPackageRecordV1 {
            package_key_sha256: String::new(),
            origin,
            source_profile: profile.to_string(),
            name: "catalog-readiness-probe".to_string(),
            version: "1.0-1".to_string(),
            package_release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            debian_multi_arch: None,
            description: None,
            checksum: conary_core::hash::sha256(b"catalog-readiness-probe"),
            size: 1,
            download_url: format!("https://example.invalid/{profile}/package.rpm"),
            metadata: None,
            is_security_update: false,
            severity: None,
            cve_ids: None,
            advisory_id: None,
            advisory_url: None,
            version_scheme: VersionScheme::Rpm,
            provides: Vec::new(),
            requirement_groups: Vec::new(),
        })
        .into_iter()
        .collect();
    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: profile.to_string(),
        },
        declared_members
            .iter()
            .enumerate()
            .map(
                |(ordinal, member)| CatalogSourceEvidenceV1::SourceSnapshot {
                    member_ordinal: u32::try_from(ordinal).unwrap(),
                    source_identity: source_identity.clone(),
                    repository_identity: member.repository_identity.clone(),
                    source_snapshot_sha256: source_resources[ordinal].1.clone(),
                },
            )
            .collect(),
        packages,
    )
    .expect("build readiness profile catalog");
    let root = inputs
        .db_path
        .parent()
        .and_then(Path::parent)
        .expect("fixture storage root");
    let candidate_dir = root.join(format!("candidate-{profile}"));
    std::fs::create_dir_all(&candidate_dir).expect("create catalog candidate");
    let binding = write_catalog_candidate(candidate_dir.join(CATALOG_FILE_NAME), &content)
        .expect("write readiness catalog candidate");
    let manifest = ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V3,
        profile: profile.to_string(),
        target_architecture: conary_core::repository::supported_profiles::profile_by_id(profile)
            .expect("known readiness profile")
            .target_architecture(),
        projection_version: super::super::catalog_refresh::PROFILE_CATALOG_PROJECTION_VERSION,
        members: declared_members
            .iter()
            .enumerate()
            .map(|(ordinal, member)| ProfileSourceMemberV2 {
                ordinal: u32::try_from(ordinal).unwrap(),
                role: member.role,
                source_identity: source_identity.clone(),
                repository_identity: member.repository_identity.clone(),
                stream: SourceStreamV1 {
                    kind: SourceStreamKindV1::Release,
                    identity: "stable".to_string(),
                },
                precedence: member.precedence,
                required: true,
                source_snapshot_sha256: source_resources[ordinal].1.clone(),
            })
            .collect(),
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    };
    let verification = write_profile_catalog_manifest(&candidate_dir, &manifest)
        .expect("write readiness profile manifest");
    let publication = publish_profile_catalog_bundle_verified(
        &candidate_dir,
        root.join("catalogs"),
        &manifest,
        verification,
    )
    .expect("publish readiness profile catalog");
    let profile_physical_attestation = RemiCatalogPhysicalAttestation::new(
        publication.portable_manifest_attestation,
        manifest.catalog.size,
    )
    .expect("construct readiness profile physical attestation");
    let digest = manifest.manifest_sha256().expect("hash readiness revision");
    let manifest_json = String::from_utf8(
        conary_core::json::canonical_json(&manifest).expect("serialize readiness revision"),
    )
    .expect("manifest JSON is UTF-8");
    let conn = conary_core::db::open_fast(&inputs.db_path).expect("open readiness database");
    for (ordinal, (source_manifest_json, source_snapshot_sha256)) in
        source_resources.iter().enumerate()
    {
        RemiCatalogResource {
            resource_sha256: source_snapshot_sha256.clone(),
            kind: RemiCatalogResourceKind::SourceSnapshot,
            source_profile: profile.to_string(),
            artifact_sha256: conary_core::hash::sha256(
                format!("readiness-source-artifact-{profile}-{ordinal}").as_bytes(),
            ),
            artifact_size: 1,
            logical_digest_sha256: conary_core::hash::sha256(
                format!("readiness-source-logical-{profile}-{ordinal}").as_bytes(),
            ),
            manifest_json: source_manifest_json.clone(),
            physical_attestation: fixture_physical_attestation(
                1,
                format!("readiness-source-portable-{profile}-{ordinal}").as_bytes(),
            ),
            durable: true,
            created_at: 1,
        }
        .insert(&conn)
        .expect("insert readiness source resource");
    }
    RemiCatalogResource {
        resource_sha256: digest.clone(),
        kind: RemiCatalogResourceKind::ProfileRevision,
        source_profile: profile.to_string(),
        artifact_sha256: manifest.catalog.sha256.clone(),
        artifact_size: i64::try_from(manifest.catalog.size).expect("artifact size fits"),
        logical_digest_sha256: manifest.logical_digest_sha256.clone(),
        manifest_json,
        physical_attestation: profile_physical_attestation,
        durable: true,
        created_at: 1,
    }
    .insert(&conn)
    .expect("insert readiness profile resource");
    for member in &manifest.members {
        RemiProfileRevisionMember {
            profile_revision_sha256: digest.clone(),
            ordinal: i64::from(member.ordinal),
            source_snapshot_sha256: member.source_snapshot_sha256.clone(),
            source_identity: member.source_identity.clone(),
            repository_identity: member.repository_identity.clone(),
            stream_kind: "release".to_string(),
            stream_identity: "stable".to_string(),
            role: member.role,
            precedence: i64::from(member.precedence),
            required: true,
        }
        .insert(&conn)
        .expect("insert readiness profile member");
    }
    let run_id = uuid::Uuid::new_v4().to_string();
    let owner_instance_uuid = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO repository_sync_runs (
             run_id, source_profile, owner_instance_uuid, fencing_epoch,
             input_profile_digest, candidate_profile_digest, state,
             started_at, heartbeat_at, lease_expires_at, finished_at
         ) VALUES (?1, ?2, ?3, 1, NULL, ?4, 'published', 1, 1, 1, 1)",
        rusqlite::params![run_id, profile, owner_instance_uuid, digest],
    )
    .expect("insert readiness activation run");
    conn.execute(
        "INSERT INTO remi_active_profile_revisions (
             source_profile, profile_revision_sha256, fencing_epoch,
             activation_run_id, owner_instance_uuid, activated_at
         ) VALUES (?1, ?2, 1, ?3, ?4, 1)",
        rusqlite::params![profile, digest, run_id, owner_instance_uuid,],
    )
    .expect("activate readiness profile catalog");
}

#[test]
fn ready_when_database_directories_and_space_all_satisfy_their_probes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inputs = inputs_for(dir.path());
    initialize_database(&inputs.db_path);
    configure_profile(&inputs.db_path, "fedora-44");
    activate_profile_catalog(&inputs, "fedora-44", true);

    let report = evaluate(&inputs);

    assert!(report.ready, "expected ready, got {report:?}");
    assert_eq!(report.database, ProbeOutcome::Ready);
    assert_eq!(report.expected_schema_revision, SCHEMA_VERSION);
}

#[test]
fn readiness_does_not_claim_database_write_authority() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inputs = inputs_for(dir.path());
    initialize_database(&inputs.db_path);
    configure_profile(&inputs.db_path, "fedora-44");
    activate_profile_catalog(&inputs, "fedora-44", true);
    let database_writer = inputs.catalog_authority.database_writer_for_test();
    let writer_guard = database_writer.hold_for_test();
    let (sender, receiver) = std::sync::mpsc::channel();
    let probe = std::thread::spawn(move || {
        sender.send(evaluate(&inputs)).expect("send report");
    });
    let prompt_report = receiver.recv_timeout(std::time::Duration::from_secs(1));
    drop(writer_guard);
    probe.join().expect("join readiness probe");
    let report = prompt_report.expect("readiness must not wait for the process SQLite writer");
    assert!(report.ready, "expected ready, got {report:?}");
}

#[test]
fn not_ready_when_no_exact_source_profile_is_configured() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inputs = inputs_for(dir.path());
    initialize_database(&inputs.db_path);

    let report = evaluate(&inputs);

    assert!(!report.ready);
    assert!(matches!(
        report.source_profiles,
        ProbeOutcome::NotReady { .. }
    ));
}

#[test]
fn not_ready_while_initial_publication_is_pending() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut inputs = inputs_for(dir.path());
    initialize_database(&inputs.db_path);
    configure_profile(&inputs.db_path, "fedora-44");
    activate_profile_catalog(&inputs, "fedora-44", true);
    inputs.publication.canonical = PublicationPhaseState::Pending;

    let report = evaluate(&inputs);

    assert!(!report.ready);
    assert_eq!(report.publication.canonical, PublicationPhaseState::Pending);
}

#[test]
fn failed_candidate_does_not_retire_usable_publication() {
    let mut publication = PublicationReadiness::default();
    publication.record_repository(PublicationPhaseState::Complete);
    publication.record_canonical(PublicationPhaseState::Partial);

    publication.record_repository(PublicationPhaseState::Failed);
    publication.record_canonical(PublicationPhaseState::Unavailable);

    assert_eq!(publication.repository, PublicationPhaseState::Complete);
    assert_eq!(publication.canonical, PublicationPhaseState::Partial);
    assert!(publication.is_ready());
}

#[test]
fn not_ready_when_a_required_profile_has_zero_packages() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut inputs = inputs_for(dir.path());
    inputs.required_source_profiles = vec!["fedora-44".to_string()];
    initialize_database(&inputs.db_path);
    let repository_id = configure_profile(&inputs.db_path, "fedora-44");
    insert_operational_package(&inputs.db_path, repository_id, "fedora-44");
    activate_profile_catalog(&inputs, "fedora-44", false);

    let report = evaluate(&inputs);

    assert!(!report.ready);
    assert!(matches!(
        report.source_profiles,
        ProbeOutcome::NotReady { .. }
    ));
}

#[test]
fn missing_active_catalog_is_unavailable_without_operational_fallback() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut inputs = inputs_for(dir.path());
    inputs.required_source_profiles = vec!["fedora-44".to_string()];
    initialize_database(&inputs.db_path);
    let repository_id = configure_profile(&inputs.db_path, "fedora-44");
    insert_operational_package(&inputs.db_path, repository_id, "fedora-44");

    let report = evaluate(&inputs);

    assert!(!report.ready);
    match report.source_profiles {
        ProbeOutcome::Unavailable { reason } => assert!(
            reason.contains("has no active immutable catalog revision"),
            "unexpected reason: {reason}"
        ),
        other => panic!("expected unavailable catalog authority, got {other:?}"),
    }
}

#[test]
fn not_ready_when_only_some_required_profiles_are_populated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut inputs = inputs_for(dir.path());
    inputs.required_source_profiles = vec!["fedora-44".to_string(), "ubuntu-26.04".to_string()];
    initialize_database(&inputs.db_path);
    configure_profile(&inputs.db_path, "fedora-44");
    activate_profile_catalog(&inputs, "fedora-44", true);
    configure_profile(&inputs.db_path, "ubuntu-26.04");
    activate_profile_catalog(&inputs, "ubuntu-26.04", false);

    let report = evaluate(&inputs);

    assert!(!report.ready);
    match report.source_profiles {
        ProbeOutcome::NotReady { reason } => {
            assert!(
                reason.contains("ubuntu-26.04"),
                "unexpected reason: {reason}"
            );
            assert!(!reason.contains("fedora-44"), "unexpected reason: {reason}");
        }
        other => panic!("expected NotReady, got {other:?}"),
    }
}

#[test]
fn ready_when_every_required_profile_is_populated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut inputs = inputs_for(dir.path());
    inputs.required_source_profiles = vec!["fedora-44".to_string(), "ubuntu-26.04".to_string()];
    initialize_database(&inputs.db_path);
    configure_profile(&inputs.db_path, "fedora-44");
    activate_profile_catalog(&inputs, "fedora-44", true);
    configure_profile(&inputs.db_path, "ubuntu-26.04");
    activate_profile_catalog(&inputs, "ubuntu-26.04", true);

    let report = evaluate(&inputs);

    assert!(report.ready, "expected ready, got {report:?}");
    assert_eq!(report.source_profiles, ProbeOutcome::Ready);
}

/// The exact defect this module replaces: the previous check accepted a
/// missing database whenever its parent directory existed, which on any
/// normal deployment is always true.
#[test]
fn not_ready_when_database_is_absent_but_its_parent_directory_exists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inputs = inputs_for(dir.path());

    assert!(
        inputs.db_path.parent().expect("db parent").is_dir(),
        "the parent directory must exist for this regression to be meaningful"
    );
    assert!(!inputs.db_path.exists(), "the database must be absent");

    let report = evaluate(&inputs);

    assert!(!report.ready, "absent database must not report ready");
    assert!(
        matches!(report.database, ProbeOutcome::NotReady { .. }),
        "expected NotReady, got {:?}",
        report.database
    );
}

#[test]
fn not_ready_when_database_carries_a_retired_schema_revision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inputs = inputs_for(dir.path());

    let conn = rusqlite::Connection::open(&inputs.db_path).expect("open database");
    conn.execute_batch(
        "CREATE TABLE schema_version (version INTEGER NOT NULL);
         INSERT INTO schema_version (version) VALUES (3);
         CREATE TABLE converted_packages (id INTEGER PRIMARY KEY);",
    )
    .expect("write retired schema");
    drop(conn);

    let report = evaluate(&inputs);

    assert!(!report.ready, "retired schema must not report ready");
    match report.database {
        ProbeOutcome::NotReady { ref reason } => {
            assert!(
                reason.contains("rebuild"),
                "reason should name the rebuild requirement, got {reason}"
            );
        }
        ref other => panic!("expected NotReady, got {other:?}"),
    }
}

#[test]
fn database_probe_is_unavailable_when_the_file_cannot_be_opened() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inputs = inputs_for(dir.path());
    std::fs::write(&inputs.db_path, b"this is not a sqlite database").expect("write junk database");

    let report = evaluate(&inputs);

    assert!(!report.ready, "unreadable database must not report ready");
    assert!(
        matches!(
            report.database,
            ProbeOutcome::NotReady { .. } | ProbeOutcome::Unavailable { .. }
        ),
        "expected a failing outcome, got {:?}",
        report.database
    );
}

#[test]
fn not_ready_when_the_chunk_directory_is_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inputs = inputs_for(dir.path());
    initialize_database(&inputs.db_path);
    std::fs::remove_dir(&inputs.chunk_dir).expect("remove chunk dir");

    let report = evaluate(&inputs);

    assert!(!report.ready);
    assert!(matches!(report.chunk_dir, ProbeOutcome::NotReady { .. }));
}

#[test]
fn not_ready_when_the_cache_path_is_a_file_rather_than_a_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inputs = inputs_for(dir.path());
    initialize_database(&inputs.db_path);
    std::fs::remove_dir(&inputs.cache_dir).expect("remove cache dir");
    std::fs::write(&inputs.cache_dir, b"not a directory").expect("write file at cache path");

    let report = evaluate(&inputs);

    assert!(!report.ready);
    assert!(matches!(report.cache_dir, ProbeOutcome::NotReady { .. }));
}

/// Insufficient space is a genuine NotReady, distinct from a probe that
/// could not run at all.
#[test]
fn not_ready_when_free_space_is_below_the_configured_threshold() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut inputs = inputs_for(dir.path());
    initialize_database(&inputs.db_path);
    inputs.min_free_bytes = u64::MAX;

    let report = evaluate(&inputs);

    assert!(!report.ready, "insufficient space must not report ready");
    match report.free_space {
        ProbeOutcome::NotReady { ref reason } => {
            assert!(
                reason.contains("below the required"),
                "reason should state the shortfall, got {reason}"
            );
        }
        ref other => panic!("expected NotReady, got {other:?}"),
    }
}

/// A probe that cannot execute must not read as success. The previous
/// implementation returned `true` on statvfs failure.
#[test]
fn free_space_probe_is_unavailable_when_the_path_cannot_be_measured() {
    let missing = Path::new("/nonexistent-remi-readiness-probe-target");

    let outcome = probe_free_space(missing, 0);

    assert!(
        matches!(outcome, ProbeOutcome::Unavailable { .. }),
        "a failed probe must be Unavailable, got {outcome:?}"
    );
    assert!(!outcome.is_ready(), "a failed probe must never be ready");
}

#[test]
fn report_serializes_each_probe_state_distinctly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let inputs = inputs_for(dir.path());

    let json = serde_json::to_value(evaluate(&inputs)).expect("serialize report");

    assert_eq!(json["ready"], serde_json::json!(false));
    assert_eq!(json["database"]["state"], serde_json::json!("not_ready"));
    assert_eq!(json["chunk_dir"]["state"], serde_json::json!("ready"));
    assert!(
        json["database"]["reason"].is_string(),
        "a failing probe must carry a reason"
    );
}
