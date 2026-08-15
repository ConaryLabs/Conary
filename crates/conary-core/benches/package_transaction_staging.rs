// conary-core/benches/package_transaction_staging.rs

//! Package transaction persistence scaling for many-tiny-file workloads.

use conary_core::db::models::{
    Changeset, ExistingDirectoryMaterialization, FileEntry, PackageTransactionStaging,
    StagedAnchorDisposition, StagedPayloadRow, Trove, TroveType,
};
use conary_core::payload::{PayloadContentAuthority, PayloadNode, ResolvedPayloadNode};
use conary_core::repository::versioning::VersionScheme;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rusqlite::Connection;
use std::hint::black_box;
use std::time::Duration;

fn database() -> (Connection, i64, i64) {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    let mut trove = Trove::new(
        "tiny-fixture".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let mut changeset = Changeset::new("tiny fixture benchmark".to_string());
    let changeset_id = changeset.insert(&conn).unwrap();
    (conn, trove_id, changeset_id)
}

fn rows(path_count: usize, trove_id: i64, changeset_id: i64) -> Vec<StagedPayloadRow> {
    (0..path_count)
        .map(|index| StagedPayloadRow {
            entry: FileEntry::new(
                format!("/usr/share/tiny-fixture/{index:05}"),
                ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
                Some(PayloadContentAuthority {
                    sha256: format!("{index:064x}"),
                    size: 1,
                }),
                trove_id,
            ),
            package_name: "tiny-fixture".to_string(),
            component_name: None,
            directory_materialization: ExistingDirectoryMaterialization::ApplyIncoming,
            disposition: StagedAnchorDisposition::Replace,
            selected_root_node: None,
            materialization_target_path: None,
            history: Some((changeset_id, "add")),
        })
        .collect()
}

fn benchmark_package_transaction_staging(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("package_transaction_staging/many_tiny_files");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(6));

    for path_count in [100_usize, 2_500] {
        let (mut conn, trove_id, changeset_id) = database();
        let rows = rows(path_count, trove_id, changeset_id);
        group.throughput(Throughput::Elements(path_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(path_count),
            &rows,
            |bencher, rows| {
                bencher.iter(|| {
                    let tx = conn.transaction().unwrap();
                    let mut staging = PackageTransactionStaging::begin(&tx).unwrap();
                    for row in rows {
                        staging.stage_payload(black_box(row)).unwrap();
                    }
                    let outcomes = staging.validate_and_reconcile().unwrap();
                    let work = staging.finish().unwrap();
                    black_box((outcomes.len(), work));
                    tx.rollback().unwrap();
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, benchmark_package_transaction_staging);
criterion_main!(benches);
