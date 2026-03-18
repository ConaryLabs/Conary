// conary-core/src/generation/gc.rs

//! Generation and CAS garbage collection.
//!
//! In the composefs-native model, CAS object liveness is determined by which
//! generations are kept. A CAS object is live if any surviving generation's
//! state_members reference a trove whose files include that object's hash.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::Connection;
use tracing::{debug, info};

/// Statistics from a CAS garbage collection run.
#[derive(Debug, Clone, Default)]
pub struct GcStats {
    /// Total CAS objects inspected.
    pub objects_checked: u64,
    /// CAS objects removed (unreferenced).
    pub objects_removed: u64,
    /// Bytes freed by removing unreferenced objects.
    pub bytes_freed: u64,
}

/// Get the set of CAS hashes referenced by surviving generations.
///
/// Queries the database for all distinct `sha256_hash` values from files
/// belonging to troves that are members of the given state snapshots.
pub fn live_cas_hashes(
    conn: &Connection,
    surviving_state_ids: &[i64],
) -> crate::Result<HashSet<String>> {
    if surviving_state_ids.is_empty() {
        return Ok(HashSet::new());
    }

    // Build a parameterized IN clause. rusqlite doesn't natively support
    // binding a slice, so we construct placeholders manually.
    let placeholders: Vec<String> = surviving_state_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        "SELECT DISTINCT f.sha256_hash FROM files f \
         JOIN troves t ON f.trove_id = t.id \
         JOIN state_members sm ON sm.trove_name = t.name AND sm.trove_version = t.version \
         WHERE sm.state_id IN ({in_clause})"
    );

    let mut stmt = conn.prepare(&sql)?;

    // Bind each state_id parameter
    let params: Vec<Box<dyn rusqlite::types::ToSql>> = surviving_state_ids
        .iter()
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), |row| row.get::<_, String>(0))?;

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
/// Walks the objects directory (two-level layout: `{prefix}/{suffix}` where
/// the full hash is `{prefix}{suffix}`) and deletes any object whose
/// reconstructed hash is not in `live_hashes`.
pub fn gc_cas_objects(objects_dir: &Path, live_hashes: &HashSet<String>) -> crate::Result<GcStats> {
    let mut stats = GcStats::default();

    if !objects_dir.exists() {
        info!("CAS objects directory does not exist, nothing to collect");
        return Ok(stats);
    }

    // Walk the two-level directory: objects/{2-char-prefix}/{rest-of-hash}
    let prefix_entries = std::fs::read_dir(objects_dir)?;

    for prefix_entry in prefix_entries {
        let prefix_entry = prefix_entry?;
        let prefix_path = prefix_entry.path();

        // Skip non-directories and special files (e.g., lock files)
        if !prefix_entry.file_type()?.is_dir() {
            continue;
        }

        let prefix_name = prefix_entry.file_name();
        let prefix_str = prefix_name.to_string_lossy();

        // Prefix directories should be exactly 2 hex characters
        if prefix_str.len() != 2 {
            continue;
        }

        let suffix_entries = std::fs::read_dir(&prefix_path)?;

        for suffix_entry in suffix_entries {
            let suffix_entry = suffix_entry?;
            let suffix_path = suffix_entry.path();

            // Skip directories and temp files
            if !suffix_entry.file_type()?.is_file() {
                continue;
            }

            let suffix_name = suffix_entry.file_name();
            let suffix_str = suffix_name.to_string_lossy();

            // Skip temp files (used during atomic writes)
            if suffix_str.starts_with('.') || suffix_str.ends_with(".tmp") {
                continue;
            }

            // Reconstruct the full hash: prefix + suffix
            let full_hash = format!("{prefix_str}{suffix_str}");

            stats.objects_checked += 1;

            if !live_hashes.contains(&full_hash) {
                // Get size before removing
                if let Ok(metadata) = suffix_path.metadata() {
                    stats.bytes_freed += metadata.len();
                }

                match std::fs::remove_file(&suffix_path) {
                    Ok(()) => {
                        stats.objects_removed += 1;
                        debug!("Removed unreferenced CAS object: {full_hash}");
                    }
                    Err(e) => {
                        tracing::warn!("Failed to remove CAS object {full_hash}: {e}");
                    }
                }
            }
        }

        // Clean up empty prefix directories
        if prefix_path.read_dir().is_ok_and(|mut d| d.next().is_none()) {
            let _ = std::fs::remove_dir(&prefix_path);
        }
    }

    info!(
        "CAS GC: checked {}, removed {}, freed {} bytes",
        stats.objects_checked, stats.objects_removed, stats.bytes_freed
    );

    Ok(stats)
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
}
