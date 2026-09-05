// crates/conary-core/src/automation/prompt.rs
//! Typed automation choices supplied by a caller; terminal interaction belongs to the CLI.

use super::PendingAction;
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

/// Filter an explicit application decision without changing the caller's check order.
pub fn actions_for_decision(
    actions: &[PendingAction],
    decision: &SummaryResponse,
) -> Vec<PendingAction> {
    actions
        .iter()
        .filter(|action| match decision {
            SummaryResponse::ApplyAll => true,
            SummaryResponse::ReviewCategory(category) => action.category == *category,
            SummaryResponse::ShowDetails | SummaryResponse::Configure | SummaryResponse::Exit => {
                false
            }
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{ActionPayload, InstalledPackageRef};

    #[test]
    fn category_selection_keeps_the_original_relative_order() {
        use crate::automation::action::package_update_action;
        let mut first = package_update_action("first", "1", "2", None);
        first.id = "z-first".into();
        let mut security = package_update_action("security", "1", "2", None);
        security.category = AutomationCategory::Security;
        let mut last = package_update_action("last", "1", "2", None);
        last.id = "a-last".into();
        let actions = [first, security, last];
        let selected = actions_for_decision(
            &actions,
            &SummaryResponse::ReviewCategory(AutomationCategory::Updates),
        );
        assert_eq!(
            selected
                .iter()
                .map(|action| action.id.as_str())
                .collect::<Vec<_>>(),
            ["z-first", "a-last"]
        );
        let selected = actions_for_decision(&actions, &SummaryResponse::ApplyAll);
        assert_eq!(
            selected
                .iter()
                .map(|action| action.id.as_str())
                .collect::<Vec<_>>(),
            actions
                .iter()
                .map(|action| action.id.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn only_explicit_application_decisions_select_actions() {
        let mut actions = Vec::new();
        for (id, category) in [
            ("security", AutomationCategory::Security),
            ("orphan", AutomationCategory::Orphans),
        ] {
            actions.push(PendingAction {
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
            actions_for_decision(&actions, &SummaryResponse::ApplyAll).len(),
            2
        );
        let security = actions_for_decision(
            &actions,
            &SummaryResponse::ReviewCategory(AutomationCategory::Security),
        );
        assert_eq!(security.len(), 1);
        assert_eq!(security[0].id, "security");
        for decision in [
            SummaryResponse::ShowDetails,
            SummaryResponse::Configure,
            SummaryResponse::Exit,
        ] {
            assert!(actions_for_decision(&actions, &decision).is_empty());
        }
        assert_eq!(actions.len(), 2);
    }
}
