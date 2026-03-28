// conary-core/src/generation/gc.rs

//! Generation and CAS garbage collection.
//!
//! In the composefs-native model, CAS object liveness is determined by which
//! generations are kept. A CAS object is live if any surviving generation's
//! state_members reference a trove whose files include that object's hash.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::filesystem::CasStore;
use rusqlite::Connection;
use tracing::{debug, info};

const GC_RECENT_OBJECT_GRACE_PERIOD: Duration = Duration::from_secs(60 * 60);

/// Statistics from a CAS garbage collection run.
#[derive(Debug, Clone, Default)]
pub struct GcStats {
    /// Total CAS objects inspected.
    pub objects_checked: u64,
    /// CAS objects removed (unreferenced).
    pub objects_removed: u64,
    /// Bytes freed by removing unreferenced objects.
    pub bytes_freed: u64,
    /// Hashes of all CAS objects that were deleted (audit trail).
    pub deleted_hashes: Vec<String>,
}

/// Get the set of CAS hashes referenced by surviving generations.
///
/// Queries `state_cas_hashes` -- an immutable snapshot table populated at
/// state creation time -- instead of joining through the mutable
/// `files`/`troves`/`state_members` tables. This ensures GC correctness
/// even after package upgrades cascade-delete old trove and file rows.
///
/// Uses `json_each()` to bind the state ID list as a single JSON array
/// parameter, avoiding the `SQLITE_MAX_VARIABLE_NUMBER` limit that a
/// per-ID placeholder approach would hit with large state lists.
pub fn live_cas_hashes(
    conn: &Connection,
    surviving_state_ids: &[i64],
) -> crate::Result<HashSet<String>> {
    if surviving_state_ids.is_empty() {
        return Ok(HashSet::new());
    }

    let json_array = serde_json::to_string(surviving_state_ids)?;

    // Query the snapshot table directly -- no join through mutable troves/files.
    // state_cas_hashes is populated at snapshot creation time and is immutable
    // thereafter, so it correctly reflects the CAS objects each generation needs
    // even after package upgrades delete old trove/file rows.
    let sql = "SELECT DISTINCT sha256_hash FROM state_cas_hashes \
               WHERE state_id IN (SELECT value FROM json_each(?1))";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([&json_array], |row| row.get::<_, String>(0))?;

    let mut hashes = HashSet::new();
    for row in rows {
        hashes.insert(row?);
    }

    debug!(
        "Found {} live CAS hashes across {} surviving states",
        hashes.len(),
        surviving_state_ids.len()
    );
    Ok(hashes)
}

/// Remove CAS objects not in the live set.
///
/// Uses `CasStore::iter_objects()` to walk the two-level objects directory
/// and deletes any object whose hash is not in `live_hashes`.
pub fn gc_cas_objects(objects_dir: &Path, live_hashes: &HashSet<String>) -> crate::Result<GcStats> {
    let mut stats = GcStats::default();

    if !objects_dir.exists() {
        info!("CAS objects directory does not exist, nothing to collect");
        return Ok(stats);
    }

    let cas = CasStore::new(objects_dir)?;

    for result in cas.iter_objects() {
        let (hash, path) = result?;
        stats.objects_checked += 1;

        if !live_hashes.contains(&hash) {
            if should_skip_recent_object(&path, SystemTime::now(), GC_RECENT_OBJECT_GRACE_PERIOD) {
                debug!("Skipping recent CAS object during GC grace period: {hash}");
                continue;
            }

            if let Ok(metadata) = path.metadata() {
                stats.bytes_freed += metadata.len();
            }

            match std::fs::remove_file(&path) {
                Ok(()) => {
                    stats.objects_removed += 1;
                    stats.deleted_hashes.push(hash.clone());
                    debug!("Removed unreferenced CAS object: {hash}");
                }
                Err(e) => {
                    tracing::warn!("Failed to remove CAS object {hash}: {e}");
                }
            }
        }
    }

    // Clean up empty prefix directories
    if let Ok(entries) = std::fs::read_dir(objects_dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|ft| ft.is_dir())
                && entry
                    .path()
                    .read_dir()
                    .is_ok_and(|mut d| d.next().is_none())
            {
                let _ = std::fs::remove_dir(entry.path());
            }
        }
    }

    info!(
        "CAS GC: checked {}, removed {}, freed {} bytes",
        stats.objects_checked, stats.objects_removed, stats.bytes_freed
    );

    Ok(stats)
}

fn should_skip_recent_object(path: &Path, now: SystemTime, grace_period: Duration) -> bool {
    path.metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age < grace_period)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::params;
    use tempfile::TempDir;

    /// Create a test database with the full schema.
    fn create_test_db() -> (TempDir, Connection) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (tmp, conn)
    }

    /// Insert a trove and its files, returning the trove ID.
    fn insert_trove_with_files(
        conn: &Connection,
        name: &str,
        version: &str,
        files: &[(&str, &str)], // (path, sha256_hash)
    ) -> i64 {
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture, install_reason) \
             VALUES (?1, ?2, 'package', 'x86_64', 'explicit')",
            params![name, version],
        )
        .unwrap();
        let trove_id = conn.last_insert_rowid();

        for (path, hash) in files {
            conn.execute(
                "INSERT INTO files (path, sha256_hash, size, permissions, trove_id) \
                 VALUES (?1, ?2, 1024, 493, ?3)",
                params![path, hash, trove_id],
            )
            .unwrap();
        }

        trove_id
    }

    /// Create a system state with the given members, returning its ID.
    ///
    /// Also populates `state_cas_hashes` by joining through troves/files,
    /// mirroring what `StateEngine::create_snapshot_at()` does in production.
    fn create_state_with_members(
        conn: &Connection,
        state_number: i64,
        members: &[(&str, &str)], // (trove_name, trove_version)
    ) -> i64 {
        conn.execute(
            "INSERT INTO system_states (state_number, summary, package_count) \
             VALUES (?1, 'test state', ?2)",
            params![state_number, members.len() as i64],
        )
        .unwrap();
        let state_id = conn.last_insert_rowid();

        for (name, version) in members {
            conn.execute(
                "INSERT INTO state_members (state_id, trove_name, trove_version, install_reason) \
                 VALUES (?1, ?2, ?3, 'explicit')",
                params![state_id, name, version],
            )
            .unwrap();
        }

        // Snapshot CAS hashes for GC liveness (mirrors create_snapshot_at).
        conn.execute(
            "INSERT OR IGNORE INTO state_cas_hashes (state_id, sha256_hash)
             SELECT ?1, f.sha256_hash
             FROM state_members sm
             JOIN troves t ON t.name = sm.trove_name AND t.version = sm.trove_version
             JOIN files f ON f.trove_id = t.id
             WHERE sm.state_id = ?1
               AND f.sha256_hash IS NOT NULL
               AND f.sha256_hash != ''
               AND NOT f.sha256_hash LIKE 'adopted-%'",
            params![state_id],
        )
        .unwrap();

        state_id
    }

    /// Create a CAS object file in the two-level directory layout.
    fn create_cas_object(objects_dir: &Path, hash: &str, content: &[u8]) {
        let (prefix, suffix) = hash.split_at(2);
        let dir = objects_dir.join(prefix);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(suffix), content).unwrap();
    }

    #[test]
    fn test_live_cas_hashes_basic() {
        let (_tmp, conn) = create_test_db();

        // Insert troves with files
        insert_trove_with_files(
            &conn,
            "pkg-a",
            "1.0",
            &[
                (
                    "/usr/bin/a",
                    "aaaa000000000000000000000000000000000000000000000000000000000001",
                ),
                (
                    "/usr/lib/liba.so",
                    "aaaa000000000000000000000000000000000000000000000000000000000002",
                ),
            ],
        );
        insert_trove_with_files(
            &conn,
            "pkg-b",
            "2.0",
            &[(
                "/usr/bin/b",
                "bbbb000000000000000000000000000000000000000000000000000000000001",
            )],
        );
        insert_trove_with_files(
            &conn,
            "pkg-c",
            "1.0",
            &[(
                "/usr/bin/c",
                "cccc000000000000000000000000000000000000000000000000000000000001",
            )],
        );

        // State 1 references pkg-a and pkg-b
        let state1 = create_state_with_members(&conn, 1, &[("pkg-a", "1.0"), ("pkg-b", "2.0")]);

        // State 2 references pkg-b and pkg-c
        let state2 = create_state_with_members(&conn, 2, &[("pkg-b", "2.0"), ("pkg-c", "1.0")]);

        // Query with both states surviving
        let hashes = live_cas_hashes(&conn, &[state1, state2]).unwrap();
        assert_eq!(hashes.len(), 4, "All 4 hashes should be live");
        assert!(
            hashes.contains("aaaa000000000000000000000000000000000000000000000000000000000001")
        );
        assert!(
            hashes.contains("aaaa000000000000000000000000000000000000000000000000000000000002")
        );
        assert!(
            hashes.contains("bbbb000000000000000000000000000000000000000000000000000000000001")
        );
        assert!(
            hashes.contains("cccc000000000000000000000000000000000000000000000000000000000001")
        );

        // Query with only state 2 surviving
        let hashes = live_cas_hashes(&conn, &[state2]).unwrap();
        assert_eq!(
            hashes.len(),
            2,
            "Only pkg-b and pkg-c hashes should be live"
        );
        assert!(
            hashes.contains("bbbb000000000000000000000000000000000000000000000000000000000001")
        );
        assert!(
            hashes.contains("cccc000000000000000000000000000000000000000000000000000000000001")
        );
        assert!(
            !hashes.contains("aaaa000000000000000000000000000000000000000000000000000000000001")
        );
    }

    #[test]
    fn test_live_cas_hashes_empty_states() {
        let (_tmp, conn) = create_test_db();

        let hashes = live_cas_hashes(&conn, &[]).unwrap();
        assert!(
            hashes.is_empty(),
            "No surviving states means no live hashes"
        );
    }

    /// Regression test for the cascade-delete GC bug (finding 4.1).
    ///
    /// Scenario: trove foo-1.0 is installed (state 1), then upgraded to
    /// foo-2.0 (state 2). The upgrade deletes the foo-1.0 trove row,
    /// which CASCADE-deletes its file rows. Without the state_cas_hashes
    /// snapshot table, GC would lose foo-1.0's hashes and incorrectly
    /// collect CAS objects still needed by state 1.
    #[test]
    fn test_live_cas_hashes_survives_trove_cascade_delete() {
        let (_tmp, conn) = create_test_db();

        let hash_v1 = "aaaa000000000000000000000000000000000000000000000000000000000099";
        let hash_v2 = "bbbb000000000000000000000000000000000000000000000000000000000099";

        // Install foo-1.0 with hash_v1
        insert_trove_with_files(&conn, "foo", "1.0", &[("/usr/bin/foo", hash_v1)]);

        // Create state 1 snapshot (captures hash_v1 in state_cas_hashes)
        let state1 = create_state_with_members(&conn, 1, &[("foo", "1.0")]);

        // Upgrade: delete foo-1.0 first (CASCADE deletes its file rows),
        // then install foo-2.0 at the same path. This mirrors the real
        // upgrade sequence where the old trove is removed before the new
        // one is installed (files.path has a UNIQUE constraint).
        conn.execute(
            "DELETE FROM troves WHERE name = 'foo' AND version = '1.0'",
            [],
        )
        .unwrap();
        insert_trove_with_files(&conn, "foo", "2.0", &[("/usr/bin/foo", hash_v2)]);

        // Create state 2 snapshot (captures hash_v2 in state_cas_hashes)
        let state2 = create_state_with_members(&conn, 2, &[("foo", "2.0")]);

        // Both states survive -- GC must see BOTH hashes
        let hashes = live_cas_hashes(&conn, &[state1, state2]).unwrap();
        assert!(
            hashes.contains(hash_v1),
            "hash_v1 from deleted trove must still be live via state_cas_hashes snapshot"
        );
        assert!(
            hashes.contains(hash_v2),
            "hash_v2 from current trove must be live"
        );
        assert_eq!(hashes.len(), 2);
    }

    #[test]
    fn test_gc_removes_unreferenced() {
        let tmp = TempDir::new().unwrap();
        let objects_dir = tmp.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();

        let live_hash = "aabbccdd00000000000000000000000000000000000000000000000000000001";
        let dead_hash = "ddee00ff00000000000000000000000000000000000000000000000000000002";

        create_cas_object(&objects_dir, live_hash, b"live content");
        create_cas_object(&objects_dir, dead_hash, b"dead content");

        let live_hashes: HashSet<String> = [live_hash.to_string()].into_iter().collect();

        let stats = gc_cas_objects(&objects_dir, &live_hashes).unwrap();

        assert_eq!(stats.objects_checked, 2);
        assert_eq!(stats.objects_removed, 1);
        assert!(stats.bytes_freed > 0);

        // Verify the live object still exists
        let (prefix, suffix) = live_hash.split_at(2);
        assert!(objects_dir.join(prefix).join(suffix).exists());

        // Verify the dead object was removed
        let (prefix, suffix) = dead_hash.split_at(2);
        assert!(!objects_dir.join(prefix).join(suffix).exists());
    }

    #[test]
    fn test_gc_preserves_referenced() {
        let tmp = TempDir::new().unwrap();
        let objects_dir = tmp.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();

        let hash1 = "aa11000000000000000000000000000000000000000000000000000000000001";
        let hash2 = "bb22000000000000000000000000000000000000000000000000000000000002";
        let hash3 = "cc33000000000000000000000000000000000000000000000000000000000003";

        create_cas_object(&objects_dir, hash1, b"content 1");
        create_cas_object(&objects_dir, hash2, b"content 2");
        create_cas_object(&objects_dir, hash3, b"content 3");

        // All three are live
        let live_hashes: HashSet<String> =
            [hash1.to_string(), hash2.to_string(), hash3.to_string()]
                .into_iter()
                .collect();

        let stats = gc_cas_objects(&objects_dir, &live_hashes).unwrap();

        assert_eq!(stats.objects_checked, 3);
        assert_eq!(stats.objects_removed, 0, "No objects should be removed");
        assert_eq!(stats.bytes_freed, 0);

        // Verify all objects survive
        for hash in &[hash1, hash2, hash3] {
            let (prefix, suffix) = hash.split_at(2);
            assert!(
                objects_dir.join(prefix).join(suffix).exists(),
                "Live object {hash} should survive GC"
            );
        }
    }

    #[test]
    fn test_gc_nonexistent_objects_dir() {
        let tmp = TempDir::new().unwrap();
        let objects_dir = tmp.path().join("does-not-exist");

        let live_hashes = HashSet::new();
        let stats = gc_cas_objects(&objects_dir, &live_hashes).unwrap();

        assert_eq!(stats.objects_checked, 0);
        assert_eq!(stats.objects_removed, 0);
        assert_eq!(stats.bytes_freed, 0);
    }

    #[test]
    fn test_gc_skips_temp_files() {
        let tmp = TempDir::new().unwrap();
        let objects_dir = tmp.path().join("objects");
        let prefix_dir = objects_dir.join("ab");
        std::fs::create_dir_all(&prefix_dir).unwrap();

        // Create a temp file that should be skipped
        std::fs::write(prefix_dir.join(".tmp_write_in_progress"), b"temp").unwrap();
        std::fs::write(prefix_dir.join("something.tmp"), b"temp2").unwrap();

        // Create a real dead object
        let dead_hash = "ab00000000000000000000000000000000000000000000000000000000000001";
        create_cas_object(&objects_dir, dead_hash, b"dead");

        let live_hashes = HashSet::new();
        let stats = gc_cas_objects(&objects_dir, &live_hashes).unwrap();

        // Only the real object should be checked and removed
        assert_eq!(stats.objects_checked, 1);
        assert_eq!(stats.objects_removed, 1);

        // Temp files should still exist
        assert!(prefix_dir.join(".tmp_write_in_progress").exists());
        assert!(prefix_dir.join("something.tmp").exists());
    }

    #[test]
    fn test_gc_skips_recently_modified_objects_within_grace_period() {
        let tmp = TempDir::new().unwrap();
        let objects_dir = tmp.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();

        let recent_hash = "ab44000000000000000000000000000000000000000000000000000000000044";
        create_cas_object(&objects_dir, recent_hash, b"recent");

        let live_hashes = HashSet::new();
        let stats = gc_cas_objects(&objects_dir, &live_hashes).unwrap();

        assert_eq!(stats.objects_checked, 1);
        assert_eq!(stats.objects_removed, 0);

        let (prefix, suffix) = recent_hash.split_at(2);
        assert!(
            objects_dir.join(prefix).join(suffix).exists(),
            "recent objects should survive the GC grace period"
        );
    }

    #[test]
    fn test_recent_object_helper_allows_nonrecent_objects() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("object");
        std::fs::write(&path, b"old").unwrap();

        let now = SystemTime::now();
        assert!(!should_skip_recent_object(
            &path,
            now + Duration::from_secs(1),
            Duration::ZERO
        ));
    }
}
