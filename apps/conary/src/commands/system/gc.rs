// conary/src/commands/system/gc.rs

use super::*;

/// Garbage collect unreferenced files from CAS storage.
///
/// This removes files from the content-addressable store that are no longer
/// referenced by any installed package or recent file history (for rollback).
pub async fn cmd_gc(
    db_path: &str,
    objects_dir: &str,
    keep_days: u32,
    dry_run: bool,
    chunks: bool,
) -> Result<()> {
    use std::collections::HashSet;
    use std::fs;

    info!(
        "Starting CAS garbage collection (keep_days={}, dry_run={})",
        keep_days, dry_run
    );

    let conn = open_db(db_path)?;
    let objects_path = Path::new(objects_dir);

    if !objects_path.exists() {
        println!("CAS directory does not exist: {}", objects_dir);
        return Ok(());
    }

    // Step 1: Collect all referenced hashes from installed files
    println!("Collecting referenced hashes from installed packages...");
    let mut referenced_hashes: HashSet<String> = HashSet::new();

    let file_hashes: Vec<String> = {
        let mut stmt = conn.prepare("SELECT DISTINCT sha256_hash FROM files WHERE sha256_hash IS NOT NULL AND sha256_hash != ''")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for hash in file_hashes {
        referenced_hashes.insert(hash);
    }
    println!(
        "  Found {} hashes from installed files",
        referenced_hashes.len()
    );

    // Step 2: Collect hashes from file_history within retention period
    println!(
        "Collecting hashes from recent file history ({}+ days)...",
        keep_days
    );
    let history_hashes: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT fh.sha256_hash FROM file_history fh
             JOIN changesets c ON fh.changeset_id = c.id
             WHERE fh.sha256_hash IS NOT NULL AND fh.sha256_hash != ''
             AND c.applied_at >= datetime('now', ?1)",
        )?;
        let days_param = format!("-{} days", keep_days);
        let rows = stmt.query_map([days_param], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for hash in history_hashes {
        referenced_hashes.insert(hash);
    }
    println!("  Total referenced hashes: {}", referenced_hashes.len());

    // Step 3: Scan CAS directory for all stored objects
    println!("Scanning CAS directory for objects...");
    let mut cas_objects: Vec<(PathBuf, String)> = Vec::new();
    let mut total_cas_size: u64 = 0;

    let cas = CasStore::new(objects_path)?;
    for result in cas.iter_objects() {
        let (hash, path) = result?;
        let metadata = fs::metadata(&path)?;
        total_cas_size += metadata.len();
        cas_objects.push((path, hash));
    }
    println!(
        "  Found {} objects in CAS ({} total)",
        cas_objects.len(),
        format_bytes(total_cas_size)
    );

    // Step 4: Find unreferenced objects
    let unreferenced: Vec<(PathBuf, String)> = cas_objects
        .into_iter()
        .filter(|(_, hash)| !referenced_hashes.contains(hash))
        .collect();

    if unreferenced.is_empty() {
        println!("\nNo unreferenced objects found. CAS is clean.");
        return Ok(());
    }

    // Calculate space to reclaim
    let mut reclaimable_size: u64 = 0;
    for (path, _) in &unreferenced {
        if let Ok(metadata) = fs::metadata(path) {
            reclaimable_size += metadata.len();
        }
    }

    println!(
        "\nFound {} unreferenced objects ({})",
        unreferenced.len(),
        format_bytes(reclaimable_size)
    );

    // Step 5: Delete unreferenced objects (or just report if dry_run)
    if dry_run {
        println!("\nDry run - would remove {} objects:", unreferenced.len());
        for (_, hash) in unreferenced.iter().take(10) {
            println!("  {}", hash);
        }
        if unreferenced.len() > 10 {
            println!("  ... and {} more", unreferenced.len() - 10);
        }
        println!("\nRun without --dry-run to actually remove these objects.");
    } else {
        println!("\nRemoving unreferenced objects...");
        let mut removed_count = 0;
        let mut error_count = 0;

        for (path, hash) in &unreferenced {
            match fs::remove_file(path) {
                Ok(()) => {
                    removed_count += 1;
                    info!("Removed: {}", hash);
                }
                Err(e) => {
                    error_count += 1;
                    info!("Failed to remove {}: {}", hash, e);
                }
            }
        }

        // Clean up empty prefix directories
        for prefix_entry in fs::read_dir(objects_path)? {
            let prefix_entry = prefix_entry?;
            let prefix_path = prefix_entry.path();

            if prefix_path.is_dir() {
                // Try to remove if empty (will fail silently if not empty)
                let _ = fs::remove_dir(&prefix_path);
            }
        }

        println!("\nGarbage collection complete:");
        println!("  Removed: {} objects", removed_count);
        println!("  Errors: {}", error_count);
        println!("  Space reclaimed: {}", format_bytes(reclaimable_size));
    }

    if chunks {
        gc_orphaned_chunks(&conn, db_path, dry_run)?;
    }

    Ok(())
}

/// Local-only chunk GC for the CLI. The full async version with R2 support
/// lives in apps/remi/src/server/chunk_gc.rs.
///
/// Scans the CAS objects directory for chunk files that are not referenced by
/// any converted package (`chunk_hashes_json`) or protected in `chunk_access`.
fn gc_orphaned_chunks(conn: &rusqlite::Connection, db_path: &str, dry_run: bool) -> Result<()> {
    use std::collections::HashSet;

    let objects_dir = conary_core::db::paths::objects_dir(db_path);

    println!("\nChunk GC: collecting referenced chunk hashes...");

    // Build referenced set from converted_packages
    let mut referenced = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT chunk_hashes_json FROM converted_packages WHERE chunk_hashes_json IS NOT NULL",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let json_str: String = row.get(0)?;
        if let Ok(hashes) = serde_json::from_str::<Vec<String>>(&json_str) {
            for hash in hashes {
                referenced.insert(hash);
            }
        }
    }

    // Add protected chunks from chunk_access
    let mut stmt = conn.prepare("SELECT hash FROM chunk_access WHERE protected = 1")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let hash: String = row.get(0)?;
        referenced.insert(hash);
    }

    // Add live file hashes from installed packages so we never delete CAS
    // objects that are still referenced by troves, rollback history, etc.
    let mut stmt = conn.prepare("SELECT DISTINCT sha256_hash FROM files WHERE sha256_hash IS NOT NULL AND sha256_hash != ''")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let hash: String = row.get(0)?;
        referenced.insert(hash);
    }

    // Add config backup hashes (used by config restore/diff)
    let mut stmt = conn.prepare(
        "SELECT DISTINCT backup_hash FROM config_backups WHERE backup_hash IS NOT NULL AND backup_hash != ''",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let hash: String = row.get(0)?;
        referenced.insert(hash);
    }

    // Add config file original hashes (used by config diff)
    if let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT original_hash FROM config_files WHERE original_hash IS NOT NULL AND original_hash != ''",
    ) {
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            referenced.insert(hash);
        }
    }

    // Add derived package CAS objects (patches and overrides)
    if let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT patch_hash FROM derived_patches WHERE patch_hash IS NOT NULL AND patch_hash != ''",
    ) {
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            referenced.insert(hash);
        }
    }
    if let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT source_hash FROM derived_overrides WHERE source_hash IS NOT NULL AND source_hash != ''",
    ) {
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            referenced.insert(hash);
        }
    }

    // Add derivation CAS objects (manifest + provenance hashes)
    if let Ok(mut stmt) = conn.prepare(
        "SELECT manifest_cas_hash FROM derivation_index WHERE manifest_cas_hash IS NOT NULL AND manifest_cas_hash != ''",
    ) {
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            referenced.insert(hash);
        }
    }
    if let Ok(mut stmt) = conn.prepare(
        "SELECT provenance_cas_hash FROM derivation_index WHERE provenance_cas_hash IS NOT NULL AND provenance_cas_hash != ''",
    ) {
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            referenced.insert(hash);
        }
    }

    // Also protect hashes referenced by state snapshots (rollback/restore)
    let mut stmt = conn.prepare("SELECT metadata FROM changesets WHERE metadata IS NOT NULL")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let json_str: String = row.get(0)?;
        // Snapshot metadata contains file hashes — extract them conservatively
        // by matching hex-like strings that look like CAS hashes.
        for word in json_str.split('"') {
            // CAS hashes are hex strings of 64 chars (SHA-256)
            if word.len() == 64 && word.chars().all(|c| c.is_ascii_hexdigit()) {
                referenced.insert(word.to_string());
            }
        }
    }

    // Scan local chunks in the objects directory
    let mut orphaned = 0usize;
    let mut freed = 0u64;
    if objects_dir.exists() {
        let cas = CasStore::new(&objects_dir)?;
        for result in cas.iter_objects() {
            let (hash, path) = result?;
            if !referenced.contains(&hash) {
                let size = path.metadata().map(|m| m.len()).unwrap_or(0);
                if dry_run {
                    println!("[dry-run] Would delete: {} ({} bytes)", hash, size);
                } else {
                    let _ = std::fs::remove_file(&path);
                    // Try to remove empty parent directory
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
                orphaned += 1;
                freed += size;
            }
        }
    }

    println!(
        "Chunk GC: {} referenced, {} orphaned, {} freed",
        referenced.len(),
        orphaned,
        format_bytes(freed)
    );

    Ok(())
}

use crate::commands::format_bytes;
