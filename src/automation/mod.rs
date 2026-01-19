// src/automation/mod.rs

//! Automation system for self-healing, auto-updates, and AI-assisted operations.
//!
//! This module implements Conary's "suggest + confirm" model for autonomous operations:
//! - By default, all automation actions are suggested and require user confirmation
//! - Users can configure specific categories to run automatically
//! - AI assistance is opt-in and configurable per-feature
//!
//! # Design Principles
//!
//! 1. **Safety First**: Default mode is `Suggest` - never auto-execute without explicit config
//! 2. **Transparency**: All actions are logged and explained
//! 3. **Configurability**: Fine-grained control per category and package
//! 4. **Reversibility**: Rollback support for all automated changes

pub mod action;
pub mod check;
pub mod prompt;
pub mod scheduler;

use crate::error::Result;
use crate::model::{
    AiAssistConfig, AiAssistMode, AutomationCategory, AutomationConfig, AutomationMode,
};
use std::collections::HashMap;
use std::time::Duration;

/// An automation action that has been identified but not yet executed
#[derive(Debug, Clone)]
pub struct PendingAction {
    /// Unique identifier for this action
    pub id: String,

    /// Category of action
    pub category: AutomationCategory,

    /// Human-readable summary
    pub summary: String,

    /// Detailed explanation of what will happen
    pub details: Vec<String>,

    /// Packages affected
    pub packages: Vec<String>,

    /// Risk level (0.0 = no risk, 1.0 = high risk)
    pub risk_level: f64,

    /// Whether this requires a reboot
    pub requires_reboot: bool,

    /// Estimated duration
    pub estimated_duration: Option<Duration>,

    /// Can this action be rolled back?
    pub reversible: bool,

    /// Timestamp when this action was identified
    pub identified_at: chrono::DateTime<chrono::Utc>,

    /// Deadline by which this action should be applied (for security updates)
    pub deadline: Option<chrono::DateTime<chrono::Utc>>,
}

/// Result of user interaction with a pending action
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionDecision {
    /// User approved the action
    Approved,
    /// User rejected the action
    Rejected,
    /// User wants to defer to later
    Deferred { until: Option<chrono::DateTime<chrono::Utc>> },
    /// User wants more details
    NeedsDetails,
    /// Action should be auto-applied per configuration
    AutoApply,
}

/// Status of an automation action
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionStatus {
    /// Action is pending user decision
    Pending,
    /// Action has been approved and is queued
    Approved,
    /// Action is currently executing
    InProgress,
    /// Action completed successfully
    Completed,
    /// Action failed
    Failed { reason: String },
    /// Action was rolled back
    RolledBack { reason: String },
    /// Action was rejected by user
    Rejected,
    /// Action was deferred
    Deferred { until: Option<chrono::DateTime<chrono::Utc>> },
}

/// AI suggestion with confidence score
#[derive(Debug, Clone)]
pub struct AiSuggestion {
    /// The suggested action or response
    pub content: String,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,

    /// Reasoning behind the suggestion
    pub reasoning: String,

    /// Category of suggestion
    pub category: String,

    /// Whether this requires human approval based on config
    pub requires_approval: bool,
}

impl AiSuggestion {
    /// Check if this suggestion should be auto-applied based on config
    pub fn should_auto_apply(&self, config: &AiAssistConfig) -> bool {
        if !config.enabled {
            return false;
        }

        match config.mode {
            AiAssistMode::Advisory => false, // Never auto-apply in advisory mode
            AiAssistMode::Autonomous => {
                // Auto-apply if confidence is high enough and category doesn't require approval
                self.confidence >= config.confidence_threshold
                    && !config.require_human_approval.contains(&self.category)
            }
            AiAssistMode::Assisted => {
                // Auto-apply only high-confidence, low-risk suggestions
                self.confidence >= config.confidence_threshold
                    && !config.require_human_approval.contains(&self.category)
                    && !self.requires_approval
            }
        }
    }
}

/// Manager for automation actions
pub struct AutomationManager {
    /// Current automation configuration
    config: AutomationConfig,

    /// Pending actions awaiting decision
    pending: HashMap<String, PendingAction>,

    /// Action history
    history: Vec<(PendingAction, ActionStatus)>,
}

impl AutomationManager {
    /// Create a new automation manager with the given configuration
    pub fn new(config: AutomationConfig) -> Self {
        Self {
            config,
            pending: HashMap::new(),
            history: Vec::new(),
        }
    }

    /// Get the effective mode for a category
    pub fn effective_mode(&self, category: AutomationCategory) -> AutomationMode {
        let category_mode = match category {
            AutomationCategory::Security => self.config.security.mode.clone(),
            AutomationCategory::Orphans => self.config.orphans.mode.clone(),
            AutomationCategory::Updates => self.config.updates.mode.clone(),
            AutomationCategory::MajorUpgrades => self.config.major_upgrades.mode.clone(),
            AutomationCategory::Repair => self.config.repair.mode.clone(),
        };
        category_mode.unwrap_or_else(|| self.config.mode.clone())
    }

    /// Register a new pending action
    pub fn register_action(&mut self, action: PendingAction) -> ActionDecision {
        let mode = self.effective_mode(action.category);

        match mode {
            AutomationMode::Disabled => {
                // Don't even register, just note it was seen
                ActionDecision::Rejected
            }
            AutomationMode::Auto => {
                // Check if this specific action requires approval
                if self.requires_approval(&action) {
                    self.pending.insert(action.id.clone(), action);
                    ActionDecision::NeedsDetails
                } else {
                    ActionDecision::AutoApply
                }
            }
            AutomationMode::Suggest => {
                self.pending.insert(action.id.clone(), action);
                ActionDecision::NeedsDetails
            }
        }
    }

    /// Check if an action requires approval even in Auto mode
    fn requires_approval(&self, action: &PendingAction) -> bool {
        match action.category {
            AutomationCategory::MajorUpgrades => {
                // Major upgrades require approval unless explicitly allowed
                if self.config.major_upgrades.require_approval {
                    return true;
                }
                // Check if any affected package is in the allow_auto list
                !action.packages.iter().all(|p| {
                    self.config.major_upgrades.allow_auto.contains(p)
                })
            }
            AutomationCategory::Security => {
                // High-risk security changes might still need approval
                action.risk_level > 0.7
            }
            _ => action.risk_level > 0.8,
        }
    }

    /// Get all pending actions
    pub fn pending_actions(&self) -> Vec<&PendingAction> {
        self.pending.values().collect()
    }

    /// Get pending actions by category
    pub fn pending_by_category(&self, category: AutomationCategory) -> Vec<&PendingAction> {
        self.pending
            .values()
            .filter(|a| a.category == category)
            .collect()
    }

    /// Record a user decision for an action
    pub fn record_decision(&mut self, action_id: &str, decision: ActionDecision) -> Result<()> {
        if let Some(action) = self.pending.remove(action_id) {
            let status = match &decision {
                ActionDecision::Approved | ActionDecision::AutoApply => ActionStatus::Approved,
                ActionDecision::Rejected => ActionStatus::Rejected,
                ActionDecision::Deferred { until } => ActionStatus::Deferred { until: *until },
                ActionDecision::NeedsDetails => {
                    // Put it back, user wants more info
                    self.pending.insert(action_id.to_string(), action);
                    return Ok(());
                }
            };
            self.history.push((action, status));
        }
        Ok(())
    }

    /// Get summary of pending actions for display
    pub fn summary(&self) -> AutomationSummary {
        let mut summary = AutomationSummary::default();

        for action in self.pending.values() {
            match action.category {
                AutomationCategory::Security => summary.security_updates += 1,
                AutomationCategory::Orphans => summary.orphaned_packages += 1,
                AutomationCategory::Updates => summary.available_updates += 1,
                AutomationCategory::MajorUpgrades => summary.major_upgrades += 1,
                AutomationCategory::Repair => summary.integrity_issues += 1,
            }
        }

        summary.total = self.pending.len();
        summary
    }
}

/// Summary of automation status for display
#[derive(Debug, Clone, Default)]
pub struct AutomationSummary {
    /// Total pending actions
    pub total: usize,

    /// Security updates available
    pub security_updates: usize,

    /// Orphaned packages identified
    pub orphaned_packages: usize,

    /// Regular updates available
    pub available_updates: usize,

    /// Major upgrades available
    pub major_upgrades: usize,

    /// Integrity issues found
    pub integrity_issues: usize,
}

impl AutomationSummary {
    /// Format as a short status line
    pub fn status_line(&self) -> String {
        if self.total == 0 {
            return "System up to date".to_string();
        }

        let mut parts = Vec::new();

        if self.security_updates > 0 {
            parts.push(format!("{} security", self.security_updates));
        }
        if self.available_updates > 0 {
            parts.push(format!("{} updates", self.available_updates));
        }
        if self.orphaned_packages > 0 {
            parts.push(format!("{} orphans", self.orphaned_packages));
        }
        if self.major_upgrades > 0 {
            parts.push(format!("{} major", self.major_upgrades));
        }
        if self.integrity_issues > 0 {
            parts.push(format!("{} integrity", self.integrity_issues));
        }

        format!("{} pending: {}", self.total, parts.join(", "))
    }
}

/// Parse a duration string like "24h", "7d", "30m"
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Duration::from_secs(0));
    }

    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: u64 = num_str.parse().map_err(|_| {
        crate::error::Error::Config(format!("Invalid duration number: {}", num_str))
    })?;

    let seconds = match unit {
        "s" => num,
        "m" => num * 60,
        "h" => num * 3600,
        "d" => num * 86400,
        "w" => num * 604800,
        _ => {
            return Err(crate::error::Error::Config(format!(
                "Invalid duration unit: {}",
                unit
            )))
        }
    };

    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
        assert_eq!(parse_duration("1w").unwrap(), Duration::from_secs(604800));
    }

    #[test]
    fn test_automation_summary_status_line() {
        let summary = AutomationSummary {
            total: 5,
            security_updates: 2,
            available_updates: 3,
            orphaned_packages: 0,
            major_upgrades: 0,
            integrity_issues: 0,
        };

        assert_eq!(summary.status_line(), "5 pending: 2 security, 3 updates");
    }

    #[test]
    fn test_ai_suggestion_auto_apply() {
        let mut config = AiAssistConfig::default();

        let suggestion = AiSuggestion {
            content: "Install recommended package".to_string(),
            confidence: 0.95,
            reasoning: "Based on your usage patterns".to_string(),
            category: "recommendation".to_string(),
            requires_approval: false,
        };

        // Disabled by default
        assert!(!suggestion.should_auto_apply(&config));

        // Enable but advisory mode
        config.enabled = true;
        config.mode = AiAssistMode::Advisory;
        assert!(!suggestion.should_auto_apply(&config));

        // Assisted mode with high confidence
        config.mode = AiAssistMode::Assisted;
        assert!(suggestion.should_auto_apply(&config));

        // But not for security-related suggestions
        let security_suggestion = AiSuggestion {
            content: "Apply security patch".to_string(),
            confidence: 0.99,
            reasoning: "Critical vulnerability".to_string(),
            category: "security".to_string(),
            requires_approval: true,
        };
        assert!(!security_suggestion.should_auto_apply(&config));
    }

    #[test]
    fn test_automation_manager_modes() {
        let mut config = AutomationConfig::default();
        config.mode = AutomationMode::Suggest;
        config.security.mode = Some(AutomationMode::Auto);

        let manager = AutomationManager::new(config);

        // Security uses Auto mode
        assert_eq!(
            manager.effective_mode(AutomationCategory::Security),
            AutomationMode::Auto
        );

        // Others inherit global Suggest mode
        assert_eq!(
            manager.effective_mode(AutomationCategory::Updates),
            AutomationMode::Suggest
        );
    }
}
