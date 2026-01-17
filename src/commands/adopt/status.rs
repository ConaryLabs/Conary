// src/commands/adopt/status.rs

//! Adoption status reporting
//!
//! Shows statistics about adopted and tracked packages.

use anyhow::Result;
use conary::db::models::{InstallSource, Trove};
use conary::packages::rpm_query;
use std::path::PathBuf;

/// Show adoption status
pub fn cmd_adopt_status(db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let troves = Trove::list_all(&conn)?;

    let mut adopted_track = 0;
    let mut adopted_full = 0;
    let mut installed_file = 0;
    let mut installed_repo = 0;

    for trove in &troves {
        match trove.install_source {
            InstallSource::AdoptedTrack => adopted_track += 1,
            InstallSource::AdoptedFull => adopted_full += 1,
            InstallSource::File => installed_file += 1,
            InstallSource::Repository => installed_repo += 1,
        }
    }

    // Get total files tracked
    let file_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files",
        [],
        |row| row.get(0),
    ).unwrap_or(0);

    // Get CAS storage stats
    let objects_dir = PathBuf::from(db_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("objects");

    let (cas_files, cas_bytes) = if objects_dir.exists() {
        count_dir_usage(&objects_dir)
    } else {
        (0, 0)
    };

    // Get system package count for comparison
    let system_count = if rpm_query::is_rpm_available() {
        rpm_query::list_installed_packages().map(|p| p.len()).unwrap_or(0)
    } else {
        0
    };

    println!("Conary Adoption Status");
    println!("======================");
    println!();
    println!("Tracked packages: {}", troves.len());
    println!("  Adopted (track mode): {}", adopted_track);
    println!("  Adopted (full mode):  {}", adopted_full);
    println!("  Installed from file:  {}", installed_file);
    println!("  Installed from repo:  {}", installed_repo);
    println!();
    println!("Tracked files: {}", file_count);
    println!();
    println!("CAS Storage:");
    println!("  Objects: {}", cas_files);
    println!("  Size:    {}", format_bytes(cas_bytes));
    println!();
    if system_count > 0 {
        println!("System RPM packages: {}", system_count);
        let coverage = (troves.len() as f64 / system_count as f64 * 100.0).min(100.0);
        println!("Coverage: {:.1}%", coverage);
    }

    Ok(())
}

/// Count files and total bytes in a directory recursively
fn count_dir_usage(dir: &std::path::Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let (f, b) = count_dir_usage(&path);
                files += f;
                bytes += b;
            } else if path.is_file() {
                files += 1;
                if let Ok(meta) = path.metadata() {
                    bytes += meta.len();
                }
            }
        }
    }

    (files, bytes)
}

/// Format bytes as human-readable string
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}
