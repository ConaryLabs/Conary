// apps/remi/src/server/chunk_gc/tests.rs
use super::*;
use conary_core::db::models::{ConvertedPackage, RemiCatalogResource, RemiCatalogResourceKind};

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
        durable: true,
        created_at: 1,
    }
    .insert(conn)
    .unwrap();
    revision
}

#[test]
fn test_extract_hash_from_path() {
    let path = Path::new("/data/objects/ab/cdef0123456789");
    assert_eq!(
        extract_hash_from_path(path),
        Some("abcdef0123456789".to_string())
    );
}

#[test]
fn test_extract_hash_from_path_short_prefix() {
    let path = Path::new("/data/objects/0a/ff");
    assert_eq!(extract_hash_from_path(path), Some("0aff".to_string()));
}

#[test]
fn test_chunk_path() {
    let objects_dir = Path::new("/data/objects");
    let path = chunk_path(objects_dir, "abcdef0123456789");
    assert_eq!(path, PathBuf::from("/data/objects/ab/cdef0123456789"));
}

#[test]
fn test_chunk_path_short_hash() {
    let objects_dir = Path::new("/data/objects");
    let path = chunk_path(objects_dir, "ab");
    assert_eq!(path, PathBuf::from("/data/objects/ab/"));
}

#[test]
fn test_scan_local_chunks_nonexistent_dir() {
    let dir = Path::new("/nonexistent/path/objects");
    let result = scan_local_chunks(dir).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_scan_local_chunks_with_files() {
    let tmp = tempfile::tempdir().unwrap();
    let objects_dir = tmp.path().join("objects");

    // Create two-level structure: objects/ab/cdef...
    let prefix_dir = objects_dir.join("ab");
    std::fs::create_dir_all(&prefix_dir).unwrap();
    std::fs::write(prefix_dir.join("cdef0123456789"), b"chunk data").unwrap();
    std::fs::write(prefix_dir.join("9876543210fedc"), b"chunk data 2").unwrap();

    // Create every private CAS staging shape that should be skipped.
    std::fs::write(prefix_dir.join("incomplete.tmp"), b"partial").unwrap();
    std::fs::write(
        prefix_dir.join("abcdef0123456789.tmp.307631.736105"),
        b"interrupted atomic write",
    )
    .unwrap();
    std::fs::write(
        prefix_dir.join(".tmp.307631.private-batch"),
        b"private batch",
    )
    .unwrap();

    // Create another prefix
    let prefix_dir2 = objects_dir.join("ff");
    std::fs::create_dir_all(&prefix_dir2).unwrap();
    std::fs::write(prefix_dir2.join("0011223344"), b"chunk 3").unwrap();

    let mut hashes = scan_local_chunks(&objects_dir).unwrap();
    hashes.sort();

    assert_eq!(hashes.len(), 3);
    assert!(hashes.contains(&"ab9876543210fedc".to_string()));
    assert!(hashes.contains(&"abcdef0123456789".to_string()));
    assert!(hashes.contains(&"ff0011223344".to_string()));
}

#[test]
fn test_build_referenced_set() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let conn = Connection::open(tmp.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();

    let first_transport = crate::server::conversion::test_support::test_transport(&[
        "hash_a".to_string(),
        "hash_b".to_string(),
        "hash_c".to_string(),
    ]);
    let first_revision = seed_profile_resource(&conn, "fedora-44");
    let mut first = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        first_revision,
        "first".to_string(),
        "1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:test1".to_string(),
        &first_transport,
        3,
        "sha256:first".to_string(),
        "/tmp/first.ccs".to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    first.insert_with_conversion_pin(&conn, 1).unwrap();

    let second_transport = crate::server::conversion::test_support::test_transport(&[
        "hash_b".to_string(),
        "hash_d".to_string(),
    ]);
    let second_revision = seed_profile_resource(&conn, "ubuntu-26.04");
    let mut second = ConvertedPackage::new_repository(
        "ubuntu-26.04".to_string(),
        second_revision,
        "second".to_string(),
        "1".to_string(),
        "amd64".to_string(),
        "deb".to_string(),
        "sha256:test2".to_string(),
        &second_transport,
        2,
        "sha256:second".to_string(),
        "/tmp/second.ccs".to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    second.insert_with_conversion_pin(&conn, 1).unwrap();

    // Insert a protected chunk_access row
    let native_transport =
        serde_json::to_string(&crate::server::conversion::test_support::test_transport(&[
            "native-chunk".to_string(),
        ]))
        .unwrap();
    conn.execute(
        "INSERT INTO chunk_access (hash, size_bytes, access_count, protected) VALUES ('hash_e', 1024, 1, 1)",
        [],
    )
    .unwrap();

    // Insert a non-protected chunk_access row (should NOT be in referenced set)
    conn.execute(
        "INSERT INTO chunk_access (hash, size_bytes, access_count, protected) VALUES ('hash_f', 512, 1, 0)",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO repositories (name, url, source_profile)
         VALUES ('fedora', 'remi-release://fedora', 'fedora-44')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, package_release, checksum, size, download_url, version_scheme)
         VALUES (1, 'hello', '1.0.0', '1', 'sha256:hello', 42, '/v1/chunks/native-content', 'rpm')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO native_package_publications (
            repository_id, repository_package_id, source_profile, name, version, package_release,
            architecture, package_kind, authority_format_version, status, content_hash,
            transport_json, total_size, package_path, target_path, trust_status
        ) VALUES (1, 1, 'fedora-44', 'hello', '1.0.0', '1', 'noarch', 'package', 2,
                  'public', 'native-content', ?1, 42,
                  '/tmp/hello.ccs', 'packages/fedora/hello.ccs', 'verified')",
        [native_transport],
    )
    .unwrap();

    let referenced = build_referenced_set(&conn).unwrap();

    assert!(referenced.contains("hash_a"));
    assert!(referenced.contains("hash_b"));
    assert!(referenced.contains("hash_c"));
    assert!(referenced.contains("hash_d"));
    assert!(referenced.contains("hash_e")); // protected
    assert!(referenced.contains("native-chunk"));
    assert!(!referenced.contains("hash_f")); // not protected, not in any package
    assert_eq!(referenced.len(), 6);
}

#[test]
fn referenced_set_rejects_malformed_converted_transport_authority() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    let transport = crate::server::conversion::test_support::test_transport(&["hash".to_string()]);
    let revision = seed_profile_resource(&conn, "fedora-44");
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        revision,
        "broken".to_string(),
        "1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &transport,
        1,
        "sha256:content".to_string(),
        "/tmp/broken.ccs".to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    let id = converted.insert_with_conversion_pin(&conn, 1).unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    conn.execute(
        "UPDATE converted_packages SET transport_json = '{bad' WHERE id = ?1",
        [id],
    )
    .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")
        .unwrap();

    let error = format!("{:#}", build_referenced_set(&conn).unwrap_err());
    assert!(error.contains("malformed transport_json"), "{error}");
}

#[test]
fn referenced_set_rejects_current_conversion_without_exact_pin() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    let revision = seed_profile_resource(&conn, "fedora-44");
    let transport = crate::server::conversion::test_support::test_transport(&["hash".to_string()]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        revision,
        "unpinned".to_string(),
        "1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:unpinned".to_string(),
        &transport,
        1,
        "sha256:content".to_string(),
        "/tmp/unpinned.ccs".to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    let id = converted.insert_with_conversion_pin(&conn, 1).unwrap();
    conn.execute(
        "DELETE FROM remi_profile_revision_pins WHERE pin_id = ?1",
        [ConvertedPackage::conversion_pin_id(id)],
    )
    .unwrap();

    let error = format!("{:#}", build_referenced_set(&conn).unwrap_err());
    assert!(
        error.contains("has no exact profile-revision pin"),
        "{error}"
    );
}

#[test]
fn referenced_set_rejects_malformed_public_native_transport_authority() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    conn.execute(
        "INSERT INTO repositories (name, url, source_profile)
         VALUES ('fedora', 'remi-release://fedora', 'fedora-44')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, package_release, checksum, size, download_url, version_scheme)
         VALUES (1, 'broken', '1', '1', 'sha256:broken', 1, '/v1/chunks/broken', 'rpm')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO native_package_publications (
            repository_id, repository_package_id, source_profile, name, version, package_release,
            architecture, package_kind, authority_format_version, status, content_hash,
            transport_json, total_size, package_path, target_path, trust_status
        ) VALUES (1, 1, 'fedora-44', 'broken', '1', '1', 'noarch', 'package', 2,
                  'public', 'sha256:broken', '{bad', 1,
                  '/tmp/broken.ccs', 'packages/fedora/broken.ccs', 'verified')",
        [],
    )
    .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")
        .unwrap();
    let id = conn.last_insert_rowid();

    let error = build_referenced_set(&conn).unwrap_err().to_string();
    assert!(
        error.contains(&format!(
            "native package publication {id} has malformed transport_json"
        )),
        "{error}"
    );
}

fn insert_chunk_access(conn: &Connection, hash: &str, last_accessed: &str) {
    conn.execute(
        "INSERT INTO chunk_access (hash, size_bytes, access_count, protected, last_accessed)
         VALUES (?1, 1024, 1, 0, ?2)",
        rusqlite::params![hash, last_accessed],
    )
    .unwrap();
}

#[test]
fn grace_filter_handles_more_orphans_than_sqlite_host_variables() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();

    let cutoff = "2026-01-02 00:00:00";
    insert_chunk_access(&conn, "recent-orphan", "2026-01-03 00:00:00");
    insert_chunk_access(&conn, "old-orphan", "2026-01-01 00:00:00");
    // Equal to the cutoff is not "recent": the comparison is strict.
    insert_chunk_access(&conn, "cutoff-orphan", cutoff);

    // More candidates than SQLite's ~32,766 host-variable limit, which the
    // previous `hash IN (?,?,...)` form bound one parameter at a time.
    let mut orphans = vec![
        "recent-orphan".to_string(),
        "old-orphan".to_string(),
        "cutoff-orphan".to_string(),
    ];
    orphans.extend((0..33_000).map(|i| format!("never-accessed-{i:05}")));

    let collected = filter_recently_accessed(&conn, &orphans, cutoff)
        .expect("grace filter must survive more candidates than SQLite host variables");
    let collected_set: HashSet<&str> = collected.iter().map(String::as_str).collect();

    assert!(
        !collected_set.contains("recent-orphan"),
        "recently-accessed orphan must be kept out of the delete set"
    );
    assert!(collected_set.contains("old-orphan"));
    assert!(collected_set.contains("cutoff-orphan"));
    // Chunks with no chunk_access row at all are collected.
    assert!(collected_set.contains("never-accessed-00000"));
    assert!(collected_set.contains("never-accessed-32999"));
    assert_eq!(collected.len(), orphans.len() - 1);
}

#[test]
fn grace_filter_returns_empty_for_no_candidates() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    insert_chunk_access(&conn, "recent-orphan", "2026-01-03 00:00:00");

    let collected = filter_recently_accessed(&conn, &[], "2026-01-02 00:00:00").unwrap();
    assert!(collected.is_empty());
}

#[test]
fn chunk_access_cleanup_deletes_every_batch() {
    let mut conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();

    // Spans three 10,000-row batches.
    let hashes: Vec<String> = (0..25_000).map(|i| format!("hash-{i:05}")).collect();
    for hash in &hashes {
        insert_chunk_access(&conn, hash, "2026-01-01 00:00:00");
    }

    // A hash with no row must not abort the batch containing it.
    let mut targets = hashes.clone();
    targets.push("absent-hash".to_string());

    let deleted = delete_chunk_access_rows(&mut conn, &targets).unwrap();
    assert_eq!(deleted, hashes.len());

    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunk_access", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn chunk_access_cleanup_accepts_empty_input() {
    let mut conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    assert_eq!(delete_chunk_access_rows(&mut conn, &[]).unwrap(), 0);
}

fn insert_sized_chunk_access(conn: &Connection, hash: &str, size_bytes: i64) {
    conn.execute(
        "INSERT INTO chunk_access (hash, size_bytes, access_count, protected)
         VALUES (?1, ?2, 1, 0)",
        rusqlite::params![hash, size_bytes],
    )
    .unwrap();
}

/// The freed-bytes estimate must count the chunks R2 removed and only those:
/// a key R2 refused is still occupying the bucket.
#[test]
fn r2_bytes_freed_excludes_hashes_r2_refused() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    insert_sized_chunk_access(&conn, "deleted-a", 1_000);
    insert_sized_chunk_access(&conn, "deleted-b", 20);
    insert_sized_chunk_access(&conn, "refused", 400_000);
    // A chunk nobody asked to delete must not be priced into the run.
    insert_sized_chunk_access(&conn, "untouched", 9_999);

    let submitted = vec![
        "deleted-a".to_string(),
        "deleted-b".to_string(),
        "refused".to_string(),
    ];
    let failed = BTreeSet::from(["refused".to_string()]);

    let freed = r2_freed_hashes(&submitted, &failed);
    assert_eq!(freed.len(), 2);
    assert!(!freed.contains("refused"));

    assert_eq!(sum_chunk_access_bytes(&conn, &freed).unwrap(), 1_020);
}

/// The estimate must not bind one host variable per hash: production runs
/// price hundreds of thousands of orphans in one pass.
#[test]
fn r2_bytes_estimate_handles_more_hashes_than_sqlite_host_variables() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    for i in 0..33_000 {
        insert_sized_chunk_access(&conn, &format!("hash-{i:05}"), 2);
    }

    let mut hashes: HashSet<String> = (0..33_000).map(|i| format!("hash-{i:05}")).collect();
    // A hash with no chunk_access row contributes nothing.
    hashes.insert("never-recorded".to_string());

    assert_eq!(sum_chunk_access_bytes(&conn, &hashes).unwrap(), 66_000);
}

#[test]
fn r2_bytes_estimate_is_zero_without_hashes() {
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    insert_sized_chunk_access(&conn, "orphan", 512);

    assert_eq!(
        sum_chunk_access_bytes(&conn, &HashSet::new()).unwrap(),
        0,
        "an empty delete set must not sum the whole table"
    );
}

/// A dry run reports what it would remove without touching disk, R2, or the
/// `chunk_access` rows it priced.
#[tokio::test]
async fn dry_run_counts_orphans_without_deleting_them() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("remi.db");
    let conn = Connection::open(&db_path).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    insert_sized_chunk_access(&conn, "abcdef0123456789", 4_096);
    drop(conn);

    let objects_dir = tmp.path().join("objects");
    std::fs::create_dir_all(objects_dir.join("ab")).unwrap();
    std::fs::write(objects_dir.join("ab").join("cdef0123456789"), b"chunk").unwrap();

    let result = run_chunk_gc(&db_path, &objects_dir, None, true, 0)
        .await
        .unwrap();

    assert_eq!(result.local_scanned, 1);
    assert_eq!(result.local_deleted, 1);
    // No R2 store configured: nothing was listed, so nothing is priced.
    assert_eq!(result.r2_scanned, 0);
    assert_eq!(result.r2_deleted, 0);
    assert_eq!(result.r2_bytes_freed, 0);
    assert!(objects_dir.join("ab").join("cdef0123456789").exists());

    let conn = Connection::open(&db_path).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunk_access", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1, "a dry run must not delete chunk_access rows");
}

/// The real path deletes the local orphan, measures its bytes, and drops its
/// `chunk_access` row.
#[tokio::test]
async fn real_run_deletes_local_orphans_and_their_access_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("remi.db");
    let conn = Connection::open(&db_path).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    insert_sized_chunk_access(&conn, "abcdef0123456789", 4_096);
    drop(conn);

    let objects_dir = tmp.path().join("objects");
    std::fs::create_dir_all(objects_dir.join("ab")).unwrap();
    let chunk = objects_dir.join("ab").join("cdef0123456789");
    std::fs::write(&chunk, b"chunk bytes").unwrap();

    let result = run_chunk_gc(&db_path, &objects_dir, None, false, 0)
        .await
        .unwrap();

    assert_eq!(result.local_deleted, 1);
    assert_eq!(result.local_bytes_freed, b"chunk bytes".len() as u64);
    assert!(!chunk.exists());

    let conn = Connection::open(&db_path).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunk_access", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

/// Seed a store whose only local chunk is `hash`, plus `chunk_access` rows
/// for every hash in `sized`.
fn seed_gc_store(sized: &[(&str, i64)]) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("remi.db");
    let conn = Connection::open(&db_path).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    for (hash, size) in sized {
        insert_sized_chunk_access(&conn, hash, *size);
    }
    drop(conn);

    let objects_dir = tmp.path().join("objects");
    std::fs::create_dir_all(&objects_dir).unwrap();
    (tmp, db_path, objects_dir)
}

fn remaining_access_rows(db_path: &Path) -> Vec<String> {
    let conn = Connection::open(db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT hash FROM chunk_access ORDER BY hash")
        .unwrap();
    let rows: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows
}

/// A `chunk_access` row may only die once everything it describes is gone.
/// R2 refused `refused-orphan`, so its object is still in the bucket and its
/// recorded size is the only thing that can price the next run's retry.
#[tokio::test]
async fn refused_r2_orphan_keeps_its_chunk_access_row() {
    let (_tmp, db_path, objects_dir) =
        seed_gc_store(&[("deleted-orphan", 1_000), ("refused-orphan", 400_000)]);

    let submitted = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = Arc::clone(&submitted);
    let result = run_chunk_gc_with_deleter(
        &db_path,
        &objects_dir,
        Some(vec![
            "deleted-orphan".to_string(),
            "refused-orphan".to_string(),
        ]),
        move |hashes: Arc<Vec<String>>| async move {
            recorder.lock().unwrap().extend(hashes.iter().cloned());
            Ok(R2BatchDeleteOutcome {
                attempted: hashes.len(),
                deleted: 1,
                failed_hashes: BTreeSet::from(["refused-orphan".to_string()]),
                failure_samples: vec![(
                    "refused-orphan".to_string(),
                    "AccessDenied: denied".to_string(),
                )],
            })
        },
        false,
        0,
    )
    .await
    .unwrap();

    let mut submitted = submitted.lock().unwrap().clone();
    submitted.sort();
    assert_eq!(submitted, vec!["deleted-orphan", "refused-orphan"]);

    assert_eq!(result.r2_scanned, 2);
    assert_eq!(result.r2_deleted, 1);
    assert_eq!(
        result.r2_bytes_freed, 1_000,
        "a refused key's bytes are still in the bucket"
    );

    assert_eq!(
        remaining_access_rows(&db_path),
        vec!["refused-orphan".to_string()],
        "the refused key must keep the row that records its size"
    );
}

/// The other half of the same contract: a key R2 actually removed loses its
/// row, so a refusal is what keeps a row rather than the R2 path as a whole.
#[tokio::test]
async fn fully_deleted_r2_orphans_lose_their_chunk_access_rows() {
    let (_tmp, db_path, objects_dir) = seed_gc_store(&[("orphan-a", 1_000), ("orphan-b", 20)]);

    let result = run_chunk_gc_with_deleter(
        &db_path,
        &objects_dir,
        Some(vec!["orphan-a".to_string(), "orphan-b".to_string()]),
        |hashes: Arc<Vec<String>>| async move {
            Ok(R2BatchDeleteOutcome {
                attempted: hashes.len(),
                deleted: hashes.len(),
                ..Default::default()
            })
        },
        false,
        0,
    )
    .await
    .unwrap();

    assert_eq!(result.r2_deleted, 2);
    assert_eq!(result.r2_bytes_freed, 1_020);
    assert!(remaining_access_rows(&db_path).is_empty());
}

/// A whole batch R2 could not even submit refuses every key in it, so no row
/// in that batch may be cleaned up.
#[tokio::test]
async fn r2_request_failure_keeps_every_chunk_access_row_it_covered() {
    let (_tmp, db_path, objects_dir) = seed_gc_store(&[("orphan-a", 1_000), ("orphan-b", 20)]);

    let result = run_chunk_gc_with_deleter(
        &db_path,
        &objects_dir,
        Some(vec!["orphan-a".to_string(), "orphan-b".to_string()]),
        |hashes: Arc<Vec<String>>| async move {
            Ok(R2BatchDeleteOutcome {
                attempted: hashes.len(),
                deleted: 0,
                failed_hashes: hashes.iter().cloned().collect(),
                failure_samples: Vec::new(),
            })
        },
        false,
        0,
    )
    .await
    .unwrap();

    assert_eq!(result.r2_deleted, 0);
    assert_eq!(result.r2_bytes_freed, 0);
    assert_eq!(
        remaining_access_rows(&db_path),
        vec!["orphan-a".to_string(), "orphan-b".to_string()]
    );
}

/// Without an R2 listing there is nothing to refuse, so every collected
/// orphan's row is cleaned up and the deleter is never called.
#[tokio::test]
async fn local_only_run_cleans_up_every_collected_row() {
    let (_tmp, db_path, objects_dir) = seed_gc_store(&[("abcdef0123456789", 4_096)]);
    std::fs::create_dir_all(objects_dir.join("ab")).unwrap();
    std::fs::write(objects_dir.join("ab").join("cdef0123456789"), b"chunk").unwrap();

    let result = run_chunk_gc_with_deleter(
        &db_path,
        &objects_dir,
        None,
        |_| async { panic!("no R2 listing means no R2 delete") },
        false,
        0,
    )
    .await
    .unwrap();

    assert_eq!(result.local_deleted, 1);
    assert_eq!(result.r2_scanned, 0);
    assert_eq!(result.r2_bytes_freed, 0);
    assert!(remaining_access_rows(&db_path).is_empty());
}

#[test]
fn test_gc_result_default() {
    let result = GcResult::default();
    assert_eq!(result.local_scanned, 0);
    assert_eq!(result.r2_scanned, 0);
    assert_eq!(result.referenced, 0);
    assert_eq!(result.local_deleted, 0);
    assert_eq!(result.r2_deleted, 0);
    assert_eq!(result.local_bytes_freed, 0);
    assert_eq!(result.r2_bytes_freed, 0);
}
