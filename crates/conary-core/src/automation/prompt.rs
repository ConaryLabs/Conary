// crates/conary-core/src/automation/prompt.rs
//! Typed automation choices supplied by a caller; terminal interaction belongs to the CLI.

use super::{AutomationManager, PendingAction};
use crate::model::AutomationCategory;

/// Response to the summary prompt
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryResponse {
    /// Apply all pending actions
    ApplyAll,
    /// Review a specific category
    ReviewCategory(AutomationCategory),
    /// Show details for all actions
    ShowDetails,
    /// Open configuration
    Configure,
    /// Exit without changes
    Exit,
}

/// Select pending actions only for an explicit application decision.
pub fn actions_for_decision(
    manager: &AutomationManager,
    decision: &SummaryResponse,
) -> Vec<PendingAction> {
    match decision {
        SummaryResponse::ApplyAll => manager.pending_actions(),
        SummaryResponse::ReviewCategory(category) => manager.pending_by_category(*category),
        SummaryResponse::ShowDetails | SummaryResponse::Configure | SummaryResponse::Exit => {
            Vec::new()
        }
    }
    .into_iter()
    .cloned()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{ActionPayload, InstalledPackageRef};
    use crate::model::AutomationConfig;

    #[test]
    fn only_explicit_application_decisions_select_actions() {
        let mut manager = AutomationManager::new(AutomationConfig::default());
        for (id, category) in [
            ("security", AutomationCategory::Security),
            ("orphan", AutomationCategory::Orphans),
        ] {
            manager.register_action(PendingAction {
                id: id.into(),
                category,
                summary: id.into(),
                details: vec![],
                packages: vec![id.into()],
                payload: ActionPayload::RemovePackages {
                    installed: vec![InstalledPackageRef {
                        name: id.into(),
                        version: None,
                        architecture: None,
                    }],
                },
                risk_level: 0.1,
                requires_reboot: false,
                estimated_duration: None,
                reversible: true,
                identified_at: chrono::Utc::now(),
                deadline: None,
            });
        }
        assert_eq!(
            actions_for_decision(&manager, &SummaryResponse::ApplyAll).len(),
            2
        );
        let security = actions_for_decision(
            &manager,
            &SummaryResponse::ReviewCategory(AutomationCategory::Security),
        );
        assert_eq!(security.len(), 1);
        assert_eq!(security[0].id, "security");
        for decision in [
            SummaryResponse::ShowDetails,
            SummaryResponse::Configure,
            SummaryResponse::Exit,
        ] {
            assert!(actions_for_decision(&manager, &decision).is_empty());
        }
        assert_eq!(manager.pending_actions().len(), 2);
    }
}
