// apps/remi/src/server/r2_durability/tests.rs

use super::*;
use conary_core::ccs::{CcsTransportEnvelopeV1, CcsTransportObjectV1};
use conary_core::db::models::{
    ConvertedPackage, EMPTY_REPOSITORY_PROVIDES_DIGEST, RemiCatalogResource,
    RemiCatalogResourceKind,
};
use std::sync::Mutex;

#[derive(Default)]
struct FakeStore {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    puts: Mutex<Vec<String>>,
    discard_puts: bool,
}

#[async_trait]
impl DurableChunkStore for FakeStore {
    async fn list_chunk_objects(&self) -> Result<Vec<R2ChunkObject>> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .iter()
            .map(|(hash, data)| R2ChunkObject {
                hash: hash.clone(),
                size_bytes: data.len() as u64,
            })
            .collect())
    }

    async fn put_chunk(&self, hash: &str, data: &[u8]) -> Result<()> {
        self.puts.lock().unwrap().push(hash.to_string());
        if !self.discard_puts {
            self.objects
                .lock()
                .unwrap()
                .insert(hash.to_string(), data.to_vec());
        }
        Ok(())
    }
}

fn seed_profile_resource(conn: &Connection, source_profile: &str) -> String {
    let manifest_json = format!(r#"{{"profile":"{source_profile}"}}"#);
    let revision = conary_core::hash::sha256(manifest_json.as_bytes());
    RemiCatalogResource {
        resource_sha256: revision.clone(),
        kind: RemiCatalogResourceKind::ProfileRevision,
        source_profile: source_profile.to_string(),
        artifact_sha256: conary_core::hash::sha256(format!("artifact-{revision}").as_bytes()),
        artifact_size: 1,
        logical_digest_sha256: conary_core::hash::sha256(format!("logical-{revision}").as_bytes()),
        manifest_json,
        physical_attestation:
            crate::server::catalog_authority::test_support::physical_attestation_for_test(
                1,
                revision.as_bytes(),
            ),
        durable: true,
        created_at: 1,
    }
    .insert(conn)
    .unwrap();
    revision
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, String, Vec<u8>) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("remi.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    let data = b"durable chunk".to_vec();
    let hash = conary_core::hash::sha256(&data);
    let transport = CcsTransportEnvelopeV1 {
        schema_version: conary_core::ccs::transport::CCS_TRANSPORT_SCHEMA_V1,
        manifest_base64: String::new(),
        signature_json: "{}".to_string(),
        debug_toml_base64: None,
        build_attestation_json: None,
        foreign_conversion_boundary_json: None,
        objects: vec![CcsTransportObjectV1 {
            sha256: hash.clone(),
            size: data.len() as u64,
        }],
    };
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        seed_profile_resource(&conn, "fedora-44"),
        "durability-fixture".to_string(),
        "1-1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:fixture".to_string(),
        &transport,
        data.len() as i64,
        "sha256:transport".to_string(),
        "/tmp/fixture.ccs".to_string(),
        EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    converted.insert_with_conversion_pin(&conn, 1).unwrap();

    let objects_dir = temp.path().join("chunks/objects");
    let path = chunk_path(&objects_dir, &hash);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, &data).unwrap();
    (temp, db_path, objects_dir, hash, data)
}

#[tokio::test]
async fn plan_reports_exact_gap_without_writing() {
    let (_temp, db_path, objects_dir, hash, data) = fixture();
    let store = Arc::new(FakeStore::default());

    let report = run_r2_durability(
        &db_path,
        &objects_dir,
        Arc::clone(&store),
        R2DurabilityMode::Plan,
        2,
    )
    .await
    .unwrap();

    assert_eq!(report.outcome, R2DurabilityOutcome::PlanBlocked);
    assert_eq!(report.planned_uploads, 1);
    assert_eq!(report.planned_upload_bytes, data.len() as u64);
    assert_eq!(report.missing_from_both, 0);
    assert!(store.puts.lock().unwrap().is_empty());
    assert_eq!(report.local.required_present, 1);
    assert_eq!(report.r2.required_missing, 1);
    assert_eq!(report.missing_from_both_samples, Vec::<String>::new());
    assert_eq!(hash.len(), 64);
}

#[tokio::test]
async fn plan_rejects_current_conversion_without_exact_pin() {
    let (_temp, db_path, objects_dir, _hash, _data) = fixture();
    let conn = crate::server::open_runtime_db(&db_path).unwrap();
    let id: i64 = conn
        .query_row(
            "SELECT id FROM converted_packages WHERE package_name = 'durability-fixture'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "DELETE FROM remi_profile_revision_pins WHERE pin_id = ?1",
        [ConvertedPackage::conversion_pin_id(id)],
    )
    .unwrap();
    drop(conn);

    let error = format!(
        "{:#}",
        run_r2_durability(
            &db_path,
            &objects_dir,
            Arc::new(FakeStore::default()),
            R2DurabilityMode::Plan,
            2,
        )
        .await
        .unwrap_err()
    );
    assert!(
        error.contains("has no exact profile-revision pin"),
        "{error}"
    );
}

#[tokio::test]
async fn plan_does_not_backfill_unreferenced_local_objects() {
    let (_temp, db_path, objects_dir, _hash, _data) = fixture();
    let unreferenced_data = b"unreferenced cache object";
    let unreferenced_hash = conary_core::hash::sha256(unreferenced_data);
    let unreferenced_path = chunk_path(&objects_dir, &unreferenced_hash);
    std::fs::create_dir_all(unreferenced_path.parent().unwrap()).unwrap();
    std::fs::write(unreferenced_path, unreferenced_data).unwrap();
    let store = Arc::new(FakeStore::default());

    let report = run_r2_durability(&db_path, &objects_dir, store, R2DurabilityMode::Plan, 2)
        .await
        .unwrap();

    assert_eq!(report.local.objects, 2);
    assert_eq!(report.required_objects, 1);
    assert_eq!(report.planned_uploads, 1);
}

#[tokio::test]
async fn apply_uploads_verified_bytes_and_rechecks_r2() {
    let (_temp, db_path, objects_dir, hash, data) = fixture();
    let store = Arc::new(FakeStore::default());

    let report = run_r2_durability(
        &db_path,
        &objects_dir,
        Arc::clone(&store),
        R2DurabilityMode::Apply,
        2,
    )
    .await
    .unwrap();

    assert_eq!(report.outcome, R2DurabilityOutcome::AppliedComplete);
    assert!(report.r2_complete);
    assert_eq!(report.attempted_uploads, 1);
    assert_eq!(report.uploaded_objects, 1);
    assert_eq!(report.uploaded_bytes, data.len() as u64);
    assert_eq!(store.puts.lock().unwrap().as_slice(), &[hash]);
    assert_eq!(report.r2.required_missing, 0);
}

#[tokio::test]
async fn apply_refuses_corrupt_local_object() {
    let (_temp, db_path, objects_dir, hash, _data) = fixture();
    std::fs::write(chunk_path(&objects_dir, &hash), b"corrupt chunk").unwrap();
    let store = Arc::new(FakeStore::default());

    let report = run_r2_durability(
        &db_path,
        &objects_dir,
        Arc::clone(&store),
        R2DurabilityMode::Apply,
        2,
    )
    .await
    .unwrap();

    assert_eq!(report.outcome, R2DurabilityOutcome::AppliedIncomplete);
    assert!(!report.r2_complete);
    assert_eq!(report.failed_uploads, 1);
    assert_eq!(report.uploaded_objects, 0);
    assert!(report.failure_samples[0].error.contains("digest"));
    assert!(store.puts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn apply_repairs_r2_size_disagreement_from_verified_local_bytes() {
    let (_temp, db_path, objects_dir, hash, data) = fixture();
    let store = Arc::new(FakeStore::default());
    store
        .objects
        .lock()
        .unwrap()
        .insert(hash.clone(), b"wrong-size".to_vec());

    let report = run_r2_durability(
        &db_path,
        &objects_dir,
        Arc::clone(&store),
        R2DurabilityMode::Apply,
        2,
    )
    .await
    .unwrap();

    assert_eq!(report.outcome, R2DurabilityOutcome::AppliedComplete);
    assert_eq!(report.planned_uploads, 1);
    assert_eq!(report.uploaded_objects, 1);
    assert_eq!(store.objects.lock().unwrap().get(&hash), Some(&data));
}

#[tokio::test]
async fn apply_does_not_trust_successful_put_without_post_inventory() {
    let (_temp, db_path, objects_dir, _hash, _data) = fixture();
    let store = Arc::new(FakeStore {
        discard_puts: true,
        ..Default::default()
    });

    let report = run_r2_durability(&db_path, &objects_dir, store, R2DurabilityMode::Apply, 2)
        .await
        .unwrap();

    assert_eq!(report.uploaded_objects, 1);
    assert_eq!(report.failed_uploads, 0);
    assert_eq!(report.outcome, R2DurabilityOutcome::AppliedIncomplete);
    assert!(!report.r2_complete);
}

#[tokio::test]
async fn plan_reports_required_object_missing_from_both_stores() {
    let (_temp, db_path, objects_dir, hash, _data) = fixture();
    std::fs::remove_file(chunk_path(&objects_dir, &hash)).unwrap();
    let store = Arc::new(FakeStore::default());

    let report = run_r2_durability(&db_path, &objects_dir, store, R2DurabilityMode::Plan, 2)
        .await
        .unwrap();

    assert_eq!(report.outcome, R2DurabilityOutcome::PlanBlocked);
    assert_eq!(report.planned_uploads, 0);
    assert_eq!(report.unrepairable_objects, 1);
    assert_eq!(
        report.unrepairable_samples[0].kind,
        R2DurabilityBlockerKind::MissingFromBoth
    );
    assert_eq!(report.missing_from_both, 1);
    assert_eq!(report.missing_from_both_samples, vec![hash]);
}

#[tokio::test]
async fn plan_explains_required_local_size_mismatch() {
    let (_temp, db_path, objects_dir, hash, _data) = fixture();
    std::fs::write(chunk_path(&objects_dir, &hash), b"short").unwrap();
    let store = Arc::new(FakeStore::default());

    let report = run_r2_durability(&db_path, &objects_dir, store, R2DurabilityMode::Plan, 2)
        .await
        .unwrap();

    assert_eq!(report.planned_uploads, 0);
    assert_eq!(report.unrepairable_objects, 1);
    assert_eq!(
        report.unrepairable_samples[0].kind,
        R2DurabilityBlockerKind::LocalSizeMismatch
    );
    assert_eq!(report.unrepairable_samples[0].local_size, Some(5));
}
