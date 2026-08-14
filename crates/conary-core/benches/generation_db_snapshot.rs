// conary-core/benches/generation_db_snapshot.rs

//! Generation database snapshot scaling against accumulated SQLite history.

use conary_core::db::generation_snapshot::GenerationDbSnapshotProvider;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::hint::black_box;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Duration;

const HISTORY_ROWS: [usize; 3] = [0, 10_000, 100_000];

fn history_database(rows: usize) -> (tempfile::TempDir, std::path::PathBuf, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary-history.sqlite");
    let mut conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE generation_history (
             id INTEGER PRIMARY KEY,
             generation_number INTEGER NOT NULL,
             summary TEXT NOT NULL,
             authority BLOB NOT NULL
         );",
    )
    .unwrap();
    let transaction = conn.transaction().unwrap();
    {
        let mut insert = transaction
            .prepare(
                "INSERT INTO generation_history (generation_number, summary, authority)
                 VALUES (?1, ?2, ?3)",
            )
            .unwrap();
        for row in 0..rows {
            let summary = format!("generation history row {row:08}");
            let authority = format!("{row:064x}{:064x}", rows.saturating_sub(row));
            insert
                .execute((row as i64, summary, authority.as_bytes()))
                .unwrap();
        }
    }
    transaction.commit().unwrap();
    (temp, db_path, conn)
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
        let (_temp, db_path, conn) = history_database(history_rows);
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
        let (_temp, db_path, conn) = history_database(history_rows);
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
}

criterion_group!(benches, benchmark_snapshot_history_scaling);
criterion_main!(benches);
