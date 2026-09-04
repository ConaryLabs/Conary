// crates/conary-core/src/db/generation_backup_chain/tests.rs

use super::*;
use crate::db::generation_delta::{
    GenerationDbDeltaCapture, GenerationDbDeltaRecorder, create_mutation_epoch_triggers,
    read_baseline_token,
};

fn fixture(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    crate::db::generation_delta::configure_mutation_epoch(&connection).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE generation_db_mutation_epoch (
                     singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                     token TEXT NOT NULL CHECK(length(token) = 36)
                 );
                 INSERT INTO generation_db_mutation_epoch (singleton, token)
                 VALUES (1, '00000000-0000-0000-0000-000000000000');
                 CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
                 INSERT INTO schema_version (version) VALUES (39);
                 CREATE TABLE authority (
                     id INTEGER PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 INSERT INTO authority (id, value) VALUES (1, 'base');",
        )
        .unwrap();
    create_mutation_epoch_triggers(&connection).unwrap();
    connection
}

fn identity(number: i64) -> GenerationDbBackupIdentity {
    GenerationDbBackupIdentity {
        generation_number: number,
        state_number: number,
        transaction_high_water_mark: None,
    }
}

#[test]
fn first_generation_zero_is_a_valid_chain_identity() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("live.sqlite");
    let state_dir = temp.path().join("generation-0-state");
    let source = fixture(&db_path);

    let record = create_generation_db_chain_base(
        &source,
        &db_path,
        &state_dir,
        "fixture-v1",
        39,
        identity(0),
    )
    .unwrap();

    assert_eq!(record.manifest.generation_number, 0);
    assert_eq!(record.manifest.state_number, 0);
    validate_manifest_structure(&record.manifest).unwrap();
}

#[test]
fn delta_generation_is_self_contained_and_reconstructs_exact_rows() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("live.sqlite");
    let base_state = temp.path().join("generation-1-state");
    let delta_state = temp.path().join("generation-2-state");
    let recovered = temp.path().join("recovered.sqlite");
    let source = fixture(&db_path);
    let base = create_generation_db_chain_base(
        &source,
        &db_path,
        &base_state,
        "fixture-v1",
        39,
        identity(1),
    )
    .unwrap();
    let mut recorder = GenerationDbDeltaRecorder::begin_against(
        &source,
        &db_path,
        base.manifest.final_baseline.clone(),
    )
    .unwrap();
    source
        .execute_batch(
            "UPDATE authority SET value = 'updated' WHERE id = 1;
                 INSERT INTO authority (id, value) VALUES (2, 'inserted');",
        )
        .unwrap();
    let GenerationDbDeltaCapture::Captured(delta) = recorder.capture().unwrap() else {
        panic!("exact base unexpectedly required a full snapshot");
    };
    let GenerationDbChainAppend::Appended(appended) = append_generation_db_chain_delta(
        &base_state,
        &delta_state,
        &delta,
        "fixture-v1",
        39,
        identity(2),
    )
    .unwrap() else {
        panic!("short exact chain unexpectedly required a new base");
    };
    assert_eq!(
        appended.metrics.payload_bytes_written,
        delta.payload_bytes()
    );
    assert_eq!(appended.metrics.linked_artifacts, 1);
    assert_eq!(appended.manifest.deltas.len(), 1);

    std::fs::remove_dir_all(&base_state).unwrap();
    materialize_and_verify_generation_db_chain(&delta_state, &recovered).unwrap();
    let recovered = Connection::open(recovered).unwrap();
    let rows = recovered
        .prepare("SELECT id, value FROM authority ORDER BY id")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![(1, "updated".to_string()), (2, "inserted".to_string())]
    );
    assert_eq!(
        read_baseline_token(&recovered).unwrap(),
        appended.manifest.final_baseline
    );
}

#[test]
fn manifest_and_delta_tampering_fail_before_replay() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("live.sqlite");
    let base_state = temp.path().join("generation-1-state");
    let delta_state = temp.path().join("generation-2-state");
    let source = fixture(&db_path);
    let base = create_generation_db_chain_base(
        &source,
        &db_path,
        &base_state,
        "fixture-v1",
        39,
        identity(1),
    )
    .unwrap();
    let recorder =
        GenerationDbDeltaRecorder::begin_against(&source, &db_path, base.manifest.final_baseline)
            .unwrap();
    source
        .execute("INSERT INTO authority (id, value) VALUES (2, 'delta')", [])
        .unwrap();
    let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
        panic!("exact base unexpectedly required a full snapshot");
    };
    let GenerationDbChainAppend::Appended(record) = append_generation_db_chain_delta(
        &base_state,
        &delta_state,
        &delta,
        "fixture-v1",
        39,
        identity(2),
    )
    .unwrap() else {
        panic!("short exact chain unexpectedly required a new base");
    };
    std::fs::write(
        delta_state.join(&record.manifest.deltas[0].file),
        b"tampered",
    )
    .unwrap();

    let error = materialize_and_verify_generation_db_chain(
        &delta_state,
        temp.path().join("recovered.sqlite"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("size mismatch"));
}

#[test]
fn manifest_rejects_a_delta_baseline_gap_even_with_a_matching_chain_digest() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("live.sqlite");
    let base_state = temp.path().join("generation-1-state");
    let delta_state = temp.path().join("generation-2-state");
    let source = fixture(&db_path);
    let base = create_generation_db_chain_base(
        &source,
        &db_path,
        &base_state,
        "fixture-v1",
        39,
        identity(1),
    )
    .unwrap();
    let recorder =
        GenerationDbDeltaRecorder::begin_against(&source, &db_path, base.manifest.final_baseline)
            .unwrap();
    source
        .execute("INSERT INTO authority (id, value) VALUES (2, 'delta')", [])
        .unwrap();
    let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
        panic!("exact base unexpectedly required a full snapshot");
    };
    let GenerationDbChainAppend::Appended(record) = append_generation_db_chain_delta(
        &base_state,
        &delta_state,
        &delta,
        "fixture-v1",
        39,
        identity(2),
    )
    .unwrap() else {
        panic!("short exact chain unexpectedly required a new base");
    };
    let mut manifest = record.manifest;
    manifest.deltas[0].source_baseline = GenerationDbBaselineToken {
        mutation_token: uuid::Uuid::new_v4().to_string(),
    };
    manifest.chain_sha256 = calculate_chain_sha256(&manifest.base, &manifest.deltas);

    let error = validate_manifest_structure(&manifest).unwrap_err();
    assert!(error.to_string().contains("does not continue"));
}

#[test]
fn current_schema_triggers_admit_exact_chain_replay() {
    let temp = tempfile::tempdir().unwrap();
    let (database, source) = crate::db::testing::create_test_db();
    let base_state = temp.path().join("generation-1-state");
    let delta_state = temp.path().join("generation-2-state");
    let recovered_path = temp.path().join("recovered.sqlite");
    let base = create_generation_db_chain_base(
        &source,
        database.path(),
        &base_state,
        crate::db::schema::SCHEMA_EPOCH,
        crate::db::schema::SCHEMA_VERSION,
        identity(1),
    )
    .unwrap();
    let recorder = GenerationDbDeltaRecorder::begin_against(
        &source,
        database.path(),
        base.manifest.final_baseline,
    )
    .unwrap();
    source
        .execute(
            "UPDATE schema_identity SET created_at = 'chain-replay-fixture'",
            [],
        )
        .unwrap();
    let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
        panic!("current-schema mutation unexpectedly required a full snapshot");
    };
    let GenerationDbChainAppend::Appended(_) = append_generation_db_chain_delta(
        &base_state,
        &delta_state,
        &delta,
        crate::db::schema::SCHEMA_EPOCH,
        crate::db::schema::SCHEMA_VERSION,
        identity(2),
    )
    .unwrap() else {
        panic!("current-schema chain unexpectedly required a new base");
    };

    materialize_and_verify_generation_db_chain(&delta_state, &recovered_path).unwrap();
    let recovered = Connection::open(recovered_path).unwrap();
    assert_eq!(
        recovered
            .query_row("SELECT created_at FROM schema_identity", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "chain-replay-fixture"
    );
}

#[test]
fn thirty_third_delta_forces_a_new_base_before_any_link_work() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("live.sqlite");
    let base_state = temp.path().join("generation-1-state");
    let next_state = temp.path().join("generation-34-state");
    let source = fixture(&db_path);
    let mut manifest = create_generation_db_chain_base(
        &source,
        &db_path,
        &base_state,
        "fixture-v1",
        39,
        identity(1),
    )
    .unwrap()
    .manifest;
    let mut source_baseline = manifest.base.result_baseline.clone();
    for index in 0..MAX_GENERATION_DB_DELTAS {
        let result_baseline = GenerationDbBaselineToken {
            mutation_token: uuid::Uuid::new_v4().to_string(),
        };
        let bytes = format!("delta-{index}").into_bytes();
        let sha256 = crate::hash::sha256(&bytes);
        manifest.deltas.push(GenerationDbDeltaArtifact {
            file: format!("conary.db.delta-{:03}-{sha256}.changeset", index + 1),
            sha256,
            payload_bytes: bytes.len() as u64,
            source_baseline,
            result_baseline: result_baseline.clone(),
        });
        source_baseline = result_baseline;
    }
    manifest.final_baseline = source_baseline.clone();
    manifest.chain_sha256 = calculate_chain_sha256(&manifest.base, &manifest.deltas);
    persist_manifest(&base_state, &manifest).unwrap();
    let next = GenerationDbDelta::from_bytes(
        b"next-delta".to_vec(),
        source_baseline,
        GenerationDbBaselineToken {
            mutation_token: uuid::Uuid::new_v4().to_string(),
        },
    );

    assert_eq!(
        append_generation_db_chain_delta(
            &base_state,
            &next_state,
            &next,
            "fixture-v1",
            39,
            identity(34),
        )
        .unwrap(),
        GenerationDbChainAppend::NeedsBase(GenerationDbChainFallbackReason::ChainLimitReached)
    );
    assert!(!next_state.exists());
}
