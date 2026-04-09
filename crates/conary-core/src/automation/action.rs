// conary-core/src/automation/action.rs

//! Action definitions and planning for the automation system.

use super::{ActionPayload, InstalledPackageRef, PendingAction};
use crate::error::Result;
use crate::model::AutomationCategory;
use chrono::Utc;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Builder for creating pending actions
pub struct ActionBuilder {
    category: AutomationCategory,
    summary: String,
    details: Vec<String>,
    packages: Vec<String>,
    payload: Option<ActionPayload>,
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
            payload: None,
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

    /// Set the typed payload for this action.
    pub fn payload(mut self, payload: ActionPayload) -> Self {
        self.payload = Some(payload);
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
        let payload = self
            .payload
            .expect("ActionBuilder::build() requires a typed payload");
        let id = stable_action_id(self.category, &self.packages, &payload);

        PendingAction {
            id,
            category: self.category,
            summary: self.summary,
            details: self.details,
            packages: self.packages,
            payload,
            risk_level: self.risk_level,
            requires_reboot: self.requires_reboot,
            reversible: self.reversible,
            identified_at: Utc::now(),
            deadline: self.deadline,
            estimated_duration: None,
        }
    }
}

fn stable_action_id(
    category: AutomationCategory,
    packages: &[String],
    payload: &ActionPayload,
) -> String {
    let mut hasher = DefaultHasher::new();
    category.hash(&mut hasher);
    packages.hash(&mut hasher);
    payload.hash(&mut hasher);
    format!("automation-{:016x}", hasher.finish())
}

/// Creates a security update action
pub fn security_update_action(
    packages: &[String],
    target_version: &str,
    architecture: Option<&str>,
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
        .payload(ActionPayload::UpdatePackage {
            target_version: target_version.to_string(),
            architecture: architecture.map(str::to_string),
        })
        .risk(risk);

    if !cve_ids.is_empty() {
        builder = builder.detail(format!("CVEs addressed: {}", cve_ids.join(", ")));
    }

    builder = builder.detail(format!("Severity: {}", severity));

    builder.build()
}

/// Creates an orphan cleanup action
pub fn orphan_cleanup_action(
    installed: &[InstalledPackageRef],
    packages: &[String],
) -> PendingAction {
    let summary = if packages.len() == 1 {
        format!("Remove orphaned package: {}", packages[0])
    } else {
        format!("Remove {} orphaned packages", packages.len())
    };

    ActionBuilder::new(AutomationCategory::Orphans, summary)
        .packages(packages.iter().cloned())
        .payload(ActionPayload::RemovePackages {
            installed: installed.to_vec(),
        })
        .detail("These packages are no longer required by any installed package")
        .risk(0.3)
        .build()
}

/// Creates a package update action
pub fn package_update_action(
    package: &str,
    current_version: &str,
    new_version: &str,
    architecture: Option<&str>,
) -> PendingAction {
    ActionBuilder::new(
        AutomationCategory::Updates,
        format!(
            "Update {} from {} to {}",
            package, current_version, new_version
        ),
    )
    .package(package)
    .payload(ActionPayload::UpdatePackage {
        target_version: new_version.to_string(),
        architecture: architecture.map(str::to_string),
    })
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
    architecture: Option<&str>,
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
    .payload(ActionPayload::UpdatePackage {
        target_version: new_version.to_string(),
        architecture: architecture.map(str::to_string),
    })
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
pub fn integrity_repair_action(files: &[String], installed: InstalledPackageRef) -> PendingAction {
    let summary = format!(
        "Repair {} corrupted files in {}",
        files.len(),
        installed.name
    );

    let mut builder = ActionBuilder::new(AutomationCategory::Repair, summary)
        .package(installed.name.clone())
        .payload(ActionPayload::RestorePackage { installed })
        .risk(0.4);

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedOp {
    Install {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
    Remove {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
    Restore {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionPlan {
    pub ops: Vec<PlannedOp>,
    pub category: AutomationCategory,
    pub action_id: String,
}

/// Executor for automation actions
pub struct ActionExecutor;

impl Default for ActionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionExecutor {
    /// Create a new action executor
    pub fn new() -> Self {
        Self
    }

    pub fn plan(&self, action: &PendingAction) -> Result<ActionPlan> {
        let ops = match (&action.category, &action.payload) {
            (
                AutomationCategory::Security
                | AutomationCategory::Updates
                | AutomationCategory::MajorUpgrades,
                ActionPayload::UpdatePackage {
                    target_version,
                    architecture,
                },
            ) => action
                .packages
                .iter()
                .cloned()
                .map(|package| PlannedOp::Install {
                    package,
                    version: Some(target_version.clone()),
                    architecture: architecture.clone(),
                })
                .collect(),
            (AutomationCategory::Orphans, ActionPayload::RemovePackages { installed }) => installed
                .iter()
                .cloned()
                .map(|installed| PlannedOp::Remove {
                    package: installed.name,
                    version: installed.version,
                    architecture: installed.architecture,
                })
                .collect(),
            (AutomationCategory::Repair, ActionPayload::RestorePackage { installed }) => {
                vec![PlannedOp::Restore {
                    package: installed.name.clone(),
                    version: installed.version.clone(),
                    architecture: installed.architecture.clone(),
                }]
            }
            (category, payload) => {
                return Err(crate::error::Error::ConfigError(format!(
                    "mismatched automation category/payload: {category:?} cannot use {payload:?}"
                )));
            }
        };

        Ok(ActionPlan {
            ops,
            category: action.category,
            action_id: action.id.clone(),
        })
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
            .payload(ActionPayload::UpdatePackage {
                target_version: "1.27.0".to_string(),
                architecture: Some("x86_64".to_string()),
            })
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
            "3.0.15-1",
            Some("x86_64"),
            &["CVE-2024-1234".to_string()],
            "critical",
        );

        assert_eq!(action.category, AutomationCategory::Security);
        assert!(action.summary.contains("openssl"));
        assert!(action.details.iter().any(|d| d.contains("CVE-2024-1234")));
    }

    #[test]
    fn test_security_update_action_sets_update_payload() {
        let action = security_update_action(
            &["openssl".to_string()],
            "3.0.15-1",
            Some("x86_64"),
            &["CVE-2024-1234".to_string()],
            "critical",
        );

        assert_eq!(
            action.payload,
            ActionPayload::UpdatePackage {
                target_version: "3.0.15-1".to_string(),
                architecture: Some("x86_64".to_string()),
            }
        );
    }

    #[test]
    fn test_orphan_cleanup_action_sets_remove_payload() {
        let action = orphan_cleanup_action(
            &[InstalledPackageRef {
                name: "unused-lib".to_string(),
                version: Some("1.2.3-1".to_string()),
                architecture: Some("x86_64".to_string()),
            }],
            &["unused-lib".to_string()],
        );

        assert_eq!(
            action.payload,
            ActionPayload::RemovePackages {
                installed: vec![InstalledPackageRef {
                    name: "unused-lib".to_string(),
                    version: Some("1.2.3-1".to_string()),
                    architecture: Some("x86_64".to_string()),
                }],
            }
        );
    }

    #[test]
    fn test_integrity_repair_action_sets_restore_payload() {
        let action = integrity_repair_action(
            &["/usr/bin/foo".to_string()],
            InstalledPackageRef {
                name: "foo".to_string(),
                version: Some("1.0.0-1".to_string()),
                architecture: Some("x86_64".to_string()),
            },
        );

        assert_eq!(
            action.payload,
            ActionPayload::RestorePackage {
                installed: InstalledPackageRef {
                    name: "foo".to_string(),
                    version: Some("1.0.0-1".to_string()),
                    architecture: Some("x86_64".to_string()),
                },
            }
        );
    }

    #[test]
    fn test_same_logical_action_builds_stable_id() {
        let action_a = ActionBuilder::new(AutomationCategory::Updates, "Test update")
            .package("nginx")
            .detail("Current version: 1.26.0")
            .detail("New version: 1.26.1")
            .payload(ActionPayload::UpdatePackage {
                target_version: "1.26.1".to_string(),
                architecture: Some("x86_64".to_string()),
            })
            .build();

        let action_b = ActionBuilder::new(AutomationCategory::Updates, "Test update")
            .package("nginx")
            .detail("Current version: 1.26.0")
            .detail("New version: 1.26.1")
            .payload(ActionPayload::UpdatePackage {
                target_version: "1.26.1".to_string(),
                architecture: Some("x86_64".to_string()),
            })
            .build();

        assert_eq!(action_a.id, action_b.id);
    }

    #[test]
    fn test_payload_change_changes_action_id() {
        let update = ActionBuilder::new(AutomationCategory::Updates, "Test update")
            .package("nginx")
            .payload(ActionPayload::UpdatePackage {
                target_version: "1.26.1".to_string(),
                architecture: Some("x86_64".to_string()),
            })
            .build();

        let other_update = ActionBuilder::new(AutomationCategory::Updates, "Test update")
            .package("nginx")
            .payload(ActionPayload::UpdatePackage {
                target_version: "1.27.0".to_string(),
                architecture: Some("x86_64".to_string()),
            })
            .build();

        assert_ne!(update.id, other_update.id);
    }

    #[test]
    fn test_orphan_cleanup_action() {
        let action = orphan_cleanup_action(
            &[
                InstalledPackageRef {
                    name: "libfoo".to_string(),
                    version: Some("1.0.0".to_string()),
                    architecture: Some("x86_64".to_string()),
                },
                InstalledPackageRef {
                    name: "libbar".to_string(),
                    version: Some("2.0.0".to_string()),
                    architecture: Some("x86_64".to_string()),
                },
            ],
            &["libfoo".to_string(), "libbar".to_string()],
        );

        assert_eq!(action.category, AutomationCategory::Orphans);
        assert!(action.summary.contains("2 orphaned packages"));
    }

    #[test]
    fn test_plan_update_action_produces_install_with_version() {
        let planner = ActionExecutor::new();
        let action = package_update_action("nginx", "1.26.0", "1.26.1", Some("x86_64"));

        let plan = planner.plan(&action).unwrap();
        assert_eq!(
            plan.ops,
            vec![PlannedOp::Install {
                package: "nginx".to_string(),
                version: Some("1.26.1".to_string()),
                architecture: Some("x86_64".to_string()),
            }]
        );
    }

    #[test]
    fn test_plan_major_upgrade_produces_install_with_version() {
        let planner = ActionExecutor::new();
        let action = major_upgrade_action(
            "postgresql",
            "15.3",
            "16.0",
            Some("x86_64"),
            &["breaking".to_string()],
        );

        let plan = planner.plan(&action).unwrap();
        assert_eq!(
            plan.ops,
            vec![PlannedOp::Install {
                package: "postgresql".to_string(),
                version: Some("16.0".to_string()),
                architecture: Some("x86_64".to_string()),
            }]
        );
    }

    #[test]
    fn test_plan_repair_action_produces_restore() {
        let planner = ActionExecutor::new();
        let action = integrity_repair_action(
            &["/usr/bin/foo".to_string()],
            InstalledPackageRef {
                name: "foo".to_string(),
                version: Some("1.0.0-1".to_string()),
                architecture: Some("x86_64".to_string()),
            },
        );

        let plan = planner.plan(&action).unwrap();
        assert_eq!(
            plan.ops,
            vec![PlannedOp::Restore {
                package: "foo".to_string(),
                version: Some("1.0.0-1".to_string()),
                architecture: Some("x86_64".to_string()),
            }]
        );
    }

    #[test]
    fn test_plan_mismatched_payload_errors() {
        let planner = ActionExecutor::new();
        let action = ActionBuilder::new(AutomationCategory::Security, "Wrong payload")
            .package("nginx")
            .payload(ActionPayload::RemovePackages {
                installed: vec![InstalledPackageRef {
                    name: "nginx".to_string(),
                    version: Some("1.26.0".to_string()),
                    architecture: Some("x86_64".to_string()),
                }],
            })
            .build();

        let error = planner.plan(&action).unwrap_err();
        assert!(error.to_string().contains("mismatched"));
    }
}
