// apps/remi/src/server/catalog_gc/tests.rs

//! Focused exact catalog collection and cache-release proofs.

use super::*;
use crate::server::catalog_authority::ProfileRevisionSelection;
use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn resource_digest(byte: char) -> String {
    conary_core::hash::sha256(format!("{{\"resource\":\"{byte}\"}}").as_bytes())
}

fn physical_attestation(
    catalog_size: u64,
    byte: char,
) -> conary_core::db::models::RemiCatalogPhysicalAttestation {
    let chunk_count = portable_chunk_count_v1(catalog_size).unwrap();
    conary_core::db::models::RemiCatalogPhysicalAttestation::new(
        conary_core::repository::catalog::PortableManifestAttestationV1 {
            sha256: digest(byte),
            size: portable_manifest_size_v1(chunk_count).unwrap(),
        },
        catalog_size,
    )
    .unwrap()
}

fn exact_bundle(path: &Path, kind: RemiCatalogResourceKind, manifest_byte: char) {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join(CATALOG_FILE_NAME), b"catalog").unwrap();
    fs::write(
        path.join(CATALOG_MANIFEST_FILE_NAME),
        format!("{{\"resource\":\"{manifest_byte}\"}}"),
    )
    .unwrap();
    let portable_size = portable_manifest_size_v1(portable_chunk_count_v1(7).unwrap()).unwrap();
    fs::write(
        path.join(CATALOG_PORTABLE_MANIFEST_FILE_NAME),
        vec![0; usize::try_from(portable_size).unwrap()],
    )
    .unwrap();
    if kind == RemiCatalogResourceKind::SourceSnapshot {
        let metadata = path.join(SOURCE_METADATA_DIRECTORY_NAME);
        fs::create_dir(&metadata).unwrap();
        fs::write(metadata.join(digest('e')), b"authenticated metadata").unwrap();
    }
}

fn retired_exact_bundle(path: &Path, kind: RemiCatalogResourceKind, manifest_byte: char) {
    exact_bundle(path, kind, manifest_byte);
    fs::remove_file(path.join(CATALOG_PORTABLE_MANIFEST_FILE_NAME)).unwrap();
}

#[test]
fn conversion_proof_gc_keeps_only_the_current_converter_version() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open_fast(&db_path).unwrap();
    for (key, version) in [
        (digest('a'), env!("CARGO_PKG_VERSION")),
        (digest('b'), "0.0.0-obsolete"),
    ] {
        let proof = serde_json::json!({"key": {"converter_version": version}});
        conn.execute(
            "INSERT INTO remi_conversion_proofs (
                 proof_key_sha256, proof_json, original_format, transport_json,
                 total_size, ccs_path, scriptlet_summary_json
             ) VALUES (?1, ?2, 'rpm', '{}', 0, '/tmp/proof.ccs', '{}')",
            rusqlite::params![key, proof.to_string()],
        )
        .unwrap();
    }

    assert_eq!(delete_unreachable_conversion_proofs(&conn).unwrap(), 1);
    let retained: Vec<String> = conn
        .prepare("SELECT proof_key_sha256 FROM remi_conversion_proofs ORDER BY proof_key_sha256")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(retained, vec![digest('a')]);
}

#[tokio::test]
async fn restart_recovery_fences_and_acknowledges_exact_expired_candidate() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("remi.db");
    let candidate_root = root.path().join("catalog-candidates");
    fs::create_dir(&candidate_root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let run_id = "10000000-0000-4000-8000-000000000001";
    let run_path = candidate_root.join(run_id);
    fs::create_dir(&run_path).unwrap();
    fs::write(run_path.join("private-candidate"), b"fixture").unwrap();
    let conn = conary_core::db::open_fast(&db_path).unwrap();
    conn.execute(
        "INSERT INTO repository_sync_runs (
             run_id, source_profile, owner_instance_uuid, fencing_epoch,
             state, started_at, heartbeat_at, lease_expires_at
         ) VALUES (?1, 'fedora-44', ?2, 1, 'fetching_objects', 1, 1, 1)",
        rusqlite::params![run_id, "00000000-0000-4000-8000-000000000001",],
    )
    .unwrap();
    drop(conn);

    assert_eq!(
        recover_catalog_refresh_runs_uncoordinated(
            db_path.clone(),
            candidate_root,
            DatabaseWriter::default(),
        )
        .await
        .unwrap(),
        1
    );
    assert!(!run_path.exists());
    let conn = conary_core::db::open_fast(&db_path).unwrap();
    let recovered: (String, bool) = conn
        .query_row(
            "SELECT state, candidate_cleaned_at IS NOT NULL
             FROM repository_sync_runs WHERE run_id = ?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(recovered, ("abandoned".to_string(), true));
}

#[test]
fn terminal_bundle_removal_accepts_retired_layout_and_refuses_malformed_content() {
    let root = tempfile::tempdir().unwrap();
    let catalog_root = root.path().join("catalogs");
    fs::create_dir_all(catalog_root.join("sources")).unwrap();
    let exact_digest = resource_digest('a');
    let exact = bundle_path(
        &catalog_root,
        RemiCatalogResourceKind::SourceSnapshot,
        "fedora-44",
        &exact_digest,
    );
    exact_bundle(&exact, RemiCatalogResourceKind::SourceSnapshot, 'a');
    assert!(
        remove_exact_bundle(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &exact_digest,
            CatalogBundleDeletionPolicy::CurrentOnly,
        )
        .unwrap()
    );
    assert!(!exact.exists());

    for (byte, kind) in [
        ('7', RemiCatalogResourceKind::SourceSnapshot),
        ('8', RemiCatalogResourceKind::ProfileRevision),
    ] {
        let retired_digest = resource_digest(byte);
        let retired = bundle_path(&catalog_root, kind, "fedora-44", &retired_digest);
        if kind == RemiCatalogResourceKind::ProfileRevision {
            fs::create_dir_all(retired.parent().unwrap()).unwrap();
        }
        retired_exact_bundle(&retired, kind, byte);
        assert!(
            remove_exact_bundle(
                &catalog_root,
                kind,
                "fedora-44",
                &retired_digest,
                CatalogBundleDeletionPolicy::CurrentOnly,
            )
            .is_err()
        );
        assert!(retired.exists());
        assert!(
            remove_exact_bundle(
                &catalog_root,
                kind,
                "fedora-44",
                &retired_digest,
                CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54,
            )
            .unwrap()
        );
        assert!(!retired.exists());
    }

    let mismatched_manifest_digest = resource_digest('2');
    let mismatched_manifest = bundle_path(
        &catalog_root,
        RemiCatalogResourceKind::SourceSnapshot,
        "fedora-44",
        &mismatched_manifest_digest,
    );
    retired_exact_bundle(
        &mismatched_manifest,
        RemiCatalogResourceKind::SourceSnapshot,
        '3',
    );
    assert!(
        remove_exact_bundle(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &mismatched_manifest_digest,
            CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54,
        )
        .is_err()
    );
    assert!(mismatched_manifest.exists());

    let malformed_digest = resource_digest('b');
    let malformed = bundle_path(
        &catalog_root,
        RemiCatalogResourceKind::SourceSnapshot,
        "fedora-44",
        &malformed_digest,
    );
    exact_bundle(&malformed, RemiCatalogResourceKind::SourceSnapshot, 'b');
    fs::write(malformed.join("unexpected"), b"evidence").unwrap();
    assert!(
        remove_exact_bundle(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &malformed_digest,
            CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54,
        )
        .is_err()
    );
    assert!(malformed.join("unexpected").exists());

    let malformed_retired_digest = resource_digest('6');
    let malformed_retired = bundle_path(
        &catalog_root,
        RemiCatalogResourceKind::SourceSnapshot,
        "fedora-44",
        &malformed_retired_digest,
    );
    retired_exact_bundle(
        &malformed_retired,
        RemiCatalogResourceKind::SourceSnapshot,
        '6',
    );
    fs::write(malformed_retired.join("unexpected"), b"evidence").unwrap();
    assert!(
        remove_exact_bundle(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &malformed_retired_digest,
            CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54,
        )
        .is_err()
    );
    assert!(malformed_retired.join("unexpected").exists());

    let malformed_portable_digest = resource_digest('9');
    let malformed_portable = bundle_path(
        &catalog_root,
        RemiCatalogResourceKind::SourceSnapshot,
        "fedora-44",
        &malformed_portable_digest,
    );
    exact_bundle(
        &malformed_portable,
        RemiCatalogResourceKind::SourceSnapshot,
        '9',
    );
    fs::write(
        malformed_portable.join(CATALOG_PORTABLE_MANIFEST_FILE_NAME),
        b"truncated",
    )
    .unwrap();
    assert!(
        remove_exact_bundle(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &malformed_portable_digest,
            CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54,
        )
        .is_err()
    );
    assert!(malformed_portable.exists());

    let malformed_metadata_digest = resource_digest('f');
    let malformed_metadata = bundle_path(
        &catalog_root,
        RemiCatalogResourceKind::SourceSnapshot,
        "fedora-44",
        &malformed_metadata_digest,
    );
    retired_exact_bundle(
        &malformed_metadata,
        RemiCatalogResourceKind::SourceSnapshot,
        'f',
    );
    fs::write(
        malformed_metadata
            .join(SOURCE_METADATA_DIRECTORY_NAME)
            .join("unexpected"),
        b"evidence",
    )
    .unwrap();
    assert!(
        remove_exact_bundle(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &malformed_metadata_digest,
            CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54,
        )
        .is_err()
    );
    assert!(malformed_metadata.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let target = root.path().join("target");
        exact_bundle(&target, RemiCatalogResourceKind::SourceSnapshot, 'c');
        let linked_digest = resource_digest('c');
        let linked = bundle_path(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &linked_digest,
        );
        symlink(&target, &linked).unwrap();
        assert!(
            remove_exact_bundle(
                &catalog_root,
                RemiCatalogResourceKind::SourceSnapshot,
                "fedora-44",
                &linked_digest,
                CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54,
            )
            .is_err()
        );
        assert!(target.exists());

        let symlinked_proof_digest = resource_digest('1');
        let symlinked_proof = bundle_path(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &symlinked_proof_digest,
        );
        exact_bundle(
            &symlinked_proof,
            RemiCatalogResourceKind::SourceSnapshot,
            '1',
        );
        let portable_path = symlinked_proof.join(CATALOG_PORTABLE_MANIFEST_FILE_NAME);
        fs::remove_file(&portable_path).unwrap();
        symlink(root.path().join("missing-portable-proof"), &portable_path).unwrap();
        assert!(
            remove_exact_bundle(
                &catalog_root,
                RemiCatalogResourceKind::SourceSnapshot,
                "fedora-44",
                &symlinked_proof_digest,
                CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54,
            )
            .is_err()
        );
        assert!(symlinked_proof.exists());

        let redirected_root = root.path().join("redirected-catalogs");
        fs::create_dir(&redirected_root).unwrap();
        let redirected_digest = resource_digest('d');
        let redirected = redirected_root.join(&redirected_digest);
        exact_bundle(&redirected, RemiCatalogResourceKind::SourceSnapshot, 'd');
        let symlinked_catalog_root = root.path().join("symlinked-parent-catalogs");
        fs::create_dir(&symlinked_catalog_root).unwrap();
        symlink(&redirected_root, symlinked_catalog_root.join("sources")).unwrap();
        assert!(
            remove_exact_bundle(
                &symlinked_catalog_root,
                RemiCatalogResourceKind::SourceSnapshot,
                "fedora-44",
                &redirected_digest,
                CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54,
            )
            .is_err()
        );
        assert!(redirected.exists());
    }
}

#[test]
fn deletion_resumes_from_exact_gc_tombstone_after_rename() {
    let root = tempfile::tempdir().unwrap();
    let catalog_root = root.path().join("catalogs");
    fs::create_dir_all(catalog_root.join("sources")).unwrap();
    let resource_digest = resource_digest('d');
    let original = bundle_path(
        &catalog_root,
        RemiCatalogResourceKind::SourceSnapshot,
        "fedora-44",
        &resource_digest,
    );
    exact_bundle(&original, RemiCatalogResourceKind::SourceSnapshot, 'd');
    let tombstone_parent = ensure_tombstone_parent(
        &catalog_root,
        RemiCatalogResourceKind::SourceSnapshot,
        "fedora-44",
    )
    .unwrap();
    let tombstone = tombstone_path(
        &catalog_root,
        RemiCatalogResourceKind::SourceSnapshot,
        "fedora-44",
        &resource_digest,
    );
    fs::rename(&original, &tombstone).unwrap();
    File::open(tombstone_parent).unwrap().sync_all().unwrap();

    assert!(
        remove_exact_bundle(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &resource_digest,
            CatalogBundleDeletionPolicy::CurrentOnly,
        )
        .unwrap()
    );
    assert!(!original.exists());
    assert!(!tombstone.exists());
}

#[test]
fn absent_profile_namespace_is_idempotent_bundle_absence() {
    let root = tempfile::tempdir().unwrap();
    let catalog_root = root.path().join("catalogs");
    fs::create_dir_all(catalog_root.join("profiles")).unwrap();

    assert!(
        !remove_exact_bundle(
            &catalog_root,
            RemiCatalogResourceKind::ProfileRevision,
            "fedora-44",
            &digest('a'),
            CatalogBundleDeletionPolicy::CurrentOnly,
        )
        .unwrap()
    );
}

#[tokio::test]
async fn registered_unreachable_resources_are_journaled_removed_and_acknowledged() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("remi.db");
    let catalog_root = root.path().join("catalogs");
    fs::create_dir_all(catalog_root.join("sources")).unwrap();
    fs::create_dir_all(catalog_root.join("profiles/fedora-44")).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open_fast(&db_path).unwrap();
    for (byte, kind) in [
        ('a', RemiCatalogResourceKind::SourceSnapshot),
        ('b', RemiCatalogResourceKind::ProfileRevision),
    ] {
        let resource = RemiCatalogResource {
            resource_sha256: resource_digest(byte),
            kind,
            source_profile: "fedora-44".to_string(),
            artifact_sha256: digest(byte),
            artifact_size: 7,
            logical_digest_sha256: digest('d'),
            manifest_json: format!("{{\"resource\":\"{byte}\"}}"),
            physical_attestation: physical_attestation(7, byte),
            durable: true,
            created_at: 1,
        };
        resource.insert(&conn).unwrap();
        exact_bundle(
            &bundle_path(&catalog_root, kind, "fedora-44", &resource.resource_sha256),
            kind,
            byte,
        );
    }
    drop(conn);

    let database_writer = DatabaseWriter::default();
    let catalog_authority = CatalogAuthority::from_paths(
        db_path.clone(),
        catalog_root.clone(),
        database_writer.clone(),
    );
    let report = collect_catalog_garbage_uncoordinated(
        db_path.clone(),
        catalog_root,
        database_writer,
        catalog_authority,
    )
    .await
    .unwrap();
    assert_eq!(report.deleted_profile_resources, 1);
    assert_eq!(report.deleted_source_resources, 1);
    assert_eq!(report.removed_bundles, 2);
    assert_eq!(report.acknowledged_deletions, 2);

    let conn = conary_core::db::open_fast(&db_path).unwrap();
    assert!(
        plan_catalog_collection(&conn)
            .unwrap()
            .pending_deletions
            .is_empty()
    );
}

#[tokio::test]
async fn matching_terminal_candidate_cannot_bypass_pending_current_layout() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("remi.db");
    let catalog_root = root.path().join("catalogs");
    fs::create_dir_all(catalog_root.join("sources")).unwrap();
    fs::create_dir_all(catalog_root.join("profiles/fedora-44")).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let resource = RemiCatalogResource {
        resource_sha256: resource_digest('e'),
        kind: RemiCatalogResourceKind::ProfileRevision,
        source_profile: "fedora-44".to_string(),
        artifact_sha256: digest('e'),
        artifact_size: 7,
        logical_digest_sha256: digest('d'),
        manifest_json: "{\"resource\":\"e\"}".to_string(),
        physical_attestation: physical_attestation(7, 'e'),
        durable: true,
        created_at: 1,
    };
    let conn = conary_core::db::open_fast(&db_path).unwrap();
    resource.insert(&conn).unwrap();
    conn.execute(
        "INSERT INTO repository_sync_runs (
             run_id, source_profile, owner_instance_uuid, fencing_epoch,
             candidate_profile_digest, state, started_at, heartbeat_at,
             lease_expires_at, finished_at, failure_stage, failure_category,
             failure_evidence
         ) VALUES (?1, 'fedora-44', ?2, 1, ?3, 'abandoned', 1, 1, 1, 2,
                   'publishing', 'internal', 'injected collision')",
        rusqlite::params![
            "10000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000001",
            &resource.resource_sha256,
        ],
    )
    .unwrap();
    drop(conn);
    let path = bundle_path(
        &catalog_root,
        resource.kind,
        &resource.source_profile,
        &resource.resource_sha256,
    );
    retired_exact_bundle(&path, resource.kind, 'e');

    let database_writer = DatabaseWriter::default();
    let catalog_authority = CatalogAuthority::from_paths(
        db_path.clone(),
        catalog_root.clone(),
        database_writer.clone(),
    );
    let error = collect_catalog_garbage_uncoordinated(
        db_path.clone(),
        catalog_root,
        database_writer,
        catalog_authority,
    )
    .await
    .expect_err("registered schema-55 deletion requires its portable proof");
    assert!(
        error
            .to_string()
            .contains("does not have a permitted exact bundle layout")
    );
    assert!(path.exists());

    let conn = conary_core::db::open_fast(&db_path).unwrap();
    let pending = plan_catalog_collection(&conn).unwrap().pending_deletions;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].resource_sha256, resource.resource_sha256);
}

#[tokio::test]
async fn matching_terminal_candidate_cannot_remove_live_registered_resource() {
    let fixture = ActiveCatalogFixture::new();
    let profile_revision_sha256 = fixture.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1.fc44",
            Some("x86_64"),
            4,
            "bash-rpm",
        )],
    );
    let profile_path = fixture
        .catalog_dir()
        .join("profiles")
        .join("fedora-44")
        .join(&profile_revision_sha256);
    fs::remove_file(profile_path.join(CATALOG_PORTABLE_MANIFEST_FILE_NAME)).unwrap();
    let conn = conary_core::db::open_fast(fixture.db_path()).unwrap();
    conn.execute(
        "INSERT INTO repository_sync_runs (
             run_id, source_profile, owner_instance_uuid, fencing_epoch,
             candidate_profile_digest, state, started_at, heartbeat_at,
             lease_expires_at, finished_at, failure_stage, failure_category,
             failure_evidence
         ) VALUES (?1, 'fedora-44', ?2, 2, ?3, 'abandoned', 2, 2, 2, 3,
                   'publishing', 'internal', 'injected collision')",
        rusqlite::params![
            "20000000-0000-4000-8000-000000000002",
            "00000000-0000-4000-8000-000000000002",
            &profile_revision_sha256,
        ],
    )
    .unwrap();
    drop(conn);

    let report = collect_catalog_garbage_uncoordinated(
        fixture.db_path().to_path_buf(),
        fixture.catalog_dir().to_path_buf(),
        fixture.authority().database_writer_for_test(),
        fixture.authority().clone(),
    )
    .await
    .unwrap();
    assert_eq!(report.removed_bundles, 0);
    assert!(profile_path.exists());

    let conn = conary_core::db::open_fast(fixture.db_path()).unwrap();
    assert!(
        RemiCatalogResource::find_by_sha256(&conn, &profile_revision_sha256)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn removed_bundles_evict_caches_but_preserve_inflight_readers() {
    let fixture = ActiveCatalogFixture::new();
    let profile_revision_sha256 = fixture.register(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1.fc44",
            Some("x86_64"),
            4,
            "bash-rpm",
        )],
    );
    let pinned = fixture
        .authority()
        .open_unrooted_cached_profile_for_test(&ProfileRevisionSelection {
            source_profile: "fedora-44".to_string(),
            profile_revision_sha256: profile_revision_sha256.clone(),
        })
        .expect("open unrooted registered profile");
    let profile_path = fixture
        .catalog_dir()
        .join("profiles")
        .join("fedora-44")
        .join(&profile_revision_sha256);
    let profile_package = pinned
        .reader()
        .find_packages_by_name("bash")
        .expect("query profile package")
        .into_iter()
        .next()
        .expect("fixture profile package");
    let expected_source_resources = pinned.manifest().members.len();
    let source = fixture
        .authority()
        .open_source_catalog_for_package(&pinned, &profile_package)
        .expect("open source reader before collection");
    let source_snapshot_sha256 = source
        .manifest()
        .manifest_sha256()
        .expect("hash source manifest");
    let source_path = fixture
        .catalog_dir()
        .join("sources")
        .join(&source_snapshot_sha256);
    assert!(
        fixture
            .authority()
            .has_verified_source_reader_for_test("fedora-44", &source_snapshot_sha256,)
    );
    assert!(
        fixture
            .authority()
            .has_verified_profile_reader_for_test("fedora-44", &profile_revision_sha256,)
    );

    let report = collect_catalog_garbage_uncoordinated(
        fixture.db_path().to_path_buf(),
        fixture.catalog_dir().to_path_buf(),
        fixture.authority().database_writer_for_test(),
        fixture.authority().clone(),
    )
    .await
    .expect("collect unrooted registered catalogs");

    assert_eq!(report.deleted_source_resources, expected_source_resources);
    assert_eq!(report.deleted_profile_resources, 1);
    assert!(!source_path.exists());
    assert!(!profile_path.exists());
    assert!(
        !fixture
            .authority()
            .has_verified_source_reader_for_test("fedora-44", &source_snapshot_sha256,)
    );
    assert!(
        !fixture
            .authority()
            .has_verified_profile_reader_for_test("fedora-44", &profile_revision_sha256,)
    );
    assert_eq!(
        pinned
            .reader()
            .find_packages_by_name("bash")
            .expect("in-flight profile reader survives unlink")
            .len(),
        1
    );
    assert_eq!(
        source
            .reader()
            .find_packages_by_name("bash")
            .expect("in-flight source reader survives unlink")
            .len(),
        1
    );
}

#[tokio::test]
async fn shared_coordinator_serializes_concurrent_collectors() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("remi.db");
    let catalog_root = root.path().join("catalogs");
    fs::create_dir_all(catalog_root.join("sources")).unwrap();
    fs::create_dir_all(catalog_root.join("profiles/fedora-44")).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open_fast(&db_path).unwrap();
    let resource = RemiCatalogResource {
        resource_sha256: resource_digest('a'),
        kind: RemiCatalogResourceKind::SourceSnapshot,
        source_profile: "fedora-44".to_string(),
        artifact_sha256: digest('a'),
        artifact_size: 7,
        logical_digest_sha256: digest('d'),
        manifest_json: "{\"resource\":\"a\"}".to_string(),
        physical_attestation: physical_attestation(7, 'a'),
        durable: true,
        created_at: 1,
    };
    resource.insert(&conn).unwrap();
    exact_bundle(
        &bundle_path(
            &catalog_root,
            resource.kind,
            &resource.source_profile,
            &resource.resource_sha256,
        ),
        resource.kind,
        'a',
    );
    drop(conn);

    let coordinator = Arc::new(Mutex::new(()));
    let database_writer = DatabaseWriter::default();
    let catalog_authority = CatalogAuthority::from_paths(
        db_path.clone(),
        catalog_root.clone(),
        database_writer.clone(),
    );
    let first = collect_catalog_garbage_serialized(
        Arc::clone(&coordinator),
        db_path.clone(),
        catalog_root.clone(),
        database_writer.clone(),
        catalog_authority.clone(),
    );
    let second = collect_catalog_garbage_serialized(
        coordinator,
        db_path.clone(),
        catalog_root,
        database_writer,
        catalog_authority,
    );
    let (first, second) = tokio::join!(first, second);
    let reports = [first.unwrap(), second.unwrap()];
    assert_eq!(
        reports
            .iter()
            .map(|report| report.acknowledged_deletions)
            .sum::<usize>(),
        1
    );
    assert_eq!(
        reports
            .iter()
            .map(|report| report.removed_bundles)
            .sum::<usize>(),
        1
    );

    let conn = conary_core::db::open_fast(&db_path).unwrap();
    assert!(
        plan_catalog_collection(&conn)
            .unwrap()
            .pending_deletions
            .is_empty()
    );
}

#[tokio::test]
async fn terminal_run_journal_removes_exact_retired_unregistered_publication() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("remi.db");
    let catalog_root = root.path().join("catalogs");
    fs::create_dir_all(catalog_root.join("sources")).unwrap();
    fs::create_dir_all(catalog_root.join("profiles/fedora-44")).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let profile_digest = resource_digest('a');
    let source_digest = resource_digest('b');
    let current_profile_digest = resource_digest('c');
    let conn = conary_core::db::open_fast(&db_path).unwrap();
    let repository_id = conn
        .query_row(
            "INSERT INTO repositories(name, url, source_profile)
             VALUES ('fixture', 'https://fixture.test', 'fedora-44')
             RETURNING id",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO repository_sync_runs (
             run_id, source_profile, owner_instance_uuid, fencing_epoch,
             candidate_profile_digest, state, started_at, heartbeat_at,
             lease_expires_at, finished_at, failure_stage, failure_category,
             failure_evidence
         ) VALUES (?1, 'fedora-44', ?2, 1, ?3, 'abandoned', 1, 1, 1, 2,
                   'publishing', 'internal', 'injected crash after rename')",
        rusqlite::params![
            "10000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000001",
            &profile_digest,
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repository_sync_run_members (
             run_id, ordinal, repository_id, source_identity,
             repository_identity, stream_kind, stream_identity, role,
             precedence, required, candidate_source_snapshot_sha256
         ) VALUES (?1, 0, ?2, 'fixture-source', 'fixture-repository',
                   'release', '44', 'base', 0, 1, ?3)",
        rusqlite::params![
            "10000000-0000-4000-8000-000000000001",
            repository_id,
            &source_digest,
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repository_sync_runs (
             run_id, source_profile, owner_instance_uuid, fencing_epoch,
             candidate_profile_digest, state, started_at, heartbeat_at,
             lease_expires_at, finished_at, failure_stage, failure_category,
             failure_evidence
         ) VALUES (?1, 'fedora-44', ?2, 2, ?3, 'abandoned', 2, 2, 2, 3,
                   'publishing', 'internal', 'injected current candidate')",
        rusqlite::params![
            "20000000-0000-4000-8000-000000000002",
            "00000000-0000-4000-8000-000000000002",
            &current_profile_digest,
        ],
    )
    .unwrap();
    drop(conn);
    let profile_path = bundle_path(
        &catalog_root,
        RemiCatalogResourceKind::ProfileRevision,
        "fedora-44",
        &profile_digest,
    );
    let source_path = bundle_path(
        &catalog_root,
        RemiCatalogResourceKind::SourceSnapshot,
        "fedora-44",
        &source_digest,
    );
    let current_profile_path = bundle_path(
        &catalog_root,
        RemiCatalogResourceKind::ProfileRevision,
        "fedora-44",
        &current_profile_digest,
    );
    retired_exact_bundle(&profile_path, RemiCatalogResourceKind::ProfileRevision, 'a');
    retired_exact_bundle(&source_path, RemiCatalogResourceKind::SourceSnapshot, 'b');
    exact_bundle(
        &current_profile_path,
        RemiCatalogResourceKind::ProfileRevision,
        'c',
    );

    let database_writer = DatabaseWriter::default();
    let catalog_authority = CatalogAuthority::from_paths(
        db_path.clone(),
        catalog_root.clone(),
        database_writer.clone(),
    );
    let report = collect_catalog_garbage_uncoordinated(
        db_path.clone(),
        catalog_root.clone(),
        database_writer.clone(),
        catalog_authority.clone(),
    )
    .await
    .unwrap();
    assert_eq!(report.removed_bundles, 3);
    assert!(!profile_path.exists());
    assert!(!source_path.exists());
    assert!(!current_profile_path.exists());

    let replay = collect_catalog_garbage_uncoordinated(
        db_path,
        catalog_root,
        database_writer,
        catalog_authority,
    )
    .await
    .unwrap();
    assert_eq!(replay.removed_bundles, 0);
    assert_eq!(replay.acknowledged_deletions, 0);
}
