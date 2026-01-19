// src/automation/action.rs

//! Action definitions and execution for automation system.

use super::{ActionStatus, PendingAction};
use crate::error::Result;
use crate::model::AutomationCategory;
use chrono::Utc;

/// Builder for creating pending actions
pub struct ActionBuilder {
    category: AutomationCategory,
    summary: String,
    details: Vec<String>,
    packages: Vec<String>,
    risk_level: f64,
    requires_reboot: bool,
    reversible: bool,
    deadline: Option<chrono::DateTime<Utc>>,
}

impl ActionBuilder {
    /// Create a new action builder for the given category
    pub fn new(category: AutomationCategory, summary: impl Into<String>) -> Self {
        Self {
            category,
            summary: summary.into(),
            details: Vec::new(),
            packages: Vec::new(),
            risk_level: 0.1,
            requires_reboot: false,
            reversible: true,
            deadline: None,
        }
    }

    /// Add a detail line
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }

    /// Add multiple detail lines
    pub fn details(mut self, details: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.details.extend(details.into_iter().map(Into::into));
        self
    }

    /// Add an affected package
    pub fn package(mut self, package: impl Into<String>) -> Self {
        self.packages.push(package.into());
        self
    }

    /// Add multiple affected packages
    pub fn packages(mut self, packages: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.packages.extend(packages.into_iter().map(Into::into));
        self
    }

    /// Set the risk level (0.0 to 1.0)
    pub fn risk(mut self, level: f64) -> Self {
        self.risk_level = level.clamp(0.0, 1.0);
        self
    }

    /// Mark as requiring reboot
    pub fn requires_reboot(mut self) -> Self {
        self.requires_reboot = true;
        self
    }

    /// Mark as not reversible
    pub fn irreversible(mut self) -> Self {
        self.reversible = false;
        self
    }

    /// Set a deadline for the action
    pub fn deadline(mut self, deadline: chrono::DateTime<Utc>) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Build the pending action
    pub fn build(self) -> PendingAction {
        let id = format!(
            "{:?}-{}-{}",
            self.category,
            self.packages.first().unwrap_or(&"system".to_string()),
            Utc::now().timestamp_millis()
        );

        PendingAction {
            id,
            category: self.category,
            summary: self.summary,
            details: self.details,
            packages: self.packages,
            risk_level: self.risk_level,
            requires_reboot: self.requires_reboot,
            reversible: self.reversible,
            identified_at: Utc::now(),
            deadline: self.deadline,
            estimated_duration: None,
        }
    }
}

/// Creates a security update action
pub fn security_update_action(
    packages: &[String],
    cve_ids: &[String],
    severity: &str,
) -> PendingAction {
    let summary = if packages.len() == 1 {
        format!("Security update for {}", packages[0])
    } else {
        format!("Security updates for {} packages", packages.len())
    };

    let risk = match severity.to_lowercase().as_str() {
        "critical" => 0.2, // Low risk to apply critical updates
        "high" => 0.3,
        "medium" => 0.4,
        "low" => 0.5,
        _ => 0.3,
    };

    let mut builder = ActionBuilder::new(AutomationCategory::Security, summary)
        .packages(packages.iter().cloned())
        .risk(risk);

    if !cve_ids.is_empty() {
        builder = builder.detail(format!("CVEs addressed: {}", cve_ids.join(", ")));
    }

    builder = builder.detail(format!("Severity: {}", severity));

    builder.build()
}

/// Creates an orphan cleanup action
pub fn orphan_cleanup_action(packages: &[String]) -> PendingAction {
    let summary = if packages.len() == 1 {
        format!("Remove orphaned package: {}", packages[0])
    } else {
        format!("Remove {} orphaned packages", packages.len())
    };

    ActionBuilder::new(AutomationCategory::Orphans, summary)
        .packages(packages.iter().cloned())
        .detail("These packages are no longer required by any installed package")
        .risk(0.3)
        .build()
}

/// Creates a package update action
pub fn package_update_action(
    package: &str,
    current_version: &str,
    new_version: &str,
) -> PendingAction {
    ActionBuilder::new(
        AutomationCategory::Updates,
        format!("Update {} from {} to {}", package, current_version, new_version),
    )
    .package(package)
    .detail(format!("Current version: {}", current_version))
    .detail(format!("New version: {}", new_version))
    .risk(0.2)
    .build()
}

/// Creates a major upgrade action
pub fn major_upgrade_action(
    package: &str,
    current_version: &str,
    new_version: &str,
    breaking_changes: &[String],
) -> PendingAction {
    let mut builder = ActionBuilder::new(
        AutomationCategory::MajorUpgrades,
        format!(
            "Major upgrade: {} {} -> {}",
            package, current_version, new_version
        ),
    )
    .package(package)
    .risk(0.6);

    if !breaking_changes.is_empty() {
        builder = builder.detail("Breaking changes:");
        for change in breaking_changes {
            builder = builder.detail(format!("  - {}", change));
        }
    }

    builder.build()
}

/// Creates an integrity repair action
pub fn integrity_repair_action(
    files: &[String],
    package: Option<&str>,
) -> PendingAction {
    let summary = if let Some(pkg) = package {
        format!("Repair {} corrupted files in {}", files.len(), pkg)
    } else {
        format!("Repair {} corrupted system files", files.len())
    };

    let mut builder = ActionBuilder::new(AutomationCategory::Repair, summary)
        .risk(0.4);

    if let Some(pkg) = package {
        builder = builder.package(pkg);
    }

    builder = builder.detail(format!("Files to restore: {}", files.len()));

    if files.len() <= 5 {
        for file in files {
            builder = builder.detail(format!("  {}", file));
        }
    } else {
        for file in files.iter().take(3) {
            builder = builder.detail(format!("  {}", file));
        }
        builder = builder.detail(format!("  ... and {} more", files.len() - 3));
    }

    builder.build()
}

/// Executor for automation actions
pub struct ActionExecutor {
    dry_run: bool,
    executed: Vec<String>,
    failed: Vec<(String, String)>,
}

impl ActionExecutor {
    /// Create a new action executor
    pub fn new(dry_run: bool) -> Self {
        Self {
            dry_run,
            executed: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// Execute an action
    pub fn execute(&mut self, action: &PendingAction) -> Result<ActionStatus> {
        if self.dry_run {
            tracing::info!(
                action_id = %action.id,
                category = ?action.category,
                "Dry run: would execute action"
            );
            return Ok(ActionStatus::Completed);
        }

        tracing::info!(
            action_id = %action.id,
            category = ?action.category,
            packages = ?action.packages,
            "Executing automation action"
        );

        // TODO: Implement actual execution logic based on category
        // For now, this is a placeholder that would dispatch to:
        // - Security: security update installation
        // - Orphans: package removal
        // - Updates: package updates
        // - MajorUpgrades: major version upgrades
        // - Repair: CAS-based file restoration

        match action.category {
            AutomationCategory::Security => {
                // Would call into install system for security updates
                self.executed.push(action.id.clone());
                Ok(ActionStatus::Completed)
            }
            AutomationCategory::Orphans => {
                // Would call into remove system
                self.executed.push(action.id.clone());
                Ok(ActionStatus::Completed)
            }
            AutomationCategory::Updates => {
                // Would call into upgrade system
                self.executed.push(action.id.clone());
                Ok(ActionStatus::Completed)
            }
            AutomationCategory::MajorUpgrades => {
                // Would call into upgrade system with major version flag
                self.executed.push(action.id.clone());
                Ok(ActionStatus::Completed)
            }
            AutomationCategory::Repair => {
                // Would call into CAS restoration system
                self.executed.push(action.id.clone());
                Ok(ActionStatus::Completed)
            }
        }
    }

    /// Get list of successfully executed action IDs
    pub fn executed(&self) -> &[String] {
        &self.executed
    }

    /// Get list of failed actions with reasons
    pub fn failed(&self) -> &[(String, String)] {
        &self.failed
    }

    /// Get execution statistics
    pub fn stats(&self) -> (usize, usize) {
        (self.executed.len(), self.failed.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_builder() {
        let action = ActionBuilder::new(AutomationCategory::Updates, "Test update")
            .package("nginx")
            .package("redis")
            .risk(0.5)
            .detail("Important fix")
            .build();

        assert_eq!(action.category, AutomationCategory::Updates);
        assert_eq!(action.packages.len(), 2);
        assert!((action.risk_level - 0.5).abs() < 0.001);
        assert!(action.reversible);
    }

    #[test]
    fn test_security_update_action() {
        let action = security_update_action(
            &["openssl".to_string()],
            &["CVE-2024-1234".to_string()],
            "critical",
        );

        assert_eq!(action.category, AutomationCategory::Security);
        assert!(action.summary.contains("openssl"));
        assert!(action.details.iter().any(|d| d.contains("CVE-2024-1234")));
    }

    #[test]
    fn test_orphan_cleanup_action() {
        let action = orphan_cleanup_action(&[
            "libfoo".to_string(),
            "libbar".to_string(),
        ]);

        assert_eq!(action.category, AutomationCategory::Orphans);
        assert!(action.summary.contains("2 orphaned packages"));
    }

    #[test]
    fn test_executor_dry_run() {
        let mut executor = ActionExecutor::new(true);
        let action = ActionBuilder::new(AutomationCategory::Updates, "Test").build();

        let status = executor.execute(&action).unwrap();
        assert_eq!(status, ActionStatus::Completed);
    }
}
