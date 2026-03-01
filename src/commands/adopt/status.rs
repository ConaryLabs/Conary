// src/commands/adopt/status.rs

//! Adoption status reporting
//!
//! Shows statistics about adopted and tracked packages.

use anyhow::Result;
use conary::db::models::{InstallReason, InstallSource, Trove};
use conary::packages::SystemPackageManager;
use std::path::PathBuf;

/// Show adoption status
pub fn cmd_adopt_status(db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let troves = Trove::list_all(&conn)?;

    let mut adopted_track = 0;
    let mut adopted_full = 0;
    let mut taken = 0;
    let mut installed_file = 0;
    let mut installed_repo = 0;
    let mut explicit_count = 0;
    let mut dep_count = 0;

    for trove in &troves {
        match trove.install_source {
            InstallSource::AdoptedTrack => adopted_track += 1,
            InstallSource::AdoptedFull => adopted_full += 1,
            InstallSource::Taken => taken += 1,
            InstallSource::File => installed_file += 1,
            InstallSource::Repository => installed_repo += 1,
        }
        match trove.install_reason {
            InstallReason::Explicit => explicit_count += 1,
            InstallReason::Dependency => dep_count += 1,
        }
    }

    let adopted_total = adopted_track + adopted_full;

    // Get total files tracked
    let file_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
        .unwrap_or(0);

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

    // Get system package count using detected package manager
    let pkg_mgr = SystemPackageManager::detect();
    let (system_count, mgr_name) = if pkg_mgr.is_available() {
        let count = match pkg_mgr {
            SystemPackageManager::Rpm => conary::packages::rpm_query::list_installed_packages()
                .map(|p| p.len())
                .unwrap_or(0),
            SystemPackageManager::Dpkg => conary::packages::dpkg_query::list_installed_packages()
                .map(|p| p.len())
                .unwrap_or(0),
            SystemPackageManager::Pacman => {
                conary::packages::pacman_query::list_installed_packages()
                    .map(|p| p.len())
                    .unwrap_or(0)
            }
            _ => 0,
        };
        (count, format!("{:?}", pkg_mgr))
    } else {
        (0, "none".to_string())
    };

    println!("Conary Adoption Status");
    println!("======================\n");

    println!("System package manager: {}", mgr_name);
    if system_count > 0 {
        println!("System packages: {}", system_count);
    }
    println!();

    println!("Tracked packages: {}", troves.len());
    println!("  Adopted (track): {}", adopted_track);
    println!("  Adopted (full):  {}", adopted_full);
    println!("  Taken over:      {}", taken);
    println!("  Installed (file): {}", installed_file);
    println!("  Installed (repo): {}", installed_repo);
    println!();

    println!("Install reasons:");
    println!("  Explicit:   {}", explicit_count);
    println!("  Dependency: {}", dep_count);
    println!();

    println!("Tracked files: {}", file_count);
    println!();

    if cas_files > 0 || cas_bytes > 0 {
        println!("CAS Storage:");
        println!("  Objects: {}", cas_files);
        println!("  Size:    {}", format_bytes(cas_bytes));
        println!();
    }

    if system_count > 0 {
        let coverage = (adopted_total as f64 / system_count as f64 * 100.0).min(100.0);
        let bar = coverage_bar(coverage, 30);
        println!("Adoption coverage:");
        println!(
            "  {} {:.1}% ({}/{})",
            bar, coverage, adopted_total, system_count
        );
    }

    Ok(())
}

/// Generate an ASCII coverage bar
fn coverage_bar(percent: f64, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "#".repeat(filled), "-".repeat(empty))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coverage_bar_empty() {
        assert_eq!(coverage_bar(0.0, 10), "[----------]");
    }

    #[test]
    fn test_coverage_bar_full() {
        assert_eq!(coverage_bar(100.0, 10), "[##########]");
    }

    #[test]
    fn test_coverage_bar_half() {
        assert_eq!(coverage_bar(50.0, 10), "[#####-----]");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(512), "512 bytes");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1_048_576), "1.00 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.00 GB");
    }
}
