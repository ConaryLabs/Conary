// src/commands/adopt/conflicts.rs

//! Conflict detection for adopted packages
//!
//! Checks for file ownership conflicts, adoption conflicts,
//! and stale file entries.

use anyhow::Result;
use conary::packages::rpm_query;

/// Check for conflicts between tracked packages and files
pub fn cmd_conflicts(db_path: &str, verbose: bool) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    println!("Checking for conflicts...\n");

    let mut issues_found = 0;

    // 1. Check for files with mismatched hashes between adopted and Conary-installed
    //    (e.g., adopted package A has file X, Conary package B also has file X)
    issues_found += check_overlapping_files(&conn, verbose)?;

    // 2. Check for adopted packages that would conflict if updated via Conary
    issues_found += check_adoption_conflicts(&conn, verbose)?;

    // 3. Check for stale file entries (tracked but missing on disk)
    issues_found += check_stale_files(&conn, verbose)?;

    println!();
    if issues_found == 0 {
        println!("No conflicts found.");
    } else {
        println!("Total issues found: {}", issues_found);
    }

    Ok(())
}

/// Check for files that might be owned by multiple packages in RPM but only tracked once in Conary
fn check_overlapping_files(conn: &rusqlite::Connection, verbose: bool) -> Result<usize> {
    // Find files where the tracked owner is adopted but RPM reports different ownership
    let mut count = 0;

    // Get all tracked files with their package info
    let files: Vec<(String, String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT f.path, f.sha256_hash, t.name, t.install_source
             FROM files f
             JOIN troves t ON f.trove_id = t.id
             ORDER BY f.path"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // For adopted packages, verify the file still matches what RPM says
    if rpm_query::is_rpm_available() {
        let mut rpm_owners: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

        // Build a cache of file -> rpm owners for faster lookups
        // We'll check a sample of files to avoid being too slow
        let sample_size = if verbose { files.len() } else { files.len().min(1000) };

        for (path, _hash, pkg_name, source) in files.iter().take(sample_size) {
            if source.starts_with("adopted") {
                // Check who RPM thinks owns this file
                if let Ok(owners) = rpm_query::query_file_owner(path) {
                    if owners.len() > 1 {
                        count += 1;
                        if verbose || count <= 10 {
                            println!("File owned by multiple RPM packages:");
                            println!("  Path: {}", path);
                            println!("  Tracked by: {} ({})", pkg_name, source);
                            println!("  RPM owners: {}", owners.join(", "));
                            println!();
                        }
                        rpm_owners.insert(path.clone(), owners);
                    } else if !owners.is_empty() && owners[0] != *pkg_name {
                        count += 1;
                        if verbose || count <= 10 {
                            println!("File ownership mismatch:");
                            println!("  Path: {}", path);
                            println!("  Tracked by: {} ({})", pkg_name, source);
                            println!("  RPM owner: {}", owners[0]);
                            println!();
                        }
                    }
                }
            }
        }

        if count > 10 && !verbose {
            println!("... and {} more ownership issues (use --verbose to see all)\n", count - 10);
        }
    }

    if count > 0 {
        println!("Overlapping file ownership: {} issues", count);
    }

    Ok(count)
}

/// Check for adopted packages that share files with Conary-installed packages
fn check_adoption_conflicts(conn: &rusqlite::Connection, verbose: bool) -> Result<usize> {
    let mut count = 0;

    // Find packages installed via Conary (not adopted)
    let _conary_packages: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, name FROM troves WHERE install_source IN ('file', 'repository')"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // Get all files owned by Conary-installed packages
    let conary_file_paths: std::collections::HashSet<String> = {
        let mut stmt = conn.prepare(
            "SELECT f.path FROM files f
             JOIN troves t ON f.trove_id = t.id
             WHERE t.install_source IN ('file', 'repository')"
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?
    };

    if conary_file_paths.is_empty() {
        return Ok(0);
    }

    // Check if any adopted package files overlap with Conary-installed files
    let adopted_files: Vec<(String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT f.path, t.name, t.version FROM files f
             JOIN troves t ON f.trove_id = t.id
             WHERE t.install_source LIKE 'adopted%'"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut conflicts_by_pkg: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    for (path, pkg_name, _version) in &adopted_files {
        if conary_file_paths.contains(path) {
            conflicts_by_pkg
                .entry(pkg_name.clone())
                .or_default()
                .push(path.clone());
            count += 1;
        }
    }

    if !conflicts_by_pkg.is_empty() {
        println!("Adopted packages with file conflicts against Conary-installed packages:");
        for (pkg, paths) in &conflicts_by_pkg {
            println!("  {} ({} conflicting files)", pkg, paths.len());
            if verbose {
                for path in paths.iter().take(5) {
                    println!("    - {}", path);
                }
                if paths.len() > 5 {
                    println!("    - ... and {} more", paths.len() - 5);
                }
            }
        }
        println!();
    }

    Ok(count)
}

/// Check for stale file entries (tracked in DB but missing on disk)
fn check_stale_files(conn: &rusqlite::Connection, verbose: bool) -> Result<usize> {
    let mut count = 0;

    // Get all tracked files
    let files: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT f.path, t.name FROM files f
             JOIN troves t ON f.trove_id = t.id
             ORDER BY t.name, f.path"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut missing_by_pkg: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    for (path, pkg_name) in &files {
        if !std::path::Path::new(path).exists() {
            missing_by_pkg
                .entry(pkg_name.clone())
                .or_default()
                .push(path.clone());
            count += 1;
        }
    }

    if !missing_by_pkg.is_empty() {
        println!("Packages with missing files:");
        for (pkg, paths) in &missing_by_pkg {
            println!("  {} ({} missing files)", pkg, paths.len());
            if verbose {
                for path in paths.iter().take(5) {
                    println!("    - {}", path);
                }
                if paths.len() > 5 {
                    println!("    - ... and {} more", paths.len() - 5);
                }
            }
        }
        println!();
    }

    Ok(count)
}
