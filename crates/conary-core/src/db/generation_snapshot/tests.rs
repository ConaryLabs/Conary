// conary-core/src/db/generation_snapshot/tests.rs

use super::*;

struct UnsupportedProvider {
    method: GenerationDbSnapshotMethod,
    checkpointed: bool,
    reason: GenerationDbSnapshotFallbackReason,
}

impl SnapshotProvider for UnsupportedProvider {
    fn method(&self) -> GenerationDbSnapshotMethod {
        self.method
    }

    fn requires_checkpointed_main(&self) -> bool {
        self.checkpointed
    }

    fn create(
        &self,
        _source: &Connection,
        _source_path: &Path,
        _destination: &Path,
    ) -> Result<GenerationDbSnapshotProviderOutcome> {
        Ok(GenerationDbSnapshotProviderOutcome::Unsupported(
            self.reason,
        ))
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("source.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE authority (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO authority (value) VALUES ('committed through WAL');",
    )
    .unwrap();
    (temp, db_path, conn)
}

#[test]
fn default_chain_snapshots_exact_committed_wal_state() {
    let (temp, db_path, conn) = fixture();
    let destination = temp.path().join("snapshot.sqlite");

    let snapshot = GenerationDbSnapshotProvider::default()
        .snapshot(&conn, &db_path, &destination)
        .unwrap();

    let copied =
        Connection::open_with_flags(&snapshot.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let value: String = copied
        .query_row("SELECT value FROM authority", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "committed through WAL");
    assert_eq!(
        snapshot.provenance.logical_size,
        destination.metadata().unwrap().len()
    );
    assert!(snapshot.provenance.wal_checkpoint.is_some());
}

#[test]
fn rollback_journal_source_records_synchronized_zero_wal_frames() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("source.sqlite");
    let destination = temp.path().join("snapshot.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE authority (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO authority (value) VALUES ('committed');",
    )
    .unwrap();

    let snapshot = GenerationDbSnapshotProvider::default()
        .snapshot(&conn, &db_path, &destination)
        .unwrap();
    let checkpoint = snapshot.provenance.wal_checkpoint.unwrap();

    assert_eq!(checkpoint.journal_mode, "delete");
    assert_eq!(checkpoint.log_frames, 0);
    assert_eq!(checkpoint.checkpointed_frames, 0);
    assert_eq!(
        Connection::open(destination)
            .unwrap()
            .query_row("SELECT value FROM authority", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "committed"
    );
}

#[test]
#[ignore = "requires CONARY_TEST_REFLINK_DIR on a reflink-capable filesystem"]
fn required_reflink_filesystem_selects_zero_copy_provider() {
    let root = std::env::var_os("CONARY_TEST_REFLINK_DIR")
        .expect("CONARY_TEST_REFLINK_DIR must name the mounted proof filesystem");
    let temp = tempfile::Builder::new()
        .prefix("conary-reflink-proof-")
        .tempdir_in(root)
        .unwrap();
    let db_path = temp.path().join("source.sqlite");
    let destination = temp.path().join("snapshot.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE authority (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO authority (value) VALUES ('reflink proof');",
    )
    .unwrap();

    let snapshot = GenerationDbSnapshotProvider::default()
        .snapshot(&conn, &db_path, &destination)
        .unwrap();

    assert_eq!(
        snapshot.provenance.method,
        GenerationDbSnapshotMethod::Reflink
    );
    assert_eq!(snapshot.provenance.payload_bytes_written, 0);
    assert!(snapshot.provenance.fallbacks.is_empty());
    assert_eq!(
        Connection::open(destination)
            .unwrap()
            .query_row("SELECT value FROM authority", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "reflink proof"
    );
}

#[test]
fn unsupported_reflink_falls_back_to_sqlite_online_backup() {
    let (temp, db_path, conn) = fixture();
    let destination = temp.path().join("snapshot.sqlite");
    let providers = GenerationDbSnapshotProvider::with_providers(vec![
        Box::new(UnsupportedProvider {
            method: GenerationDbSnapshotMethod::Reflink,
            checkpointed: true,
            reason: GenerationDbSnapshotFallbackReason::FilesystemUnsupported,
        }),
        Box::new(SqliteBackupSnapshot),
        Box::new(FullCopyFallback),
    ]);

    let snapshot = providers.snapshot(&conn, &db_path, &destination).unwrap();

    assert_eq!(
        snapshot.provenance.method,
        GenerationDbSnapshotMethod::SqliteOnlineBackup
    );
    assert_eq!(
        snapshot.provenance.fallbacks,
        vec![GenerationDbSnapshotFallback {
            method: GenerationDbSnapshotMethod::Reflink,
            reason: GenerationDbSnapshotFallbackReason::FilesystemUnsupported,
        }]
    );
    assert_eq!(
        snapshot.provenance.payload_bytes_written,
        destination.metadata().unwrap().len()
    );
}

#[test]
fn busy_wal_checkpoint_uses_online_backup_without_losing_committed_frames() {
    let (temp, db_path, conn) = fixture();
    let reader = Connection::open(&db_path).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _: i64 = reader
        .query_row("SELECT COUNT(*) FROM authority", [], |row| row.get(0))
        .unwrap();
    conn.execute(
        "INSERT INTO authority (value) VALUES ('committed after reader snapshot')",
        [],
    )
    .unwrap();
    let destination = temp.path().join("snapshot.sqlite");

    let snapshot = GenerationDbSnapshotProvider::default()
        .snapshot(&conn, &db_path, &destination)
        .unwrap();

    assert_eq!(
        snapshot.provenance.method,
        GenerationDbSnapshotMethod::SqliteOnlineBackup
    );
    assert_eq!(
        snapshot.provenance.fallbacks,
        vec![GenerationDbSnapshotFallback {
            method: GenerationDbSnapshotMethod::Reflink,
            reason: GenerationDbSnapshotFallbackReason::WalCheckpointBusy,
        }]
    );
    let copied =
        Connection::open_with_flags(&destination, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let count: i64 = copied
        .query_row("SELECT COUNT(*) FROM authority", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn full_copy_is_used_only_after_both_faster_providers_are_unavailable() {
    let (temp, db_path, conn) = fixture();
    let destination = temp.path().join("snapshot.sqlite");
    let providers = GenerationDbSnapshotProvider::with_providers(vec![
        Box::new(UnsupportedProvider {
            method: GenerationDbSnapshotMethod::Reflink,
            checkpointed: true,
            reason: GenerationDbSnapshotFallbackReason::FilesystemUnsupported,
        }),
        Box::new(UnsupportedProvider {
            method: GenerationDbSnapshotMethod::SqliteOnlineBackup,
            checkpointed: false,
            reason: GenerationDbSnapshotFallbackReason::OnlineBackupUnavailable,
        }),
        Box::new(FullCopyFallback),
    ]);

    let snapshot = providers.snapshot(&conn, &db_path, &destination).unwrap();

    assert_eq!(
        snapshot.provenance.method,
        GenerationDbSnapshotMethod::FullCopy
    );
    assert_eq!(snapshot.provenance.fallbacks.len(), 2);
    assert_eq!(
        snapshot.provenance.payload_bytes_written,
        destination.metadata().unwrap().len()
    );
}

#[test]
fn source_connection_must_name_the_exact_database_path() {
    let (temp, db_path, conn) = fixture();
    let other_path = temp.path().join("other.sqlite");
    Connection::open(&other_path).unwrap();

    let error = GenerationDbSnapshotProvider::default()
        .snapshot(&conn, &other_path, temp.path().join("snapshot.sqlite"))
        .unwrap_err();

    assert!(error.to_string().contains("source mismatch"));
    assert!(db_path.exists());
}

#[test]
fn reflink_provider_is_byte_exact_when_the_filesystem_supports_it() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    std::fs::write(&source, b"reflink snapshot authority").unwrap();

    match create_reflink(&source, &destination).unwrap() {
        GenerationDbSnapshotProviderOutcome::Unsupported(reason) => assert!(matches!(
            reason,
            GenerationDbSnapshotFallbackReason::PlatformUnsupported
                | GenerationDbSnapshotFallbackReason::FilesystemUnsupported
                | GenerationDbSnapshotFallbackReason::CrossDevice
        )),
        GenerationDbSnapshotProviderOutcome::Created {
            payload_bytes_written,
        } => {
            assert_eq!(payload_bytes_written, 0);
            assert_eq!(
                std::fs::read(&destination).unwrap(),
                b"reflink snapshot authority"
            );
            std::fs::write(&destination, b"copy-on-write destination").unwrap();
            assert_eq!(
                std::fs::read(&source).unwrap(),
                b"reflink snapshot authority"
            );
        }
    }
}

#[test]
fn persisted_provenance_rejects_impossible_provider_order() {
    let provenance = GenerationDbSnapshotProvenance {
        method: GenerationDbSnapshotMethod::FullCopy,
        fallbacks: vec![GenerationDbSnapshotFallback {
            method: GenerationDbSnapshotMethod::Reflink,
            reason: GenerationDbSnapshotFallbackReason::FilesystemUnsupported,
        }],
        wal_checkpoint: Some(GenerationDbWalCheckpoint {
            journal_mode: "wal".to_string(),
            log_frames: 0,
            checkpointed_frames: 0,
        }),
        payload_bytes_written: 4096,
        logical_size: 4096,
    };

    let error = provenance.validate().unwrap_err();
    assert!(error.to_string().contains("provider order is invalid"));
}
