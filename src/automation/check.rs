// src/automation/check.rs

//! Automation checks - scan system for actionable items.
//!
//! This module implements the detection phase of automation:
//! - Security update scanning
//! - Orphaned package detection
//! - Available update checking
//! - Integrity verification

use super::action::{
    integrity_repair_action, orphan_cleanup_action, package_update_action,
    security_update_action,
};
use super::PendingAction;
use crate::error::Result;
use crate::hash::verify_file_sha256;
use crate::model::AutomationConfig;
use chrono::{Duration, Utc};
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::Path;
use tracing::{debug, trace, warn};

/// Results from an automation check run
#[derive(Debug, Default)]
pub struct CheckResults {
    /// Security updates found
    pub security: Vec<PendingAction>,

    /// Orphaned packages found
    pub orphans: Vec<PendingAction>,

    /// Available updates found
    pub updates: Vec<PendingAction>,

    /// Integrity issues found
    pub integrity: Vec<PendingAction>,

    /// Errors encountered during checks
    pub errors: Vec<String>,
}

impl CheckResults {
    /// Total number of actions found
    pub fn total(&self) -> usize {
        self.security.len() + self.orphans.len() + self.updates.len() + self.integrity.len()
    }

    /// Get all actions as a flat list
    pub fn all_actions(&self) -> Vec<&PendingAction> {
        let mut actions = Vec::new();
        actions.extend(self.security.iter());
        actions.extend(self.orphans.iter());
        actions.extend(self.updates.iter());
        actions.extend(self.integrity.iter());
        actions
    }
}

/// Checker for automation actions
pub struct AutomationChecker<'a> {
    conn: &'a Connection,
    config: &'a AutomationConfig,
}

impl<'a> AutomationChecker<'a> {
    /// Create a new automation checker
    pub fn new(conn: &'a Connection, config: &'a AutomationConfig) -> Self {
        Self { conn, config }
    }

    /// Run all enabled checks
    pub fn run_all(&self) -> Result<CheckResults> {
        let mut results = CheckResults::default();

        // Security checks
        if let Err(e) = self.check_security(&mut results) {
            results.errors.push(format!("Security check failed: {}", e));
        }

        // Orphan checks
        if let Err(e) = self.check_orphans(&mut results) {
            results.errors.push(format!("Orphan check failed: {}", e));
        }

        // Update checks
        if let Err(e) = self.check_updates(&mut results) {
            results.errors.push(format!("Update check failed: {}", e));
        }

        // Integrity checks (only if enabled)
        if self.config.repair.integrity_check
            && let Err(e) = self.check_integrity(&mut results)
        {
            results.errors.push(format!("Integrity check failed: {}", e));
        }

        Ok(results)
    }

    /// Check for security updates
    fn check_security(&self, results: &mut CheckResults) -> Result<()> {
        // Query repository_packages for security updates
        let security_updates = self.find_security_updates()?;

        for (package, cves, severity) in security_updates {
            let action = security_update_action(&[package], &cves, &severity);

            // Apply deadline based on config
            let deadline = self.calculate_security_deadline(&severity);
            let mut action = action;
            action.deadline = deadline;

            results.security.push(action);
        }

        Ok(())
    }

    /// Find packages with available security updates
    fn find_security_updates(&self) -> Result<Vec<(String, Vec<String>, String)>> {
        // Query the repository_packages table for security updates
        // This checks the is_security_update and security_severity columns
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.name, rp.security_cves, rp.security_severity
             FROM troves t
             JOIN repository_packages rp ON t.name = rp.name
             WHERE rp.is_security_update = 1
               AND rp.version > t.version
             ORDER BY
               CASE rp.security_severity
                 WHEN 'critical' THEN 1
                 WHEN 'high' THEN 2
                 WHEN 'medium' THEN 3
                 WHEN 'low' THEN 4
                 ELSE 5
               END",
        )?;

        let mut updates = Vec::new();
        let rows = stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let cves_str: Option<String> = row.get(1)?;
            let severity: String = row.get(2)?;
            Ok((name, cves_str, severity))
        })?;

        for row in rows {
            let (name, cves_str, severity) = row?;
            let cves: Vec<String> = cves_str
                .map(|s| s.split(',').map(|c| c.trim().to_string()).collect())
                .unwrap_or_default();

            // Filter by configured severity levels
            if self.should_include_severity(&severity) {
                updates.push((name, cves, severity));
            }
        }

        Ok(updates)
    }

    /// Check if a severity level should be included based on config
    fn should_include_severity(&self, severity: &str) -> bool {
        self.config
            .security
            .severities
            .iter()
            .any(|s| s.eq_ignore_ascii_case(severity))
    }

    /// Calculate deadline for security update based on severity and config
    fn calculate_security_deadline(
        &self,
        severity: &str,
    ) -> Option<chrono::DateTime<Utc>> {
        let window = super::parse_duration(&self.config.security.within).ok()?;
        let multiplier = match severity.to_lowercase().as_str() {
            "critical" => 0.25, // 25% of window for critical
            "high" => 0.5,      // 50% of window for high
            "medium" => 1.0,    // Full window for medium
            "low" => 2.0,       // Double window for low
            _ => 1.0,
        };

        let adjusted_secs = (window.as_secs() as f64 * multiplier) as i64;
        Some(Utc::now() + Duration::seconds(adjusted_secs))
    }

    /// Check for orphaned packages
    fn check_orphans(&self, results: &mut CheckResults) -> Result<()> {
        let orphans = self.find_orphan_packages()?;

        if orphans.is_empty() {
            return Ok(());
        }

        // Filter out packages in the keep list
        let keep_set: HashSet<_> = self.config.orphans.keep.iter().collect();
        let orphans: Vec<_> = orphans
            .into_iter()
            .filter(|p| !keep_set.contains(p))
            .collect();

        if orphans.is_empty() {
            return Ok(());
        }

        // Check grace period
        let grace_period = super::parse_duration(&self.config.orphans.after)?;
        let orphans_with_age = self.filter_by_grace_period(&orphans, grace_period)?;

        if !orphans_with_age.is_empty() {
            results.orphans.push(orphan_cleanup_action(&orphans_with_age));
        }

        Ok(())
    }

    /// Find packages that are no longer required by anything
    fn find_orphan_packages(&self) -> Result<Vec<String>> {
        // Find packages installed as dependencies that are no longer needed
        let mut stmt = self.conn.prepare(
            "SELECT t.name FROM troves t
             WHERE t.install_reason = 'dependency'
               AND NOT EXISTS (
                 SELECT 1 FROM dependencies d
                 JOIN troves t2 ON d.trove_id = t2.id
                 WHERE d.name = t.name
               )",
        )?;

        let orphans: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(orphans)
    }

    /// Filter orphan packages by grace period
    ///
    /// This uses the `orphan_since` column in the troves table to track when
    /// packages first became orphaned. Packages are only returned for cleanup
    /// if they've been orphaned longer than the grace period.
    ///
    /// For packages that just became orphaned (no `orphan_since` set), this
    /// function sets the timestamp to now and excludes them from cleanup.
    fn filter_by_grace_period(
        &self,
        packages: &[String],
        grace: std::time::Duration,
    ) -> Result<Vec<String>> {
        if packages.is_empty() {
            return Ok(Vec::new());
        }

        let now = Utc::now();
        let grace_duration = Duration::seconds(grace.as_secs() as i64);
        let cutoff = now - grace_duration;
        let cutoff_str = cutoff.to_rfc3339();
        let now_str = now.to_rfc3339();

        let mut ready_for_cleanup = Vec::new();

        for name in packages {
            // Query the orphan_since timestamp for this package
            let orphan_since: Option<String> = self
                .conn
                .query_row(
                    "SELECT orphan_since FROM troves WHERE name = ?1 LIMIT 1",
                    [name],
                    |row| row.get(0),
                )
                .ok()
                .flatten();

            match orphan_since {
                Some(timestamp) => {
                    // Package was already marked as orphan - check if grace period passed
                    if timestamp <= cutoff_str {
                        debug!(
                            "Package {} orphaned since {}, past grace period (cutoff: {})",
                            name, timestamp, cutoff_str
                        );
                        ready_for_cleanup.push(name.clone());
                    } else {
                        debug!(
                            "Package {} orphaned since {}, still in grace period",
                            name, timestamp
                        );
                    }
                }
                None => {
                    // First time this package is detected as orphan - mark it
                    debug!("Marking package {} as orphaned at {}", name, now_str);
                    if let Err(e) = self.conn.execute(
                        "UPDATE troves SET orphan_since = ?1 WHERE name = ?2",
                        [&now_str, name],
                    ) {
                        warn!("Failed to set orphan_since for {}: {}", name, e);
                    }
                    // Don't include in cleanup yet - grace period starts now
                }
            }
        }

        Ok(ready_for_cleanup)
    }

    /// Clear orphan_since for packages that are no longer orphaned
    ///
    /// Call this when a package gains a new dependent, so the grace period
    /// resets if it becomes orphaned again later.
    pub fn clear_orphan_status(&self, packages: &[String]) -> Result<()> {
        if packages.is_empty() {
            return Ok(());
        }

        for name in packages {
            self.conn.execute(
                "UPDATE troves SET orphan_since = NULL WHERE name = ?1",
                [name],
            )?;
            debug!("Cleared orphan status for {}", name);
        }

        Ok(())
    }

    /// Check for available updates
    fn check_updates(&self, results: &mut CheckResults) -> Result<()> {
        // Find packages with newer versions in repos (excluding security which is handled separately)
        let mut stmt = self.conn.prepare(
            "SELECT t.name, t.version, MAX(rp.version) as new_version
             FROM troves t
             JOIN repository_packages rp ON t.name = rp.name
             WHERE rp.version > t.version
               AND (rp.is_security_update IS NULL OR rp.is_security_update = 0)
             GROUP BY t.name
             ORDER BY t.name",
        )?;

        let rows = stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let current: String = row.get(1)?;
            let new: String = row.get(2)?;
            Ok((name, current, new))
        })?;

        // Filter out excluded packages
        let exclude_set: HashSet<_> = self.config.updates.exclude.iter().collect();

        for row in rows {
            let (name, current, new) = row?;
            if exclude_set.contains(&name) {
                continue;
            }

            // Check if this is a major version upgrade
            if is_major_upgrade(&current, &new) {
                // Handle separately in major upgrades category
                continue;
            }

            results.updates.push(package_update_action(&name, &current, &new));
        }

        Ok(())
    }

    /// Check file integrity against CAS
    fn check_integrity(&self, results: &mut CheckResults) -> Result<()> {
        // Query files table and verify against CAS
        // This is a placeholder - real implementation would:
        // 1. Query files table for all managed files
        // 2. Compute hash of each file on disk
        // 3. Compare against stored hash
        // 4. Report mismatches

        let corrupted = self.find_corrupted_files()?;

        if !corrupted.is_empty() {
            // Group by package
            let action = integrity_repair_action(&corrupted, None);
            results.integrity.push(action);
        }

        Ok(())
    }

    /// Find files that have been corrupted (hash mismatch)
    ///
    /// Queries all managed files from the database and verifies each one:
    /// - File exists on disk
    /// - File hash matches expected SHA-256 hash
    ///
    /// Returns paths of files that are missing or have hash mismatches.
    fn find_corrupted_files(&self) -> Result<Vec<String>> {
        // Query all managed files with their expected hashes
        let mut stmt = self.conn.prepare(
            "SELECT path, sha256_hash FROM files ORDER BY path",
        )?;

        let files: Vec<(String, String)> = stmt
            .query_map([], |row| {
                let path: String = row.get(0)?;
                let hash: String = row.get(1)?;
                Ok((path, hash))
            })?
            .filter_map(|r| r.ok())
            .collect();

        debug!("Checking integrity of {} managed files", files.len());

        let mut corrupted = Vec::new();

        for (path, expected_hash) in files {
            let file_path = Path::new(&path);

            // Check if file exists
            if !file_path.exists() {
                // File is missing - only report if it's not a symlink target issue
                if !file_path.is_symlink() {
                    trace!("Missing file: {}", path);
                    corrupted.push(path);
                }
                continue;
            }

            // Skip directories - they don't have content hashes
            if file_path.is_dir() {
                continue;
            }

            // Skip symlinks - hash is of link target path, not content
            if file_path.is_symlink() {
                continue;
            }

            // Verify hash matches
            match verify_file_sha256(file_path, &expected_hash) {
                Ok(()) => {
                    trace!("Verified: {}", path);
                }
                Err(e) => {
                    warn!("Integrity check failed for {}: {}", path, e);
                    corrupted.push(path);
                }
            }
        }

        if !corrupted.is_empty() {
            debug!("Found {} corrupted/missing files", corrupted.len());
        }

        Ok(corrupted)
    }
}

/// Check if version change is a major upgrade
fn is_major_upgrade(current: &str, new: &str) -> bool {
    // Simple heuristic: compare first numeric component
    let current_major = current
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|s| s.parse::<u32>().ok());

    let new_major = new
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|s| s.parse::<u32>().ok());

    match (current_major, new_major) {
        (Some(c), Some(n)) => n > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_major_upgrade() {
        assert!(is_major_upgrade("1.0.0", "2.0.0"));
        assert!(is_major_upgrade("1.5.3", "2.0.0"));
        assert!(!is_major_upgrade("1.0.0", "1.1.0"));
        assert!(!is_major_upgrade("1.0.0", "1.0.1"));
        assert!(is_major_upgrade("20.04", "22.04"));
    }

    #[test]
    fn test_check_results_total() {
        let mut results = CheckResults::default();
        assert_eq!(results.total(), 0);

        results.security.push(security_update_action(
            &["test".to_string()],
            &[],
            "high",
        ));
        assert_eq!(results.total(), 1);
    }

    #[test]
    fn test_find_corrupted_files_empty_db() {
        // Create in-memory database with schema
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                sha256_hash TEXT NOT NULL,
                size INTEGER NOT NULL,
                permissions INTEGER NOT NULL,
                trove_id INTEGER NOT NULL
            );
            CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL
            );",
        ).unwrap();

        let config = AutomationConfig::default();
        let checker = AutomationChecker::new(&conn, &config);

        // No files = no corruption
        let corrupted = checker.find_corrupted_files().unwrap();
        assert!(corrupted.is_empty());
    }

    #[test]
    fn test_find_corrupted_files_detects_mismatch() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create in-memory database
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                sha256_hash TEXT NOT NULL,
                size INTEGER NOT NULL,
                permissions INTEGER NOT NULL,
                trove_id INTEGER NOT NULL
            );",
        ).unwrap();

        // Create a temp file with known content
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "hello world").unwrap();
        temp_file.flush().unwrap();
        let temp_path = temp_file.path().to_str().unwrap();

        // The correct hash for "hello world" is:
        let correct_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let wrong_hash = "0000000000000000000000000000000000000000000000000000000000000000";

        // Insert file with WRONG hash - should be detected as corrupted
        conn.execute(
            "INSERT INTO files (path, sha256_hash, size, permissions, trove_id)
             VALUES (?1, ?2, 11, 644, 1)",
            [temp_path, wrong_hash],
        ).unwrap();

        let config = AutomationConfig::default();
        let checker = AutomationChecker::new(&conn, &config);

        let corrupted = checker.find_corrupted_files().unwrap();
        assert_eq!(corrupted.len(), 1);
        assert_eq!(corrupted[0], temp_path);

        // Now update with correct hash - should NOT be detected
        conn.execute(
            "UPDATE files SET sha256_hash = ?1 WHERE path = ?2",
            [correct_hash, temp_path],
        ).unwrap();

        let corrupted = checker.find_corrupted_files().unwrap();
        assert!(corrupted.is_empty());
    }

    #[test]
    fn test_find_corrupted_files_missing_file() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                sha256_hash TEXT NOT NULL,
                size INTEGER NOT NULL,
                permissions INTEGER NOT NULL,
                trove_id INTEGER NOT NULL
            );",
        ).unwrap();

        // Insert a file that doesn't exist
        conn.execute(
            "INSERT INTO files (path, sha256_hash, size, permissions, trove_id)
             VALUES ('/nonexistent/file/path/abc123.txt', 'abc123', 100, 644, 1)",
            [],
        ).unwrap();

        let config = AutomationConfig::default();
        let checker = AutomationChecker::new(&conn, &config);

        let corrupted = checker.find_corrupted_files().unwrap();
        assert_eq!(corrupted.len(), 1);
        assert_eq!(corrupted[0], "/nonexistent/file/path/abc123.txt");
    }

    #[test]
    fn test_filter_by_grace_period_empty() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                orphan_since TEXT
            );",
        ).unwrap();

        let config = AutomationConfig::default();
        let checker = AutomationChecker::new(&conn, &config);

        // Empty list returns empty
        let result = checker
            .filter_by_grace_period(&[], std::time::Duration::from_secs(3600))
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_filter_by_grace_period_marks_new_orphans() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                orphan_since TEXT
            );
            INSERT INTO troves (name) VALUES ('libfoo');",
        ).unwrap();

        let config = AutomationConfig::default();
        let checker = AutomationChecker::new(&conn, &config);

        // First time detecting orphan - should mark it but not return it
        let result = checker
            .filter_by_grace_period(&["libfoo".to_string()], std::time::Duration::from_secs(3600))
            .unwrap();
        assert!(result.is_empty(), "New orphan should not be returned immediately");

        // Verify orphan_since was set
        let orphan_since: Option<String> = conn
            .query_row("SELECT orphan_since FROM troves WHERE name = 'libfoo'", [], |row| row.get(0))
            .unwrap();
        assert!(orphan_since.is_some(), "orphan_since should be set");
    }

    #[test]
    fn test_filter_by_grace_period_respects_grace() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                orphan_since TEXT
            );",
        ).unwrap();

        // Insert package that was orphaned 2 hours ago
        let two_hours_ago = Utc::now() - Duration::hours(2);
        conn.execute(
            "INSERT INTO troves (name, orphan_since) VALUES ('libold', ?1)",
            [two_hours_ago.to_rfc3339()],
        ).unwrap();

        // Insert package that was orphaned 30 minutes ago
        let thirty_mins_ago = Utc::now() - Duration::minutes(30);
        conn.execute(
            "INSERT INTO troves (name, orphan_since) VALUES ('libnew', ?1)",
            [thirty_mins_ago.to_rfc3339()],
        ).unwrap();

        let config = AutomationConfig::default();
        let checker = AutomationChecker::new(&conn, &config);

        // With 1 hour grace period, only libold should be returned
        let result = checker
            .filter_by_grace_period(
                &["libold".to_string(), "libnew".to_string()],
                std::time::Duration::from_secs(3600), // 1 hour
            )
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "libold");
    }

    #[test]
    fn test_clear_orphan_status() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                orphan_since TEXT
            );",
        ).unwrap();

        // Insert package with orphan_since set
        let yesterday = Utc::now() - Duration::days(1);
        conn.execute(
            "INSERT INTO troves (name, orphan_since) VALUES ('libfoo', ?1)",
            [yesterday.to_rfc3339()],
        ).unwrap();

        let config = AutomationConfig::default();
        let checker = AutomationChecker::new(&conn, &config);

        // Clear orphan status
        checker.clear_orphan_status(&["libfoo".to_string()]).unwrap();

        // Verify orphan_since is now NULL
        let orphan_since: Option<String> = conn
            .query_row("SELECT orphan_since FROM troves WHERE name = 'libfoo'", [], |row| row.get(0))
            .unwrap();
        assert!(orphan_since.is_none(), "orphan_since should be cleared");
    }
}
