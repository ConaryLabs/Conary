// conary-core/src/db/generation_delta/tests.rs

use super::*;
use rusqlite::session::ConflictAction;

fn create_fixture(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE authority (
                     id INTEGER PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 INSERT INTO authority (id, value) VALUES (1, 'base');",
        )
        .unwrap();
    connection
}

fn create_epoch_fixture(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    configure_mutation_epoch(&connection).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE generation_db_mutation_epoch (
                     singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                     token TEXT NOT NULL CHECK(length(token) = 36)
                 );
                 INSERT INTO generation_db_mutation_epoch (singleton, token)
                 VALUES (1, '00000000-0000-0000-0000-000000000000');
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

#[test]
fn session_delta_replays_exact_insert_update_and_delete() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source.sqlite");
    let base_path = temp.path().join("base.sqlite");
    let source = create_epoch_fixture(&source_path);
    let base = create_epoch_fixture(&base_path);
    let recorder = GenerationDbDeltaRecorder::begin(&source, &source_path).unwrap();

    source
        .execute_batch(
            "UPDATE authority SET value = 'updated' WHERE id = 1;
                 INSERT INTO authority (id, value) VALUES (2, 'inserted');
                 INSERT INTO authority (id, value) VALUES (3, 'deleted');
                 DELETE FROM authority WHERE id = 3;",
        )
        .unwrap();

    let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
        panic!("same-connection mutations unexpectedly required a full snapshot");
    };
    assert!(!delta.bytes().is_empty());
    assert_eq!(delta.sha256(), crate::hash::sha256(delta.bytes()));

    let mut input = delta.bytes();
    base.apply_strm(&mut input, None::<fn(&str) -> bool>, |_conflict, _item| {
        ConflictAction::SQLITE_CHANGESET_ABORT
    })
    .unwrap();
    base.execute(
        "UPDATE generation_db_mutation_epoch SET token = ?1 WHERE singleton = 1",
        [&delta.result_baseline().mutation_token],
    )
    .unwrap();
    let rows = base
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
}

#[test]
fn another_connection_commit_forces_typed_full_snapshot_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source.sqlite");
    let source = create_epoch_fixture(&source_path);
    let writer = Connection::open(&source_path).unwrap();
    configure_mutation_epoch(&writer).unwrap();
    let recorder = GenerationDbDeltaRecorder::begin(&source, &source_path).unwrap();

    writer
        .execute(
            "INSERT INTO authority (id, value) VALUES (2, 'other connection')",
            [],
        )
        .unwrap();

    assert_eq!(
        recorder.finish().unwrap(),
        GenerationDbDeltaCapture::Fallback(
            GenerationDbDeltaFallbackReason::ConcurrentConnectionWrite
        )
    );
}

#[test]
fn commit_while_the_recorder_is_arming_forces_full_snapshot_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source.sqlite");
    let source = create_epoch_fixture(&source_path);
    let writer = Connection::open(&source_path).unwrap();
    configure_mutation_epoch(&writer).unwrap();
    let recorder = GenerationDbDeltaRecorder::begin_with_arm_hook(&source, &source_path, || {
        writer.execute(
            "INSERT INTO authority (id, value) VALUES (2, 'arming window')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        recorder.finish().unwrap(),
        GenerationDbDeltaCapture::Fallback(
            GenerationDbDeltaFallbackReason::ConcurrentConnectionWrite
        )
    );
}

#[test]
fn mutation_token_admits_only_the_exact_prior_source_state() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source.sqlite");
    let source = create_epoch_fixture(&source_path);
    let baseline = read_baseline_token(&source).unwrap();
    let recorder =
        GenerationDbDeltaRecorder::begin_against(&source, &source_path, baseline.clone()).unwrap();
    source
        .execute(
            "INSERT INTO authority (id, value) VALUES (2, 'same connection')",
            [],
        )
        .unwrap();

    let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
        panic!("matching mutation token unexpectedly required a full snapshot");
    };
    assert_ne!(delta.result_baseline(), &baseline);
    assert_eq!(
        delta.result_baseline(),
        &read_baseline_token(&source).unwrap()
    );
}

#[test]
fn cumulative_capture_excludes_internal_epoch_and_tracks_latest_token() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source.sqlite");
    let base_path = temp.path().join("base.sqlite");
    let source = create_epoch_fixture(&source_path);
    let base = create_epoch_fixture(&base_path);
    let baseline = read_baseline_token(&source).unwrap();
    let mut recorder =
        GenerationDbDeltaRecorder::begin_against(&source, &source_path, baseline).unwrap();

    source
        .execute("INSERT INTO authority (id, value) VALUES (2, 'first')", [])
        .unwrap();
    let GenerationDbDeltaCapture::Captured(first) = recorder.capture().unwrap() else {
        panic!("first cumulative capture unexpectedly required a full snapshot");
    };
    source
        .execute("INSERT INTO authority (id, value) VALUES (3, 'second')", [])
        .unwrap();
    let GenerationDbDeltaCapture::Captured(second) = recorder.capture().unwrap() else {
        panic!("second cumulative capture unexpectedly required a full snapshot");
    };

    assert!(second.payload_bytes() > first.payload_bytes());
    let mut input = second.bytes();
    base.apply_strm(&mut input, None::<fn(&str) -> bool>, |_conflict, _item| {
        ConflictAction::SQLITE_CHANGESET_ABORT
    })
    .unwrap();
    base.execute(
        "UPDATE generation_db_mutation_epoch SET token = ?1 WHERE singleton = 1",
        [&second.result_baseline().mutation_token],
    )
    .unwrap();

    assert_eq!(
        base.query_row("SELECT COUNT(*) FROM authority", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        read_baseline_token(&base).unwrap(),
        *second.result_baseline()
    );
}

#[test]
fn committed_source_change_rejects_the_prior_baseline_token() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source.sqlite");
    let source = create_epoch_fixture(&source_path);
    let baseline = read_baseline_token(&source).unwrap();
    source
        .execute(
            "INSERT INTO authority (id, value) VALUES (2, 'committed divergence')",
            [],
        )
        .unwrap();
    let recorder =
        GenerationDbDeltaRecorder::begin_against(&source, &source_path, baseline).unwrap();

    assert_eq!(
        recorder.finish().unwrap(),
        GenerationDbDeltaCapture::Fallback(GenerationDbDeltaFallbackReason::SourceBaselineChanged)
    );
}

#[test]
fn one_transaction_uses_one_mutation_token_for_many_rows() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source.sqlite");
    let source = create_epoch_fixture(&source_path);
    let before = read_baseline_token(&source).unwrap();
    let tx = source.unchecked_transaction().unwrap();
    tx.execute("INSERT INTO authority (id, value) VALUES (2, 'first')", [])
        .unwrap();
    let after_first = read_baseline_token(&tx).unwrap();
    tx.execute("INSERT INTO authority (id, value) VALUES (3, 'second')", [])
        .unwrap();
    let after_second = read_baseline_token(&tx).unwrap();
    tx.commit().unwrap();

    assert_ne!(after_first, before);
    assert_eq!(after_second, after_first);
    assert_eq!(read_baseline_token(&source).unwrap(), after_first);
}

#[test]
fn another_configured_connection_advances_the_persisted_token() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source.sqlite");
    let source = create_epoch_fixture(&source_path);
    let before = read_baseline_token(&source).unwrap();
    let writer = Connection::open(&source_path).unwrap();
    configure_mutation_epoch(&writer).unwrap();

    writer
        .execute(
            "INSERT INTO authority (id, value) VALUES (2, 'other connection')",
            [],
        )
        .unwrap();

    assert_ne!(read_baseline_token(&source).unwrap(), before);
}

#[test]
fn unconfigured_writer_cannot_bypass_the_mutation_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source.sqlite");
    let _source = create_epoch_fixture(&source_path);
    let writer = Connection::open(&source_path).unwrap();

    let error = writer
        .execute(
            "INSERT INTO authority (id, value) VALUES (2, 'untracked write')",
            [],
        )
        .unwrap_err();

    assert!(error.to_string().contains(MUTATION_TOKEN_FUNCTION));
}

#[test]
fn source_connection_must_name_the_exact_database_path() {
    let temp = tempfile::tempdir().unwrap();
    let source_path = temp.path().join("source.sqlite");
    let other_path = temp.path().join("other.sqlite");
    let source = create_fixture(&source_path);
    create_fixture(&other_path);

    let error = match GenerationDbDeltaRecorder::begin(&source, &other_path) {
        Ok(_) => panic!("mismatched source path unexpectedly admitted"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("delta source mismatch"));
}

#[test]
fn every_current_schema_table_is_session_compatible() {
    let (_database, connection) = crate::db::testing::create_test_db();
    let mut statement = connection
        .prepare(
            "SELECT m.name
                 FROM sqlite_schema AS m
                 WHERE m.type = 'table'
                   AND m.name NOT LIKE 'sqlite_%'
                   AND NOT EXISTS (
                       SELECT 1 FROM pragma_table_info(m.name) WHERE pk > 0
                   )
                 ORDER BY m.name",
        )
        .unwrap();
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        tables.is_empty(),
        "SQLite session changesets require a declared primary key: {tables:?}"
    );

    let table_count: i64 = connection
        .query_row(
            "SELECT COUNT(*)
                 FROM sqlite_schema
                 WHERE type = 'table'
                   AND name NOT LIKE 'sqlite_%'
                   AND name != ?1",
            [MUTATION_EPOCH_TABLE],
            |row| row.get(0),
        )
        .unwrap();
    let trigger_count: i64 = connection
        .query_row(
            "SELECT COUNT(*)
                 FROM sqlite_schema
                 WHERE type = 'trigger'
                   AND name LIKE 'conary_generation_mutation_epoch_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trigger_count, table_count * 3);
}
