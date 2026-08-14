// conary-core/benches/generation_db_snapshot.rs

//! Generation database snapshot scaling against accumulated SQLite history.

use conary_core::db::generation_backup_chain::{
    GenerationDbBackupIdentity, GenerationDbChainAppend, append_generation_db_chain_delta,
    create_generation_db_chain_base,
};
use conary_core::db::generation_delta::{GenerationDbDeltaCapture, GenerationDbDeltaRecorder};
use conary_core::db::generation_snapshot::GenerationDbSnapshotProvider;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::hint::black_box;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Duration;

const HISTORY_ROWS: [usize; 3] = [0, 10_000, 100_000];

fn history_database(rows: usize) -> (tempfile::TempDir, std::path::PathBuf, Connection, i64) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary-history.sqlite");
    conary_core::db::init(&db_path).unwrap();
    let mut conn = conary_core::db::open(&db_path).unwrap();
    let transaction = conn.transaction().unwrap();
    {
        let mut insert = transaction
            .prepare(
                "INSERT INTO changesets (description, status, metadata)
                 VALUES (?1, 'applied', ?2)",
            )
            .unwrap();
        for row in 0..rows {
            let summary = format!("generation history row {row:08}");
            let authority = format!("{row:064x}{:064x}", rows.saturating_sub(row));
            insert.execute((summary, authority)).unwrap();
        }
    }
    transaction.commit().unwrap();
    conn.execute(
        "INSERT INTO changesets (description, status)
         VALUES ('fixed delta benchmark row a', 'applied')",
        [],
    )
    .unwrap();
    let benchmark_row = conn.last_insert_rowid();
    (temp, db_path, conn, benchmark_row)
}

fn sha256_file(path: &Path) -> String {
    let file = std::fs::File::open(path).unwrap();
    let mut reader = BufReader::new(file);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    hex::encode(digest.finalize())
}

fn benchmark_snapshot_history_scaling(criterion: &mut Criterion) {
    let mut provider_group = criterion.benchmark_group("generation_db_snapshot/provider");
    provider_group.sample_size(20);
    provider_group.warm_up_time(Duration::from_secs(2));
    provider_group.measurement_time(Duration::from_secs(6));

    for history_rows in HISTORY_ROWS {
        let (_temp, db_path, conn, _) = history_database(history_rows);
        let destination = db_path.with_file_name("provider-snapshot.sqlite");
        let probe = GenerationDbSnapshotProvider::default()
            .snapshot(&conn, &db_path, &destination)
            .unwrap();
        let method = probe.provenance.method.as_str().to_string();
        std::fs::remove_file(&destination).unwrap();

        provider_group.bench_with_input(
            BenchmarkId::new(method, history_rows),
            &history_rows,
            |bencher, _| {
                bencher.iter(|| {
                    let snapshot = GenerationDbSnapshotProvider::default()
                        .snapshot(black_box(&conn), black_box(&db_path), &destination)
                        .unwrap();
                    black_box(snapshot.provenance);
                });
            },
        );
    }
    provider_group.finish();

    let mut identity_group =
        criterion.benchmark_group("generation_db_snapshot/provider_plus_sha256");
    identity_group.sample_size(20);
    identity_group.warm_up_time(Duration::from_secs(2));
    identity_group.measurement_time(Duration::from_secs(6));

    for history_rows in HISTORY_ROWS {
        let (_temp, db_path, conn, _) = history_database(history_rows);
        let destination = db_path.with_file_name("identified-snapshot.sqlite");
        let probe = GenerationDbSnapshotProvider::default()
            .snapshot(&conn, &db_path, &destination)
            .unwrap();
        let method = probe.provenance.method.as_str().to_string();
        std::fs::remove_file(&destination).unwrap();

        identity_group.bench_with_input(
            BenchmarkId::new(method, history_rows),
            &history_rows,
            |bencher, _| {
                bencher.iter(|| {
                    let snapshot = GenerationDbSnapshotProvider::default()
                        .snapshot(black_box(&conn), black_box(&db_path), &destination)
                        .unwrap();
                    black_box(sha256_file(&snapshot.path));
                });
            },
        );
    }
    identity_group.finish();

    let mut delta_group = criterion.benchmark_group("generation_db_delta/fixed_update_capture");
    delta_group.sample_size(20);
    delta_group.warm_up_time(Duration::from_secs(2));
    delta_group.measurement_time(Duration::from_secs(6));

    for history_rows in HISTORY_ROWS {
        let (_temp, db_path, conn, benchmark_row) = history_database(history_rows);
        let mut use_a = true;
        delta_group.bench_with_input(
            BenchmarkId::new("sqlite-session", history_rows),
            &history_rows,
            |bencher, _| {
                bencher.iter(|| {
                    let recorder =
                        GenerationDbDeltaRecorder::begin(black_box(&conn), black_box(&db_path))
                            .unwrap();
                    use_a = !use_a;
                    let description = if use_a {
                        "fixed delta benchmark row a"
                    } else {
                        "fixed delta benchmark row b"
                    };
                    conn.execute(
                        "UPDATE changesets SET description = ?1 WHERE id = ?2",
                        (description, benchmark_row),
                    )
                    .unwrap();
                    let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap()
                    else {
                        panic!("single-connection benchmark unexpectedly required a full base");
                    };
                    black_box((delta.payload_bytes(), delta.sha256()));
                });
            },
        );
    }
    delta_group.finish();

    let mut append_group =
        criterion.benchmark_group("generation_db_delta/fixed_append_persistence");
    append_group.sample_size(20);
    append_group.warm_up_time(Duration::from_secs(2));
    append_group.measurement_time(Duration::from_secs(6));

    for history_rows in HISTORY_ROWS {
        let (temp, db_path, conn, benchmark_row) = history_database(history_rows);
        let base_state = temp.path().join("base-state");
        create_generation_db_chain_base(
            &conn,
            &db_path,
            &base_state,
            conary_core::db::schema::SCHEMA_EPOCH,
            conary_core::db::schema::SCHEMA_VERSION,
            GenerationDbBackupIdentity {
                generation_number: 1,
                state_number: 1,
                transaction_high_water_mark: None,
            },
        )
        .unwrap();
        let recorder = GenerationDbDeltaRecorder::begin(&conn, &db_path).unwrap();
        conn.execute(
            "UPDATE changesets SET description = 'fixed delta benchmark row b' WHERE id = ?1",
            [benchmark_row],
        )
        .unwrap();
        let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
            panic!("single-connection benchmark unexpectedly required a full base");
        };
        let mut next_generation = 2_i64;
        append_group.bench_with_input(
            BenchmarkId::new("sqlite-session", history_rows),
            &history_rows,
            |bencher, _| {
                bencher.iter(|| {
                    let generation_number = next_generation;
                    next_generation += 1;
                    let state_dir = temp
                        .path()
                        .join(format!("append-state-{generation_number}"));
                    let GenerationDbChainAppend::Appended(record) =
                        append_generation_db_chain_delta(
                            black_box(&base_state),
                            black_box(&state_dir),
                            black_box(&delta),
                            conary_core::db::schema::SCHEMA_EPOCH,
                            conary_core::db::schema::SCHEMA_VERSION,
                            GenerationDbBackupIdentity {
                                generation_number,
                                state_number: generation_number,
                                transaction_high_water_mark: None,
                            },
                        )
                        .unwrap()
                    else {
                        panic!("exact single-delta append unexpectedly required a full base");
                    };
                    assert_eq!(record.metrics.payload_bytes_written, delta.payload_bytes());
                    black_box(record.metrics);
                });
            },
        );
    }
    append_group.finish();
}

criterion_group!(benches, benchmark_snapshot_history_scaling);
criterion_main!(benches);
