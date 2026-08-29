// crates/conary-core/src/repository/catalog/portable_vfs/tests.rs

use std::fs::{self, File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Barrier};

use rusqlite::{Connection, ErrorCode, params};
use tempfile::TempDir;

use super::*;
use crate::repository::catalog::{CatalogArtifactV1, PORTABLE_CHUNK_SIZE_V1};

struct CatalogFixture {
    _temp: TempDir,
    path: std::path::PathBuf,
    manifest: PortableChunkManifestV1,
}

impl CatalogFixture {
    fn with_text(value: &str) -> Self {
        let temp = tempfile::tempdir().expect("create fixture directory");
        let path = temp.path().join("catalog.sqlite");
        let connection = Connection::open(&path).expect("create fixture catalog");
        connection
            .execute_batch(
                "PRAGMA page_size = 4096;
                 PRAGMA journal_mode = DELETE;
                 CREATE TABLE values_by_key (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .expect("create fixture schema");
        connection
            .execute(
                "INSERT INTO values_by_key (key, value) VALUES ('fixture', ?1)",
                [value],
            )
            .expect("insert fixture value");
        connection.execute_batch("VACUUM").expect("compact fixture");
        connection.close().expect("close fixture writer");
        Self::finish(temp, path)
    }

    fn with_blob(bytes: usize) -> Self {
        let temp = tempfile::tempdir().expect("create fixture directory");
        let path = temp.path().join("catalog.sqlite");
        let connection = Connection::open(&path).expect("create fixture catalog");
        connection
            .execute_batch(
                "PRAGMA page_size = 4096;
                 PRAGMA journal_mode = DELETE;
                 CREATE TABLE payloads (id INTEGER PRIMARY KEY, payload BLOB NOT NULL);",
            )
            .expect("create fixture schema");
        let payload = vec![0x5a_u8; bytes];
        connection
            .execute(
                "INSERT INTO payloads (id, payload) VALUES (1, ?1)",
                params![payload],
            )
            .expect("insert fixture payload");
        connection.execute_batch("VACUUM").expect("compact fixture");
        connection.close().expect("close fixture writer");
        Self::finish(temp, path)
    }

    fn finish(temp: TempDir, path: std::path::PathBuf) -> Self {
        File::open(&path)
            .expect("open fixture for durability")
            .sync_all()
            .expect("sync fixture");
        let mut hash_reader = File::open(&path).expect("open fixture for hash");
        let artifact = CatalogArtifactV1 {
            sha256: crate::hash::sha256_reader_hex(&mut hash_reader).expect("hash fixture"),
            size: fs::metadata(&path).expect("stat fixture").len(),
        };
        let carrier = File::open(&path).expect("open fixture for manifest");
        let manifest = PortableChunkManifestV1::build(&carrier, &artifact)
            .expect("build portable fixture manifest");
        Self {
            _temp: temp,
            path,
            manifest,
        }
    }

    fn open(&self) -> PortableCatalogConnection {
        PortableCatalogConnection::open(
            File::open(&self.path).expect("open fixture carrier"),
            self.manifest.clone(),
        )
        .expect("open portable fixture")
    }
}

#[test]
fn sqlite_queries_use_only_authenticated_cached_chunks() {
    let fixture = CatalogFixture::with_text("authenticated");
    let reader = fixture.open();

    let value: String = reader
        .connection()
        .query_row(
            "SELECT value FROM values_by_key WHERE key = 'fixture'",
            [],
            |row| row.get(0),
        )
        .expect("query authenticated catalog");
    assert_eq!(value, "authenticated");

    let metrics = reader.metrics();
    assert!(metrics.read_calls > 0);
    assert!(metrics.authenticated_chunks > 0);
    assert!(metrics.authenticated_bytes > 0);
    assert_eq!(
        metrics.chunk_accesses,
        metrics.cache_hits + metrics.cache_misses
    );
    assert_eq!(metrics.cache_misses, metrics.authenticated_chunks);
    assert_eq!(metrics.integrity_failures, 0);
}

#[test]
fn sqlite_mutations_are_refused_without_changing_the_carrier() {
    let fixture = CatalogFixture::with_text("unchanged");
    let before = fs::read(&fixture.path).expect("read carrier before refused mutation");
    let reader = fixture.open();

    let error = reader
        .connection()
        .execute("UPDATE values_by_key SET value = 'changed'", [])
        .expect_err("portable catalog update must be refused");
    assert!(matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == ErrorCode::ReadOnly
    ));
    drop(reader);

    assert_eq!(
        fs::read(&fixture.path).expect("read carrier after refused mutation"),
        before
    );
}

#[cfg(unix)]
#[test]
fn post_open_unread_chunk_tamper_fails_before_query_data_returns() {
    let fixture = CatalogFixture::with_blob(4 * PORTABLE_CHUNK_SIZE_V1 as usize);
    assert!(fixture.manifest.chunk_count() >= 3);
    let reader = fixture.open();
    let target = fixture.manifest.chunk_range(2).expect("third chunk range");

    let carrier = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fixture.path)
        .expect("open carrier for tamper");
    let tamper_offset = target.offset + 17;
    let mut original = [0_u8; 1];
    carrier
        .read_at(&mut original, tamper_offset)
        .expect("read original carrier byte");
    carrier
        .write_at(&[original[0] ^ 0xff], tamper_offset)
        .expect("tamper unread carrier chunk");

    let error = reader
        .connection()
        .query_row("SELECT payload FROM payloads WHERE id = 1", [], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .expect_err("tampered chunk must not produce query data");
    assert!(matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.extended_code == ffi::SQLITE_IOERR_DATA
    ));
    let failure = reader
        .first_failure()
        .expect("authenticated refusal records exact failure");
    assert_eq!(failure.kind, PortableVfsFailureKindV1::Authentication);
    assert_eq!(failure.chunk_index, Some(2));
    assert!(reader.metrics().integrity_failures > 0);
}

#[cfg(unix)]
#[test]
fn post_open_cached_chunk_mutation_keeps_serving_authenticated_bytes() {
    let fixture = CatalogFixture::with_text("authenticated-cache");
    let reader = fixture.open();
    let offset = 128;
    let mut expected = [0_u8; 1];
    assert!(
        !reader
            .file
            .read(offset, &mut expected)
            .expect("prime exact authenticated chunk")
    );
    let before = reader.metrics();

    let carrier = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fixture.path)
        .expect("open carrier for cached-chunk mutation");
    carrier
        .write_at(&[expected[0] ^ 0xff], offset)
        .expect("mutate cached carrier chunk");
    carrier.sync_all().expect("sync cached-chunk mutation");

    let mut observed = [0_u8; 1];
    assert!(
        !reader
            .file
            .read(offset, &mut observed)
            .expect("read retained authenticated chunk")
    );
    assert_eq!(observed, expected);
    let after = reader.metrics();
    assert!(after.cache_hits > before.cache_hits);
    assert_eq!(after.authenticated_chunks, before.authenticated_chunks);
    assert_eq!(after.integrity_failures, 0);
}

#[test]
fn mismatched_manifest_refuses_the_sqlite_open() {
    let expected = CatalogFixture::with_text("alpha");
    let other = CatalogFixture::with_text("bravo");
    assert_eq!(
        fs::metadata(&expected.path).unwrap().len(),
        fs::metadata(&other.path).unwrap().len()
    );

    let error = PortableCatalogConnection::open(
        File::open(&other.path).expect("open mismatched carrier"),
        expected.manifest.clone(),
    )
    .err()
    .expect("mismatched manifest must refuse");
    assert!(error.to_string().contains("authenticated read failed"));
}

#[test]
fn concurrent_connections_keep_exact_token_and_vfs_ownership() {
    let fixture = CatalogFixture::with_text("parallel");
    let barrier = Arc::new(Barrier::new(8));
    let path = Arc::new(fixture.path.clone());
    let manifest = Arc::new(fixture.manifest.clone());
    let mut readers = Vec::new();

    for _ in 0..8 {
        let barrier = barrier.clone();
        let path = path.clone();
        let manifest = manifest.clone();
        readers.push(std::thread::spawn(move || {
            barrier.wait();
            let reader = PortableCatalogConnection::open(
                File::open(path.as_ref()).expect("open concurrent carrier"),
                manifest.as_ref().clone(),
            )
            .expect("open concurrent authenticated reader");
            let value: String = reader
                .connection()
                .query_row(
                    "SELECT value FROM values_by_key WHERE key = 'fixture'",
                    [],
                    |row| row.get(0),
                )
                .expect("query concurrent authenticated reader");
            assert_eq!(value, "parallel");
            assert!(reader.metrics().authenticated_chunks > 0);
        }));
    }
    for reader in readers {
        reader.join().expect("concurrent reader did not panic");
    }
}
