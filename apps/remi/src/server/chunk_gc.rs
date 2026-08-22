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
use conary_core::db::models::ConvertedPackage;
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

    // Repository conversions are roots only when their exact conversion
    // algorithm and durable profile-revision pin are current. Stale rows are
    // not roots; a current row with a missing or mismatched pin is corruption
    // and aborts the collection rather than weakening the live set.
    for converted in ConvertedPackage::list_repository_conversions(conn)? {
        if !converted.repository_conversion_is_current()? {
            continue;
        }
        let id = converted
            .id
            .ok_or_else(|| anyhow::anyhow!("current converted repository row has no ID"))?;
        ConvertedPackage::require_conversion_pin(conn, id)
            .with_context(|| format!("validate conversion pin for repository row {id}"))?;
        converted
            .scriptlet_summary()
            .with_context(|| format!("validate lifecycle summary for repository row {id}"))?;
        for hash in converted.object_hashes()? {
            referenced.insert(hash);
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
mod tests;
