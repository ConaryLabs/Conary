// conary-core/src/model/parser/automation.rs

//! Automation policy types owned by the system-model parser.

use serde::{Deserialize, Serialize};

/// Automation mode - how autonomous should the system be?
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AutomationMode {
    /// Always suggest changes and wait for confirmation (default, safest)
    #[default]
    Suggest,
    /// Automatically apply changes without confirmation
    Auto,
    /// Completely disabled - don't even check
    Disabled,
}

/// Configuration for automated system maintenance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationConfig {
    /// Global automation mode (default for all categories)
    #[serde(default)]
    pub mode: AutomationMode,

    /// How often to check for automation actions (e.g., "1h", "6h", "daily")
    #[serde(default = "default_check_interval")]
    pub check_interval: String,

    /// Email/webhook notifications for automation actions
    #[serde(default)]
    pub notify: Vec<String>,

    /// Security update automation
    #[serde(default)]
    pub security: SecurityAutomation,

    /// Orphaned dependency cleanup
    #[serde(default)]
    pub orphans: OrphanAutomation,

    /// Regular update automation
    #[serde(default)]
    pub updates: UpdateAutomation,

    /// Major version upgrade handling
    #[serde(default)]
    pub major_upgrades: MajorUpgradeAutomation,

    /// Self-healing/integrity repair
    #[serde(default)]
    pub repair: RepairAutomation,

    /// Deferred assistant-related configuration.
    #[serde(default)]
    pub ai_assist: AiAssistConfig,
}

fn default_check_interval() -> String {
    "6h".to_string()
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            mode: AutomationMode::Suggest,
            check_interval: default_check_interval(),
            notify: Vec::new(),
            security: SecurityAutomation::default(),
            orphans: OrphanAutomation::default(),
            updates: UpdateAutomation::default(),
            major_upgrades: MajorUpgradeAutomation::default(),
            repair: RepairAutomation::default(),
            ai_assist: AiAssistConfig::default(),
        }
    }
}

/// Security update automation settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAutomation {
    /// Override mode for security updates (inherits from global if None)
    #[serde(default)]
    pub mode: Option<AutomationMode>,

    /// Maximum time window to apply security updates (e.g., "24h", "7d")
    #[serde(default = "default_security_window")]
    pub within: String,

    /// Severity levels to auto-apply (if mode is Auto): critical, high, medium, low
    #[serde(default = "default_security_severities")]
    pub severities: Vec<String>,

    /// Reboot policy after security updates: "never", "suggest", "auto"
    #[serde(default = "default_reboot_policy")]
    pub reboot: String,
}

fn default_security_window() -> String {
    "24h".to_string()
}

fn default_security_severities() -> Vec<String> {
    vec!["critical".to_string(), "high".to_string()]
}

fn default_reboot_policy() -> String {
    "suggest".to_string()
}

impl Default for SecurityAutomation {
    fn default() -> Self {
        Self {
            mode: None,
            within: default_security_window(),
            severities: default_security_severities(),
            reboot: default_reboot_policy(),
        }
    }
}

/// Orphaned package cleanup settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrphanAutomation {
    /// Override mode for orphan cleanup
    #[serde(default)]
    pub mode: Option<AutomationMode>,

    /// Grace period before suggesting/removing orphans (e.g., "30d", "7d")
    #[serde(default = "default_orphan_grace")]
    pub after: String,

    /// Packages to never auto-remove even if orphaned
    #[serde(default)]
    pub keep: Vec<String>,
}

fn default_orphan_grace() -> String {
    "30d".to_string()
}

impl Default for OrphanAutomation {
    fn default() -> Self {
        Self {
            mode: None,
            after: default_orphan_grace(),
            keep: Vec::new(),
        }
    }
}

/// Regular update automation settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAutomation {
    /// Override mode for updates
    #[serde(default)]
    pub mode: Option<AutomationMode>,

    /// How often to check for updates (e.g., "daily", "weekly")
    #[serde(default = "default_update_frequency")]
    pub frequency: String,

    /// Time window for applying updates (e.g., "02:00-06:00")
    #[serde(default)]
    pub window: Option<String>,

    /// Packages to exclude from auto-updates
    #[serde(default)]
    pub exclude: Vec<String>,
}

fn default_update_frequency() -> String {
    "weekly".to_string()
}

impl Default for UpdateAutomation {
    fn default() -> Self {
        Self {
            mode: None,
            frequency: default_update_frequency(),
            window: None,
            exclude: Vec::new(),
        }
    }
}

/// Major version upgrade handling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MajorUpgradeAutomation {
    /// Override mode (defaults to Suggest - always ask for major upgrades)
    #[serde(default = "default_major_mode")]
    pub mode: Option<AutomationMode>,

    /// Require explicit approval even in Auto mode
    #[serde(default = "default_require_approval")]
    pub require_approval: bool,

    /// Packages where major upgrades are allowed in Auto mode
    #[serde(default)]
    pub allow_auto: Vec<String>,
}

fn default_major_mode() -> Option<AutomationMode> {
    Some(AutomationMode::Suggest)
}

fn default_require_approval() -> bool {
    true
}

impl Default for MajorUpgradeAutomation {
    fn default() -> Self {
        Self {
            mode: default_major_mode(),
            require_approval: default_require_approval(),
            allow_auto: Vec::new(),
        }
    }
}

/// Self-healing and integrity repair settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairAutomation {
    /// Override mode for repair operations
    #[serde(default)]
    pub mode: Option<AutomationMode>,

    /// Enable periodic integrity checking
    #[serde(default)]
    pub integrity_check: bool,

    /// Interval for integrity checks (e.g., "24h", "weekly")
    #[serde(default = "default_integrity_interval")]
    pub check_interval: String,

    /// Auto-repair corrupted files from CAS
    #[serde(default)]
    pub auto_restore: bool,

    /// Rollback triggers (health checks)
    #[serde(default)]
    pub rollback_triggers: Vec<RollbackTrigger>,
}

fn default_integrity_interval() -> String {
    "24h".to_string()
}

impl Default for RepairAutomation {
    fn default() -> Self {
        Self {
            mode: None,
            integrity_check: false,
            check_interval: default_integrity_interval(),
            auto_restore: false,
            rollback_triggers: Vec::new(),
        }
    }
}

/// Health check that can trigger automatic rollback
///
/// SAFETY: The `command` field MUST NOT be passed through a shell (`/bin/sh -c`).
/// It should be tokenized with `shlex::split()` or split on whitespace and
/// executed via `Command::new(parts[0]).args(&parts[1..])` to prevent shell
/// injection from model TOML files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackTrigger {
    /// Name for this trigger (for logging)
    pub name: String,

    /// Command to run as health check (tokenized, NOT passed to shell)
    pub command: String,

    /// Timeout for health check (e.g., "30s")
    #[serde(default = "default_trigger_timeout")]
    pub timeout: String,

    /// Time window after changes to monitor (e.g., "5m")
    #[serde(default = "default_failure_window")]
    pub failure_window: String,

    /// Auto-rollback on failure
    #[serde(default)]
    pub auto_rollback: bool,
}

fn default_trigger_timeout() -> String {
    "30s".to_string()
}

fn default_failure_window() -> String {
    "5m".to_string()
}

/// Deferred assistant-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAssistConfig {
    /// Enable reserved assistant configuration fields.
    #[serde(default)]
    pub enabled: bool,

    /// Reserved assistant interaction mode.
    #[serde(default)]
    pub mode: AiAssistMode,

    /// Reserve intent-based package resolution behavior.
    #[serde(default)]
    pub intent_resolution: bool,

    /// Reserve scriptlet translation behavior.
    #[serde(default)]
    pub scriptlet_translation: bool,

    /// Reserve natural language system query behavior.
    #[serde(default)]
    pub natural_language: bool,

    /// Reserved confidence threshold for future assistant suggestions (0.0-1.0).
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,

    /// Categories where future assistant suggestions require human approval.
    #[serde(default = "default_require_human_approval")]
    pub require_human_approval: Vec<String>,
}

/// Reserved assistant interaction policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AiAssistMode {
    /// Future assistant provides suggestions; user must confirm all actions.
    #[default]
    Advisory,
    /// Future assistant can auto-apply low-risk suggestions, asks for others.
    Assisted,
    /// Future assistant operates autonomously within configured bounds.
    Autonomous,
}

fn default_confidence_threshold() -> f64 {
    0.9
}

fn default_require_human_approval() -> Vec<String> {
    vec![
        "security".to_string(),
        "removal".to_string(),
        "major_upgrade".to_string(),
    ]
}

impl Default for AiAssistConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: AiAssistMode::Advisory,
            intent_resolution: false,
            scriptlet_translation: false,
            natural_language: false,
            confidence_threshold: default_confidence_threshold(),
            require_human_approval: default_require_human_approval(),
        }
    }
}

/// Categories of automation actions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutomationCategory {
    /// Security updates
    Security,
    /// Orphaned package cleanup
    Orphans,
    /// Regular package updates
    Updates,
    /// Major version upgrades
    MajorUpgrades,
    /// Integrity repair
    Repair,
}

impl AutomationCategory {
    /// Get display name for the category
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Security => "Security Updates",
            Self::Orphans => "Orphaned Packages",
            Self::Updates => "Package Updates",
            Self::MajorUpgrades => "Major Upgrades",
            Self::Repair => "Integrity Repair",
        }
    }
}

/// Reserved assistant feature flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFeature {
    /// Intent-based package resolution
    IntentResolution,
    /// Scriptlet translation
    ScriptletTranslation,
    /// Natural language queries
    NaturalLanguage,
}
