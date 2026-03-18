// conary-server/src/server/chunk_gc.rs

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
            "SELECT chunk_hashes_json FROM converted_packages WHERE chunk_hashes_json IS NOT NULL",
        )
        .context("prepare converted_packages query")?;

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .context("query converted_packages chunk hashes")?;

    for row in rows {
        let json_str = row.context("read chunk_hashes_json row")?;
        if let Ok(hashes) = serde_json::from_str::<Vec<String>>(&json_str) {
            for hash in hashes {
                referenced.insert(hash);
            }
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
        let conn = conary_core::db::open(&db_path_owned)?;
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
            let conn = conary_core::db::open(&db_path_grace)?;
            let cutoff = chrono::Utc::now() - chrono::Duration::seconds(grace as i64);
            let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();

            // Batch-load all chunk_access entries into a HashMap to avoid
            // one query per orphan candidate.
            let mut access_map: std::collections::HashMap<String, Option<String>> =
                std::collections::HashMap::new();
            if !orphan_strings.is_empty() {
                let placeholders = orphan_strings
                    .iter()
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!(
                    "SELECT hash, last_accessed FROM chunk_access WHERE hash IN ({})",
                    placeholders
                );
                let mut stmt = conn.prepare(&sql)?;
                let params: Vec<&dyn rusqlite::types::ToSql> = orphan_strings
                    .iter()
                    .map(|s| s as &dyn rusqlite::types::ToSql)
                    .collect();
                let mut rows = stmt.query(params.as_slice())?;
                while let Some(row) = rows.next()? {
                    let hash: String = row.get(0)?;
                    let last_accessed: Option<String> = row.get(1)?;
                    access_map.insert(hash, last_accessed);
                }
            }

            let mut kept = Vec::new();
            for hash in &orphan_strings {
                // Check if this chunk was recently accessed.
                // ISO 8601 timestamps are lexicographically sortable, so string
                // comparison is correct here.
                if let Some(Some(last)) = access_map.get(hash)
                    && last.as_str() > cutoff_str.as_str()
                {
                    tracing::debug!(
                        "Keeping recently-accessed orphan chunk {} (last_accessed: {})",
                        hash,
                        last
                    );
                    continue;
                }
                kept.push(hash.clone());
            }
            Ok(kept)
        })
        .await
        .context("spawn_blocking for grace period check")?
        .context("grace period check")?;

    if dry_run {
        // Log what would be deleted without making changes
        for hash in &orphans_after_grace {
            let on_local = local_set.contains(hash.as_str());
            let on_r2 = r2_set.contains(hash.as_str());
            tracing::info!(
                "[DRY RUN] Would delete orphan chunk {} (local={}, r2={})",
                hash,
                on_local,
                on_r2
            );
        }
        // Populate counts for reporting even in dry-run
        result.local_deleted = orphans_after_grace
            .iter()
            .filter(|h| local_set.contains(h.as_str()))
            .count();
        result.r2_deleted = orphans_after_grace
            .iter()
            .filter(|h| r2_set.contains(h.as_str()))
            .count();
        return Ok(result);
    }

    // Step 6: Delete orphans
    for hash in &orphans_after_grace {
        // Delete from local disk
        if local_set.contains(hash.as_str()) {
            let path = chunk_path(objects_dir, hash);
            match tokio::fs::metadata(&path).await {
                Ok(meta) => {
                    result.local_bytes_freed += meta.len();
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        tracing::warn!("Failed to delete local chunk {}: {}", hash, e);
                        continue;
                    }
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
        let conn = conary_core::db::open(&db_path_cleanup)?;
        for hash in &deleted_hashes {
            if let Err(e) = conary_core::db::models::ChunkAccess::delete(&conn, hash) {
                tracing::warn!("Failed to delete chunk_access row for {}: {}", hash, e);
            }
        }
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
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();

        // Insert a converted package with chunk_hashes_json
        conn.execute(
            "INSERT INTO converted_packages (original_format, original_checksum, conversion_version, conversion_fidelity, chunk_hashes_json)
             VALUES ('rpm', 'sha256:test1', 1, 'high', ?1)",
            [r#"["hash_a","hash_b","hash_c"]"#],
        )
        .unwrap();

        // Insert another with different hashes
        conn.execute(
            "INSERT INTO converted_packages (original_format, original_checksum, conversion_version, conversion_fidelity, chunk_hashes_json)
             VALUES ('deb', 'sha256:test2', 1, 'high', ?1)",
            [r#"["hash_b","hash_d"]"#],
        )
        .unwrap();

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

        let referenced = build_referenced_set(&conn).unwrap();

        assert!(referenced.contains("hash_a"));
        assert!(referenced.contains("hash_b"));
        assert!(referenced.contains("hash_c"));
        assert!(referenced.contains("hash_d"));
        assert!(referenced.contains("hash_e")); // protected
        assert!(!referenced.contains("hash_f")); // not protected, not in any package
        assert_eq!(referenced.len(), 5);
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
