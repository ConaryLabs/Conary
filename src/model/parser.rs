// src/model/parser.rs

//! Parser for system model TOML files.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use super::{ModelError, ModelResult};

/// Current model file version
pub const MODEL_VERSION: u32 = 1;

/// The main system model configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemModel {
    /// Core model configuration
    #[serde(rename = "model")]
    pub config: ModelConfig,

    /// Pinned package versions (package name -> version pattern)
    #[serde(default)]
    pub pin: HashMap<String, String>,

    /// Optional packages (install if available)
    #[serde(default)]
    pub optional: OptionalConfig,

    /// Derived package definitions
    #[serde(default)]
    pub derive: Vec<DerivedPackage>,

    /// Remote model includes
    #[serde(default)]
    pub include: IncludeConfig,

    /// Automation configuration (self-healing, auto-updates, etc.)
    #[serde(default)]
    pub automation: AutomationConfig,

    /// Federation configuration (CAS sharing across machines)
    #[serde(default)]
    pub federation: FederationConfig,
}

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

    /// AI-assisted features (intent resolution, scriptlet translation, etc.)
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackTrigger {
    /// Name for this trigger (for logging)
    pub name: String,

    /// Command to run as health check
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

/// AI-assisted features configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAssistConfig {
    /// Enable AI assistance features
    #[serde(default)]
    pub enabled: bool,

    /// AI interaction mode
    #[serde(default)]
    pub mode: AiAssistMode,

    /// Enable intent-based package resolution
    #[serde(default)]
    pub intent_resolution: bool,

    /// Enable AI-assisted scriptlet translation
    #[serde(default)]
    pub scriptlet_translation: bool,

    /// Enable natural language system queries
    #[serde(default)]
    pub natural_language: bool,

    /// Confidence threshold for auto-applying AI suggestions (0.0-1.0)
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,

    /// Categories where AI suggestions require human approval
    #[serde(default = "default_require_human_approval")]
    pub require_human_approval: Vec<String>,
}

/// How AI assistance interacts with the user
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AiAssistMode {
    /// AI provides suggestions, user must confirm all actions (default)
    #[default]
    Advisory,
    /// AI can auto-apply low-risk suggestions, asks for others
    Assisted,
    /// AI operates autonomously within configured bounds
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

/// AI assistance feature flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFeature {
    /// Intent-based package resolution
    IntentResolution,
    /// AI-assisted scriptlet translation
    ScriptletTranslation,
    /// Natural language queries
    NaturalLanguage,
}

/// Core model configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Model file version (for forward compatibility)
    #[serde(default = "default_version")]
    pub version: u32,

    /// Package search path (label specs, checked in order)
    #[serde(default)]
    pub search: Vec<String>,

    /// Packages to install and keep installed
    #[serde(default)]
    pub install: Vec<String>,

    /// Packages to exclude (never install, even as dependencies)
    #[serde(default)]
    pub exclude: Vec<String>,
}

fn default_version() -> u32 {
    MODEL_VERSION
}

/// Optional packages configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OptionalConfig {
    /// Optional packages to install if available
    #[serde(default)]
    pub packages: Vec<String>,
}

/// A derived package definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedPackage {
    /// Name of the derived package
    pub name: String,

    /// Package to derive from
    pub from: String,

    /// Version handling: "inherit" or specific version
    #[serde(default = "default_version_inherit")]
    pub version: String,

    /// Patches to apply (paths relative to model file)
    #[serde(default)]
    pub patches: Vec<String>,

    /// Files to override (dest path -> source path)
    #[serde(default)]
    pub override_files: HashMap<String, String>,
}

fn default_version_inherit() -> String {
    "inherit".to_string()
}

/// Configuration for including remote models/collections
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IncludeConfig {
    /// Remote models to include (e.g., "group-base@repo:branch")
    #[serde(default)]
    pub models: Vec<String>,

    /// Conflict resolution strategy when local and remote define same package
    #[serde(default)]
    pub on_conflict: ConflictStrategy,
}

/// Strategy for resolving conflicts between local and remote model definitions
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConflictStrategy {
    /// Local definitions take precedence (default)
    #[default]
    Local,
    /// Remote definitions take precedence
    Remote,
    /// Fail on any conflict
    Error,
}

/// Peer tier in the federation hierarchy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FederationTier {
    /// WAN hub, requires mTLS
    RegionHub,
    /// Site-local cache (rack-level)
    CellHub,
    /// Individual node (default)
    #[default]
    Leaf,
}

/// Federation configuration for CAS sharing across machines
///
/// Enables multiple machines to share content-addressable storage chunks
/// over a network, reducing bandwidth and storage by deduplicating content.
///
/// # Example (TOML)
///
/// ```toml
/// [federation]
/// enabled = true
/// tier = "leaf"
/// region_hubs = ["https://remi.conary.io:7891"]
/// cell_hubs = ["http://rack-cache.local:7891"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationConfig {
    /// Enable federation (default: false)
    #[serde(default)]
    pub enabled: bool,

    /// Optional node identifier (auto-generated if not set)
    #[serde(default)]
    pub node_id: Option<String>,

    /// What role is this node? (default: leaf)
    #[serde(default)]
    pub tier: FederationTier,

    /// Cell-local hubs (fast path, LAN)
    #[serde(default)]
    pub cell_hubs: Vec<String>,

    /// WAN hubs (mTLS required in production)
    #[serde(default)]
    pub region_hubs: Vec<String>,

    /// Enable mDNS for LAN peer discovery (default: false)
    #[serde(default)]
    pub enable_mdns: bool,

    /// Number of candidate peers per chunk (default: 3)
    #[serde(default = "default_rendezvous_k")]
    pub rendezvous_k: usize,

    /// Try cell peers before region peers (default: true)
    #[serde(default = "default_prefer_cell")]
    pub prefer_cell: bool,

    /// Failures before opening circuit breaker (default: 5)
    #[serde(default = "default_circuit_threshold")]
    pub circuit_threshold: u32,

    /// Cooldown before retrying open circuit (default: 30)
    #[serde(default = "default_circuit_cooldown")]
    pub circuit_cooldown_secs: u64,

    /// Random jitter factor for cooldowns (default: 0.5 = 50%)
    #[serde(default = "default_jitter_factor")]
    pub jitter_factor: f32,

    /// Per-request timeout in milliseconds (default: 5000)
    #[serde(default = "default_request_timeout")]
    pub request_timeout_ms: u64,

    /// Maximum chunk size to accept (default: 512KB)
    #[serde(default = "default_max_chunk_size")]
    pub max_chunk_size: usize,

    /// Listen port for this node (if acting as hub)
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,

    /// Upstream URL for pull-through caching (cell hubs only)
    #[serde(default)]
    pub upstream: Option<String>,
}

fn default_rendezvous_k() -> usize {
    3
}

fn default_prefer_cell() -> bool {
    true
}

fn default_circuit_threshold() -> u32 {
    5
}

fn default_circuit_cooldown() -> u64 {
    30
}

fn default_jitter_factor() -> f32 {
    0.5
}

fn default_request_timeout() -> u64 {
    5000
}

fn default_max_chunk_size() -> usize {
    512 * 1024 // 512KB
}

fn default_listen_port() -> u16 {
    7891
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            node_id: None,
            tier: FederationTier::Leaf,
            cell_hubs: Vec::new(),
            region_hubs: Vec::new(),
            enable_mdns: false,
            rendezvous_k: default_rendezvous_k(),
            prefer_cell: default_prefer_cell(),
            circuit_threshold: default_circuit_threshold(),
            circuit_cooldown_secs: default_circuit_cooldown(),
            jitter_factor: default_jitter_factor(),
            request_timeout_ms: default_request_timeout(),
            max_chunk_size: default_max_chunk_size(),
            listen_port: default_listen_port(),
            upstream: None,
        }
    }
}

impl SystemModel {
    /// Create a new empty system model
    pub fn new() -> Self {
        Self {
            config: ModelConfig {
                version: MODEL_VERSION,
                search: Vec::new(),
                install: Vec::new(),
                exclude: Vec::new(),
            },
            pin: HashMap::new(),
            optional: OptionalConfig::default(),
            derive: Vec::new(),
            include: IncludeConfig::default(),
            automation: AutomationConfig::default(),
            federation: FederationConfig::default(),
        }
    }

    /// Get the effective automation mode for a category
    pub fn effective_mode(&self, category: AutomationCategory) -> AutomationMode {
        let category_mode = match category {
            AutomationCategory::Security => self.automation.security.mode.clone(),
            AutomationCategory::Orphans => self.automation.orphans.mode.clone(),
            AutomationCategory::Updates => self.automation.updates.mode.clone(),
            AutomationCategory::MajorUpgrades => self.automation.major_upgrades.mode.clone(),
            AutomationCategory::Repair => self.automation.repair.mode.clone(),
        };
        // Category-specific mode overrides global, or fall back to global
        category_mode.unwrap_or_else(|| self.automation.mode.clone())
    }

    /// Check if AI assist is enabled for a specific feature
    pub fn ai_assist_enabled(&self, feature: AiFeature) -> bool {
        if !self.automation.ai_assist.enabled {
            return false;
        }
        match feature {
            AiFeature::IntentResolution => self.automation.ai_assist.intent_resolution,
            AiFeature::ScriptletTranslation => self.automation.ai_assist.scriptlet_translation,
            AiFeature::NaturalLanguage => self.automation.ai_assist.natural_language,
        }
    }

    /// Check if this model has any remote includes
    pub fn has_includes(&self) -> bool {
        !self.include.models.is_empty()
    }

    /// Get pinned version pattern for a package, if any
    pub fn get_pin(&self, package: &str) -> Option<&str> {
        self.pin.get(package).map(|s| s.as_str())
    }

    /// Check if a package is excluded
    pub fn is_excluded(&self, package: &str) -> bool {
        self.config.exclude.contains(&package.to_string())
    }

    /// Check if a package is explicitly installed (not just a dependency)
    pub fn is_explicit(&self, package: &str) -> bool {
        self.config.install.contains(&package.to_string())
    }

    /// Check if a package is optional
    pub fn is_optional(&self, package: &str) -> bool {
        self.optional.packages.contains(&package.to_string())
    }

    /// Get all packages that should be installed (explicit + optional)
    pub fn all_install_packages(&self) -> Vec<&str> {
        let mut packages: Vec<&str> = self.config.install.iter().map(|s| s.as_str()).collect();
        packages.extend(self.optional.packages.iter().map(|s| s.as_str()));
        packages
    }

    /// Validate the model for consistency
    pub fn validate(&self) -> ModelResult<()> {
        // Check version
        if self.config.version != MODEL_VERSION {
            return Err(ModelError::VersionMismatch {
                expected: MODEL_VERSION,
                found: self.config.version,
            });
        }

        // Check for conflicts between install and exclude
        for pkg in &self.config.install {
            if self.config.exclude.contains(pkg) {
                return Err(ModelError::ConflictingSpecs(format!(
                    "Package '{}' is both in install and exclude lists",
                    pkg
                )));
            }
        }

        // Validate pin patterns (basic check for now)
        for (pkg, pattern) in &self.pin {
            if pattern.is_empty() {
                return Err(ModelError::InvalidPinPattern(format!(
                    "Empty pin pattern for package '{}'",
                    pkg
                )));
            }
        }

        Ok(())
    }

    /// Serialize the model to TOML
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

impl Default for SystemModel {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a system model from a TOML file
pub fn parse_model_file(path: &Path) -> ModelResult<SystemModel> {
    let content = std::fs::read_to_string(path)?;
    parse_model_string(&content)
}

/// Parse a system model from a TOML string
pub fn parse_model_string(content: &str) -> ModelResult<SystemModel> {
    let model: SystemModel = toml::from_str(content)?;
    model.validate()?;
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_model() {
        let model = SystemModel::new();
        assert_eq!(model.config.version, MODEL_VERSION);
        assert!(model.config.install.is_empty());
    }

    #[test]
    fn test_parse_model_string() {
        let toml = r#"
[model]
version = 1
search = ["fedora@f41:stable"]
install = ["nginx", "redis"]
exclude = ["sendmail"]

[pin]
openssl = "3.0.*"
"#;
        let model = parse_model_string(toml).unwrap();
        assert_eq!(model.config.install.len(), 2);
        assert!(model.is_excluded("sendmail"));
        assert!(!model.is_excluded("nginx"));
        assert_eq!(model.get_pin("openssl"), Some("3.0.*"));
        assert_eq!(model.get_pin("nginx"), None);
    }

    #[test]
    fn test_conflict_detection() {
        let toml = r#"
[model]
version = 1
install = ["nginx"]
exclude = ["nginx"]
"#;
        let result = parse_model_string(toml);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ModelError::ConflictingSpecs(_)));
    }

    #[test]
    fn test_derived_package() {
        let toml = r#"
[model]
version = 1
install = ["nginx-custom"]

[[derive]]
name = "nginx-custom"
from = "nginx"
version = "inherit"
patches = ["custom.patch"]

[derive.override_files]
"/etc/nginx/nginx.conf" = "files/nginx.conf"
"#;
        let model = parse_model_string(toml).unwrap();
        assert_eq!(model.derive.len(), 1);
        assert_eq!(model.derive[0].name, "nginx-custom");
        assert_eq!(model.derive[0].from, "nginx");
        assert_eq!(model.derive[0].patches.len(), 1);
    }

    #[test]
    fn test_to_toml_roundtrip() {
        let mut model = SystemModel::new();
        model.config.search = vec!["fedora@f41:stable".to_string()];
        model.config.install = vec!["nginx".to_string(), "redis".to_string()];
        model.pin.insert("openssl".to_string(), "3.0.*".to_string());

        let toml = model.to_toml().unwrap();
        let parsed = parse_model_string(&toml).unwrap();

        assert_eq!(parsed.config.install, model.config.install);
        assert_eq!(parsed.pin, model.pin);
    }

    #[test]
    fn test_parse_include_section() {
        let toml = r#"
[model]
version = 1
install = ["custom-app"]

[include]
models = ["group-base-server@myrepo:stable", "group-security@corp:production"]
on_conflict = "local"
"#;
        let model = parse_model_string(toml).unwrap();
        assert_eq!(model.include.models.len(), 2);
        assert_eq!(model.include.models[0], "group-base-server@myrepo:stable");
        assert_eq!(model.include.on_conflict, ConflictStrategy::Local);
    }

    #[test]
    fn test_parse_include_error_strategy() {
        let toml = r#"
[model]
version = 1
install = ["custom-app"]

[include]
models = ["group-base@myrepo:stable"]
on_conflict = "error"
"#;
        let model = parse_model_string(toml).unwrap();
        assert_eq!(model.include.on_conflict, ConflictStrategy::Error);
    }

    #[test]
    fn test_has_includes() {
        let mut model = SystemModel::new();
        assert!(!model.has_includes());

        model.include.models.push("group-base@repo:stable".to_string());
        assert!(model.has_includes());
    }

    #[test]
    fn test_automation_defaults() {
        let model = SystemModel::new();
        // Default mode is Suggest (safest)
        assert_eq!(model.automation.mode, AutomationMode::Suggest);
        // AI assist is disabled by default
        assert!(!model.automation.ai_assist.enabled);
        // Major upgrades require approval by default
        assert!(model.automation.major_upgrades.require_approval);
    }

    #[test]
    fn test_parse_automation_config() {
        let toml = r#"
[model]
version = 1
install = ["nginx"]

[automation]
mode = "suggest"
check_interval = "1h"
notify = ["admin@example.com"]

[automation.security]
mode = "auto"
within = "12h"
severities = ["critical", "high", "medium"]
reboot = "never"

[automation.orphans]
mode = "suggest"
after = "14d"
keep = ["libfoo"]

[automation.updates]
mode = "disabled"
frequency = "daily"
window = "02:00-04:00"
exclude = ["kernel"]

[automation.major_upgrades]
require_approval = true
allow_auto = ["nodejs"]

[automation.repair]
integrity_check = true
check_interval = "12h"
auto_restore = true

[[automation.repair.rollback_triggers]]
name = "nginx-health"
command = "curl -f localhost/health"
timeout = "10s"
failure_window = "3m"
auto_rollback = true

[automation.ai_assist]
enabled = true
mode = "assisted"
intent_resolution = true
scriptlet_translation = false
natural_language = true
confidence_threshold = 0.85
require_human_approval = ["security", "removal"]
"#;
        let model = parse_model_string(toml).unwrap();

        // Global settings
        assert_eq!(model.automation.mode, AutomationMode::Suggest);
        assert_eq!(model.automation.check_interval, "1h");
        assert_eq!(model.automation.notify, vec!["admin@example.com"]);

        // Security
        assert_eq!(model.automation.security.mode, Some(AutomationMode::Auto));
        assert_eq!(model.automation.security.within, "12h");
        assert_eq!(model.automation.security.severities.len(), 3);
        assert_eq!(model.automation.security.reboot, "never");

        // Orphans
        assert_eq!(model.automation.orphans.mode, Some(AutomationMode::Suggest));
        assert_eq!(model.automation.orphans.after, "14d");
        assert_eq!(model.automation.orphans.keep, vec!["libfoo"]);

        // Updates
        assert_eq!(model.automation.updates.mode, Some(AutomationMode::Disabled));
        assert_eq!(model.automation.updates.frequency, "daily");
        assert_eq!(model.automation.updates.window, Some("02:00-04:00".to_string()));
        assert_eq!(model.automation.updates.exclude, vec!["kernel"]);

        // Major upgrades
        assert!(model.automation.major_upgrades.require_approval);
        assert_eq!(model.automation.major_upgrades.allow_auto, vec!["nodejs"]);

        // Repair
        assert!(model.automation.repair.integrity_check);
        assert!(model.automation.repair.auto_restore);
        assert_eq!(model.automation.repair.rollback_triggers.len(), 1);
        let trigger = &model.automation.repair.rollback_triggers[0];
        assert_eq!(trigger.name, "nginx-health");
        assert!(trigger.auto_rollback);

        // AI assist
        assert!(model.automation.ai_assist.enabled);
        assert_eq!(model.automation.ai_assist.mode, AiAssistMode::Assisted);
        assert!(model.automation.ai_assist.intent_resolution);
        assert!(!model.automation.ai_assist.scriptlet_translation);
        assert!(model.automation.ai_assist.natural_language);
        assert!((model.automation.ai_assist.confidence_threshold - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_effective_automation_mode() {
        let toml = r#"
[model]
version = 1
install = ["nginx"]

[automation]
mode = "suggest"

[automation.security]
mode = "auto"
"#;
        let model = parse_model_string(toml).unwrap();

        // Security has explicit override
        assert_eq!(
            model.effective_mode(AutomationCategory::Security),
            AutomationMode::Auto
        );
        // Orphans inherits global
        assert_eq!(
            model.effective_mode(AutomationCategory::Orphans),
            AutomationMode::Suggest
        );
        // Updates inherits global
        assert_eq!(
            model.effective_mode(AutomationCategory::Updates),
            AutomationMode::Suggest
        );
    }

    #[test]
    fn test_ai_assist_feature_checks() {
        let mut model = SystemModel::new();

        // AI assist disabled by default
        assert!(!model.ai_assist_enabled(AiFeature::IntentResolution));
        assert!(!model.ai_assist_enabled(AiFeature::NaturalLanguage));

        // Enable AI assist
        model.automation.ai_assist.enabled = true;
        model.automation.ai_assist.intent_resolution = true;

        // Now intent resolution is enabled
        assert!(model.ai_assist_enabled(AiFeature::IntentResolution));
        // But scriptlet translation is still disabled
        assert!(!model.ai_assist_enabled(AiFeature::ScriptletTranslation));
    }

    #[test]
    fn test_automation_mode_serialization() {
        let mut model = SystemModel::new();
        model.automation.mode = AutomationMode::Auto;
        model.automation.security.mode = Some(AutomationMode::Suggest);

        let toml = model.to_toml().unwrap();
        let parsed = parse_model_string(&toml).unwrap();

        assert_eq!(parsed.automation.mode, AutomationMode::Auto);
        assert_eq!(parsed.automation.security.mode, Some(AutomationMode::Suggest));
    }
}
