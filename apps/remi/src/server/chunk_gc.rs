// apps/remi/src/server/chunk_gc.rs

//! Chunk garbage collection for the Remi server.
//!
//! Finds orphaned chunks that are no longer referenced by any converted
//! package, then deletes them from local disk, R2 object storage, and
//! the `chunk_access` tracking table. Supports dry-run mode and a
//! configurable grace period to avoid removing chunks that are still
//! being written by in-flight conversions.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::r2::R2Store;

/// Result of a garbage collection run.
#[derive(Debug, Clone, Default)]
pub struct GcResult {
    /// Number of chunks scanned on local disk
    pub local_scanned: usize,
    /// Number of chunks scanned in R2
    pub r2_scanned: usize,
    /// Number of chunks in the referenced set
    pub referenced: usize,
    /// Number of chunks deleted from local disk
    pub local_deleted: usize,
    /// Number of chunks deleted from R2
    pub r2_deleted: usize,
    /// Bytes freed on local disk
    pub local_bytes_freed: u64,
    /// Bytes freed in R2 (estimated from chunk_access size_bytes)
    pub r2_bytes_freed: u64,
}

/// Build the set of chunk hashes referenced by converted packages or
/// marked as protected in `chunk_access`.
///
/// The referenced set is the union of:
/// 1. All hashes from `converted_packages.chunk_hashes_json` columns
/// 2. All hashes from `chunk_access WHERE protected = 1`
pub fn build_referenced_set(conn: &Connection) -> Result<HashSet<String>> {
    let mut referenced = HashSet::new();

    // Collect hashes from converted_packages.chunk_hashes_json
    let mut stmt = conn
        .prepare(
            "SELECT id, chunk_hashes_json
             FROM converted_packages
             WHERE chunk_hashes_json IS NOT NULL",
        )
        .context("prepare converted_packages query")?;

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .context("query converted_packages chunk hashes")?;

    for row in rows {
        let (id, json_str) = row.context("read converted chunk_hashes_json row")?;
        let hashes = serde_json::from_str::<Vec<String>>(&json_str)
            .with_context(|| format!("converted package {id} has malformed chunk_hashes_json"))?;
        for hash in hashes {
            referenced.insert(hash);
        }
    }

    // Collect hashes from active native publications.
    let mut stmt = conn
        .prepare(
            "SELECT id, chunk_hashes_json FROM native_package_publications
             WHERE status = 'public' AND chunk_hashes_json IS NOT NULL",
        )
        .context("prepare native_package_publications chunk query")?;

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .context("query native_package_publications chunk hashes")?;

    for row in rows {
        let (id, json_str) = row.context("read native chunk_hashes_json row")?;
        let hashes = serde_json::from_str::<Vec<String>>(&json_str).with_context(|| {
            format!("native package publication {id} has malformed chunk_hashes_json")
        })?;
        for hash in hashes {
            referenced.insert(hash);
        }
    }

    // Collect protected chunk hashes from chunk_access
    let mut stmt = conn
        .prepare("SELECT hash FROM chunk_access WHERE protected = 1")
        .context("prepare chunk_access protected query")?;

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .context("query protected chunks")?;

    for row in rows {
        let hash = row.context("read protected chunk hash")?;
        referenced.insert(hash);
    }

    Ok(referenced)
}

/// Walk the two-level CAS directory structure and return all chunk hashes.
///
/// Directory layout: `{objects_dir}/{hash[0:2]}/{hash[2:]}`.
/// Skips `.tmp` files (incomplete writes).
pub fn scan_local_chunks(objects_dir: &Path) -> Result<Vec<String>> {
    let mut hashes = Vec::new();

    if !objects_dir.exists() {
        return Ok(hashes);
    }

    let walker = walkdir::WalkDir::new(objects_dir).min_depth(2).max_depth(2);

    for entry in walker {
        let entry = entry.context("walk chunk directory")?;
        if !entry.file_type().is_file() {
            continue;
        }

        // Skip .tmp files (in-flight writes)
        if entry.path().extension().is_some_and(|ext| ext == "tmp") {
            continue;
        }

        if let Some(hash) = extract_hash_from_path(entry.path()) {
            hashes.push(hash);
        }
    }

    Ok(hashes)
}

/// Extract a chunk hash from its two-level path.
///
/// `{objects_dir}/ab/cdef0123...` -> `"abcdef0123..."`
fn extract_hash_from_path(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    let prefix = parent.file_name()?.to_str()?;
    Some(format!("{prefix}{file_name}"))
}

/// Build the local filesystem path for a chunk hash.
///
/// `{objects_dir}/{hash[0:2]}/{hash[2:]}`
fn chunk_path(objects_dir: &Path, hash: &str) -> PathBuf {
    let (prefix, rest) = hash.split_at(2.min(hash.len()));
    objects_dir.join(prefix).join(rest)
}

/// Return the orphan candidates that fall outside the grace period.
///
/// A candidate is skipped only when `chunk_access` records a `last_accessed`
/// strictly newer than `cutoff`. Timestamps use the sortable
/// `%Y-%m-%d %H:%M:%S` format, so the comparison is lexicographic in SQLite
/// exactly as it was in Rust. (`last_accessed` is declared NOT NULL; a
/// hypothetical NULL would compare as not-recent and be collected.)
///
/// The recent set is loaded with a single bound parameter so query size stays
/// independent of the orphan count. Binding one host variable per candidate
/// exceeds SQLite's ~32,766 parameter limit once a store holds tens of
/// thousands of orphans, which failed the prepare and aborted the whole run.
/// The in-memory recent set is bounded by rows accessed inside the grace
/// window — small next to the orphan and local-scan sets already held here.
fn filter_recently_accessed(
    conn: &Connection,
    orphans: &[String],
    cutoff: &str,
) -> Result<Vec<String>> {
    if orphans.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = conn
        .prepare("SELECT hash FROM chunk_access WHERE last_accessed > ?1")
        .context("prepare chunk_access grace period query")?;
    let mut rows = stmt
        .query([cutoff])
        .context("query recently-accessed chunks")?;

    let mut recent: HashSet<String> = HashSet::new();
    while let Some(row) = rows.next().context("read recently-accessed chunk row")? {
        recent.insert(row.get(0).context("read recently-accessed chunk hash")?);
    }

    let mut collected = Vec::with_capacity(orphans.len());
    for hash in orphans {
        if recent.contains(hash) {
            continue;
        }
        collected.push(hash.clone());
    }

    let skipped = orphans.len() - collected.len();
    if skipped > 0 {
        tracing::debug!(
            "Keeping {} recently-accessed orphan chunk(s) (last_accessed newer than {})",
            skipped,
            cutoff
        );
    }

    Ok(collected)
}

/// Delete `chunk_access` rows for `hashes` in batched explicit transactions.
///
/// Returns the number of rows actually removed. A per-row failure warns and
/// continues so one bad hash cannot abandon the rest of its batch.
fn delete_chunk_access_rows(conn: &mut Connection, hashes: &[String]) -> Result<usize> {
    /// Rows per commit: one implicit transaction per hash costs one fsync each.
    const BATCH_SIZE: usize = 10_000;

    let mut deleted = 0usize;

    for batch in hashes.chunks(BATCH_SIZE) {
        let tx = conn
            .transaction()
            .context("begin chunk_access cleanup transaction")?;
        {
            let mut stmt = tx
                .prepare("DELETE FROM chunk_access WHERE hash = ?1")
                .context("prepare chunk_access delete")?;
            for hash in batch {
                match stmt.execute([hash]) {
                    Ok(rows) => deleted += rows,
                    Err(e) => {
                        tracing::warn!("Failed to delete chunk_access row for {}: {}", hash, e);
                    }
                }
            }
        }
        tx.commit()
            .context("commit chunk_access cleanup transaction")?;
    }

    Ok(deleted)
}

/// Run chunk garbage collection.
///
/// 1. Builds the referenced set from `converted_packages` and protected `chunk_access` rows.
/// 2. Scans local disk (and optionally R2) for stored chunks.
/// 3. Identifies orphans (stored but not referenced).
/// 4. Applies a grace period: chunks with `last_accessed` newer than `now - grace_period_secs`
///    are kept even if unreferenced, to avoid removing chunks for in-flight conversions.
/// 5. Deletes orphans from local disk, R2, and the `chunk_access` table.
/// 6. In `dry_run` mode, logs what would be deleted without making changes.
pub async fn run_chunk_gc(
    db_path: &Path,
    objects_dir: &Path,
    r2_store: Option<Arc<R2Store>>,
    dry_run: bool,
    grace_period_secs: u64,
) -> Result<GcResult> {
    let mut result = GcResult::default();

    // Step 1: Build referenced set (blocking DB work)
    // Each spawn_blocking task opens its own DB connection because
    // rusqlite::Connection is !Send and can't cross await points.
    let db_path_owned = db_path.to_path_buf();
    let referenced = tokio::task::spawn_blocking(move || -> Result<HashSet<String>> {
        let conn = crate::server::open_runtime_db(&db_path_owned)?;
        build_referenced_set(&conn)
    })
    .await
    .context("spawn_blocking for build_referenced_set")?
    .context("build_referenced_set")?;
    result.referenced = referenced.len();

    // Step 2: Scan local disk (blocking I/O)
    let objects_dir_owned = objects_dir.to_path_buf();
    let local_chunks = tokio::task::spawn_blocking(move || scan_local_chunks(&objects_dir_owned))
        .await
        .context("spawn_blocking for scan_local_chunks")?
        .context("scan_local_chunks")?;
    result.local_scanned = local_chunks.len();

    // Step 3: Optionally list R2 chunks
    let r2_chunks = if let Some(ref store) = r2_store {
        let chunks = store.list_chunks().await.context("list R2 chunks")?;
        result.r2_scanned = chunks.len();
        chunks
    } else {
        Vec::new()
    };

    // Step 4: Find orphans (stored but not referenced)
    let local_set: HashSet<&str> = local_chunks.iter().map(String::as_str).collect();
    let r2_set: HashSet<&str> = r2_chunks.iter().map(String::as_str).collect();
    let mut all_stored: HashSet<&str> =
        HashSet::with_capacity(local_chunks.len() + r2_chunks.len());
    for h in &local_chunks {
        all_stored.insert(h.as_str());
    }
    for h in &r2_chunks {
        all_stored.insert(h.as_str());
    }

    let orphan_candidates: Vec<&str> = all_stored
        .iter()
        .filter(|h| !referenced.contains(**h))
        .copied()
        .collect();

    // Step 5: Apply grace period -- skip recently-accessed chunks
    let db_path_grace = db_path.to_path_buf();
    let orphan_strings: Vec<String> = orphan_candidates.iter().map(|s| (*s).to_string()).collect();
    let grace = grace_period_secs;

    let orphans_after_grace: Vec<String> =
        tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let conn = crate::server::open_runtime_db(&db_path_grace)?;
            let cutoff = chrono::Utc::now() - chrono::Duration::seconds(grace as i64);
            let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();
            filter_recently_accessed(&conn, &orphan_strings, &cutoff_str)
        })
        .await
        .context("spawn_blocking for grace period check")?
        .context("grace period check")?;

    if dry_run {
        // Populate counts for reporting even in dry-run
        result.local_deleted = orphans_after_grace
            .iter()
            .filter(|h| local_set.contains(h.as_str()))
            .count();
        result.r2_deleted = orphans_after_grace
            .iter()
            .filter(|h| r2_set.contains(h.as_str()))
            .count();

        // Summarize rather than logging a line per orphan: a production store
        // has millions of them.
        tracing::info!(
            "[DRY RUN] Would delete {} orphan chunk(s): {} on local disk, {} in R2",
            orphans_after_grace.len(),
            result.local_deleted,
            result.r2_deleted
        );
        for hash in orphans_after_grace.iter().take(10) {
            tracing::debug!(
                "[DRY RUN] Orphan chunk sample {} (local={}, r2={})",
                hash,
                local_set.contains(hash.as_str()),
                r2_set.contains(hash.as_str())
            );
        }
        return Ok(result);
    }

    // Step 6: Delete orphans
    for hash in &orphans_after_grace {
        // Delete from local disk
        if local_set.contains(hash.as_str()) {
            let path = chunk_path(objects_dir, hash);
            match tokio::fs::metadata(&path).await {
                Ok(meta) => {
                    let size = meta.len();
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        tracing::warn!("Failed to delete local chunk {}: {}", hash, e);
                        continue;
                    }
                    // Count bytes only once the unlink actually succeeded.
                    result.local_bytes_freed += size;
                    result.local_deleted += 1;

                    // Try to remove the parent prefix directory if empty
                    if let Some(parent) = path.parent() {
                        let _ = tokio::fs::remove_dir(parent).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to stat local chunk {}: {}", hash, e);
                }
            }
        }

        // Delete from R2
        if r2_set.contains(hash.as_str())
            && let Some(ref store) = r2_store
        {
            match store.delete_chunk(hash).await {
                Ok(_) => {
                    result.r2_deleted += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to delete R2 chunk {}: {}", hash, e);
                }
            }
        }
    }

    // Delete chunk_access rows for all deleted orphans (blocking DB)
    let db_path_cleanup = db_path.to_path_buf();
    let deleted_hashes = orphans_after_grace.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut conn = crate::server::open_runtime_db(&db_path_cleanup)?;
        let removed = delete_chunk_access_rows(&mut conn, &deleted_hashes)?;
        tracing::debug!("Removed {} chunk_access row(s)", removed);
        Ok(())
    })
    .await
    .context("spawn_blocking for chunk_access cleanup")?
    .context("chunk_access cleanup")?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // Create a .tmp file that should be skipped
        std::fs::write(prefix_dir.join("incomplete.tmp"), b"partial").unwrap();

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
        use conary_core::db::models::ConvertedPackage;

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();

        let mut first = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            "first".to_string(),
            "1".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            "sha256:test1".to_string(),
            &[
                "hash_a".to_string(),
                "hash_b".to_string(),
                "hash_c".to_string(),
            ],
            3,
            "sha256:first".to_string(),
            "/tmp/first.ccs".to_string(),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        first.insert(&conn).unwrap();

        let mut second = ConvertedPackage::new_repository(
            "ubuntu-26.04".to_string(),
            "second".to_string(),
            "1".to_string(),
            "amd64".to_string(),
            "deb".to_string(),
            "sha256:test2".to_string(),
            &["hash_b".to_string(), "hash_d".to_string()],
            2,
            "sha256:second".to_string(),
            "/tmp/second.ccs".to_string(),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        second.insert(&conn).unwrap();

        // Insert a protected chunk_access row
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
                chunk_hashes_json, total_size, package_path, target_path, trust_status
            ) VALUES (1, 1, 'fedora-44', 'hello', '1.0.0', '1', 'noarch', 'package', 2,
                      'public', 'native-content', '[\"native-chunk\"]', 42,
                      '/tmp/hello.ccs', 'packages/fedora/hello.ccs', 'verified')",
            [],
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
    fn referenced_set_rejects_malformed_converted_chunk_authority() {
        use conary_core::db::models::ConvertedPackage;

        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            "broken".to_string(),
            "1".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            "sha256:source".to_string(),
            &["hash".to_string()],
            1,
            "sha256:content".to_string(),
            "/tmp/broken.ccs".to_string(),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        let id = converted.insert(&conn).unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        conn.execute(
            "UPDATE converted_packages SET chunk_hashes_json = '{bad' WHERE id = ?1",
            [id],
        )
        .unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")
            .unwrap();

        let error = build_referenced_set(&conn).unwrap_err().to_string();
        assert!(
            error.contains(&format!(
                "converted package {id} has malformed chunk_hashes_json"
            )),
            "{error}"
        );
    }

    #[test]
    fn referenced_set_rejects_malformed_public_native_chunk_authority() {
        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
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
                chunk_hashes_json, total_size, package_path, target_path, trust_status
            ) VALUES (1, 1, 'fedora-44', 'broken', '1', '1', 'noarch', 'package', 2,
                      'public', 'sha256:broken', '{bad', 1,
                      '/tmp/broken.ccs', 'packages/fedora/broken.ccs', 'verified')",
            [],
        )
        .unwrap();
        let id = conn.last_insert_rowid();

        let error = build_referenced_set(&conn).unwrap_err().to_string();
        assert!(
            error.contains(&format!(
                "native package publication {id} has malformed chunk_hashes_json"
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
}
