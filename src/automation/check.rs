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
use crate::model::AutomationConfig;
use chrono::{Duration, Utc};
use rusqlite::Connection;
use std::collections::HashSet;

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
    fn filter_by_grace_period(
        &self,
        packages: &[String],
        _grace: std::time::Duration,
    ) -> Result<Vec<String>> {
        // In a real implementation, we'd track when packages became orphaned
        // For now, return all packages (assume they've been orphaned long enough)
        // TODO: Add orphan_since tracking to troves table
        Ok(packages.to_vec())
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
    fn find_corrupted_files(&self) -> Result<Vec<String>> {
        // Placeholder - would actually verify file hashes
        // For now, return empty (no corruption detected)
        Ok(Vec::new())
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
}
