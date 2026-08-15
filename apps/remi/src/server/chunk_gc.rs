// apps/remi/src/server/chunk_gc.rs

//! Chunk garbage collection for the Remi server.
//!
//! Finds orphaned chunks that are no longer referenced by any converted
//! package, then deletes them from local disk, R2 object storage, and
//! the `chunk_access` tracking table. Supports dry-run mode and a
//! configurable grace period to avoid removing chunks that are still
//! being written by in-flight conversions.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::r2::{R2BatchDeleteOutcome, R2Store};

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
    /// Bytes freed on local disk, measured by stat before each unlink.
    ///
    /// A dry run cannot measure this without stat-ing every candidate, so it
    /// reports 0 here and only counts chunks.
    pub local_bytes_freed: u64,
    /// Bytes freed in R2, estimated from `chunk_access.size_bytes`.
    ///
    /// R2 object sizes are not in the listing this GC uses, so the recorded
    /// chunk size is the estimate on both paths. It costs one `chunk_access`
    /// scan either way, so a dry run reports the same estimate for the chunks
    /// it would have deleted.
    pub r2_bytes_freed: u64,
}

/// Build the set of chunk hashes referenced by converted packages or
/// marked as protected in `chunk_access`.
///
/// The referenced set is the union of:
/// 1. All signed object hashes from current package transport envelopes
/// 2. All hashes from `chunk_access WHERE protected = 1`
pub fn build_referenced_set(conn: &Connection) -> Result<HashSet<String>> {
    let mut referenced = HashSet::new();

    // Collect exact signed objects from converted package transports.
    let mut stmt = conn
        .prepare(
            "SELECT id, transport_json
             FROM converted_packages
             WHERE transport_json IS NOT NULL",
        )
        .context("prepare converted_packages query")?;

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .context("query converted_packages transports")?;

    for row in rows {
        let (id, json_str) = row.context("read converted transport_json row")?;
        let transport = serde_json::from_str::<conary_core::ccs::CcsTransportEnvelopeV1>(&json_str)
            .with_context(|| format!("converted package {id} has malformed transport_json"))?;
        for object in transport.objects {
            referenced.insert(object.sha256);
        }
    }

    // Collect hashes from active native publications.
    let mut stmt = conn
        .prepare(
            "SELECT id, transport_json FROM native_package_publications
             WHERE status = 'public' AND transport_json IS NOT NULL",
        )
        .context("prepare native_package_publications chunk query")?;

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .context("query native_package_publications transports")?;

    for row in rows {
        let (id, json_str) = row.context("read native transport_json row")?;
        let transport = serde_json::from_str::<conary_core::ccs::CcsTransportEnvelopeV1>(&json_str)
            .with_context(|| {
                format!("native package publication {id} has malformed transport_json")
            })?;
        for object in transport.objects {
            referenced.insert(object.sha256);
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

        if conary_core::filesystem::is_temporary_object_name(entry.file_name()) {
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

/// Hashes whose bytes count as freed from R2: everything submitted to the bulk
/// delete except the keys R2 refused.
///
/// Deletion is idempotent, so a key R2 did not report as an error is gone
/// whether or not the object existed -- the same rule `R2BatchDeleteOutcome`
/// counts by.
fn r2_freed_hashes(submitted: &[String], failed: &BTreeSet<String>) -> HashSet<String> {
    submitted
        .iter()
        .filter(|hash| !failed.contains(*hash))
        .cloned()
        .collect()
}

/// Sum the recorded `chunk_access.size_bytes` of `hashes`.
///
/// The table is streamed once and filtered in memory rather than bound one host
/// variable per hash: a production orphan set is an order of magnitude past
/// SQLite's ~32,766 parameter limit, the same wall the grace query hit. Cost is
/// one scan of `chunk_access` however many hashes are being priced, and the
/// running total is a single integer.
///
/// A hash with no `chunk_access` row contributes nothing; its size was never
/// recorded, so the result is an underestimate rather than a guess.
fn sum_chunk_access_bytes(conn: &Connection, hashes: &HashSet<String>) -> Result<u64> {
    if hashes.is_empty() {
        return Ok(0);
    }

    let mut stmt = conn
        .prepare("SELECT hash, size_bytes FROM chunk_access")
        .context("prepare chunk_access size query")?;
    let mut rows = stmt.query([]).context("query chunk_access sizes")?;

    let mut total = 0u64;
    while let Some(row) = rows.next().context("read chunk_access size row")? {
        let hash: String = row.get(0).context("read chunk_access size row hash")?;
        if !hashes.contains(&hash) {
            continue;
        }
        let size: i64 = row.get(1).context("read chunk_access size row size")?;
        total += u64::try_from(size).unwrap_or(0);
    }

    Ok(total)
}

/// Estimate the R2 bytes represented by `hashes` from the `chunk_access` table.
///
/// Callers must run this before `chunk_access` rows are deleted: the sizes only
/// exist while the rows do.
async fn estimate_r2_bytes_freed(db_path: &Path, hashes: HashSet<String>) -> Result<u64> {
    if hashes.is_empty() {
        return Ok(0);
    }

    let db_path = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<u64> {
        let conn = crate::server::open_runtime_db(&db_path)?;
        sum_chunk_access_bytes(&conn, &hashes)
    })
    .await
    .context("spawn_blocking for R2 freed-bytes estimate")?
    .context("R2 freed-bytes estimate")
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
/// 5. Deletes orphans from local disk, R2, and the `chunk_access` table, in
///    that order: the R2 freed-bytes estimate reads `chunk_access.size_bytes`,
///    which only exists until the row cleanup runs. A key R2 refused keeps its
///    row so the next run can still price the retry.
/// 6. In `dry_run` mode, logs what would be deleted without making changes.
pub async fn run_chunk_gc(
    db_path: &Path,
    objects_dir: &Path,
    r2_store: Option<Arc<R2Store>>,
    dry_run: bool,
    grace_period_secs: u64,
) -> Result<GcResult> {
    let Some(store) = r2_store else {
        return run_chunk_gc_with_deleter(
            db_path,
            objects_dir,
            None,
            |_| async {
                Err(anyhow::anyhow!(
                    "chunk GC submitted an R2 delete without an R2 listing"
                ))
            },
            dry_run,
            grace_period_secs,
        )
        .await;
    };

    let r2_chunks = store.list_chunks().await.context("list R2 chunks")?;
    run_chunk_gc_with_deleter(
        db_path,
        objects_dir,
        Some(r2_chunks),
        move |hashes: Arc<Vec<String>>| async move {
            store
                .delete_chunks(&hashes)
                .await
                .context("bulk delete R2 orphan chunks")
        },
        dry_run,
        grace_period_secs,
    )
    .await
}

/// Run chunk garbage collection against an already-listed R2 key set and an
/// injected bulk deleter.
///
/// The deleter is a parameter so a test can drive the real run against an R2
/// that refuses keys; production passes `R2Store::delete_chunks`. It is handed
/// an `Arc` of the submitted keys because the caller still needs that set
/// afterwards to decide which chunks are actually gone, and a production run
/// submits hundreds of thousands of them.
async fn run_chunk_gc_with_deleter<F, Fut>(
    db_path: &Path,
    objects_dir: &Path,
    r2_chunks: Option<Vec<String>>,
    delete_r2: F,
    dry_run: bool,
    grace_period_secs: u64,
) -> Result<GcResult>
where
    F: FnOnce(Arc<Vec<String>>) -> Fut,
    Fut: Future<Output = Result<R2BatchDeleteOutcome>>,
{
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

    // Step 3: Account for the R2 listing, when R2 is configured at all.
    let r2_chunks = r2_chunks.unwrap_or_default();
    result.r2_scanned = r2_chunks.len();

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

    // The keys the bulk delete below submits. Built once so a dry run reports
    // exactly the set a real run would remove, rather than a second expression
    // that has to be kept in step with it. Without an R2 listing `r2_set` is
    // empty, so this is empty too and the deleter is never called.
    let r2_orphans: Arc<Vec<String>> = Arc::new(
        orphans_after_grace
            .iter()
            .filter(|h| r2_set.contains(h.as_str()))
            .cloned()
            .collect(),
    );

    if dry_run {
        // Populate counts for reporting even in dry-run
        result.local_deleted = orphans_after_grace
            .iter()
            .filter(|h| local_set.contains(h.as_str()))
            .count();
        result.r2_deleted = r2_orphans.len();
        // Nothing has been refused yet, so a dry run prices every submitted key.
        result.r2_bytes_freed =
            estimate_r2_bytes_freed(db_path, r2_freed_hashes(&r2_orphans, &BTreeSet::new()))
                .await?;

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

    // Step 6: Delete orphans.
    //
    // Local unlinks stay one at a time, but the R2 orphans collected above are
    // removed in one bulk pass below. One HTTP round trip per orphan turned the
    // first production run's 345,913 R2 orphans into hours of serial deletes.
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
    }

    // Bulk-delete the R2 orphans in one pass.
    let mut r2_refused: BTreeSet<String> = BTreeSet::new();
    if !r2_orphans.is_empty() {
        let outcome = delete_r2(Arc::clone(&r2_orphans)).await?;
        result.r2_deleted += outcome.deleted;
        if outcome.failed() > 0 {
            tracing::warn!(
                "Failed to delete {} of {} R2 orphan chunk(s); samples: {:?}",
                outcome.failed(),
                outcome.attempted,
                outcome.failure_samples
            );
        }

        // Price the delete before the cleanup below drops the rows that hold
        // the sizes. Keys R2 refused are still in the bucket, so their bytes
        // were not freed.
        result.r2_bytes_freed = estimate_r2_bytes_freed(
            db_path,
            r2_freed_hashes(&r2_orphans, &outcome.failed_hashes),
        )
        .await?;
        r2_refused = outcome.failed_hashes;
    }

    // Delete chunk_access rows for the orphans that are actually gone.
    //
    // A key R2 refused still has its object in the bucket, and that row holds
    // the only record of its size. Orphan detection is listing-based, so the
    // next run re-lists the surviving object and retries the delete -- but
    // without the row it could not price what it freed, and `r2_bytes_freed`
    // would silently under-report every retry. A failed local unlink is not
    // excluded here: local bytes are measured by `stat` at unlink rather than
    // read from the row, so the row carries nothing that path needs.
    let db_path_cleanup = db_path.to_path_buf();
    let deleted_hashes: Vec<String> = if r2_refused.is_empty() {
        orphans_after_grace
    } else {
        orphans_after_grace
            .into_iter()
            .filter(|hash| !r2_refused.contains(hash))
            .collect()
    };
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
        use conary_core::db::models::ConvertedPackage;

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();

        let first_transport = crate::server::conversion::test_support::test_transport(&[
            "hash_a".to_string(),
            "hash_b".to_string(),
            "hash_c".to_string(),
        ]);
        let mut first = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
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
        first.insert(&conn).unwrap();

        let second_transport = crate::server::conversion::test_support::test_transport(&[
            "hash_b".to_string(),
            "hash_d".to_string(),
        ]);
        let mut second = ConvertedPackage::new_repository(
            "ubuntu-26.04".to_string(),
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
        second.insert(&conn).unwrap();

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
        use conary_core::db::models::ConvertedPackage;

        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        let transport =
            crate::server::conversion::test_support::test_transport(&["hash".to_string()]);
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
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
        let id = converted.insert(&conn).unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        conn.execute(
            "UPDATE converted_packages SET transport_json = '{bad' WHERE id = ?1",
            [id],
        )
        .unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")
            .unwrap();

        let error = build_referenced_set(&conn).unwrap_err().to_string();
        assert!(
            error.contains(&format!(
                "converted package {id} has malformed transport_json"
            )),
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
}
