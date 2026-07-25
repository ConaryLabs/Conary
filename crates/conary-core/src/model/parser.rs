// conary-core/src/model/parser.rs

//! Parser for system model TOML files.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use super::{ModelError, ModelResult};

mod automation;
mod federation;
mod source_policy;

pub use automation::{
    AiAssistConfig, AiAssistMode, AiFeature, AutomationCategory, AutomationConfig, AutomationMode,
    MajorUpgradeAutomation, OrphanAutomation, RepairAutomation, RollbackTrigger,
    SecurityAutomation, UpdateAutomation,
};
pub use federation::{FederationConfig, FederationTier};
pub use source_policy::{ConvergenceIntent, PackageOverrideConfig, SourcePinConfig, SystemConfig};
use source_policy::{selection_mode_from_profile, selection_mode_from_string};

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

    /// System-level source selection policy
    #[serde(default)]
    pub system: SystemConfig,

    /// Per-package distro overrides
    #[serde(default)]
    pub overrides: HashMap<String, PackageOverrideConfig>,
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
#[serde(deny_unknown_fields)]
pub struct IncludeConfig {
    /// Remote models to include (e.g., "group-base@repo:branch")
    #[serde(default)]
    pub models: Vec<String>,

    /// Conflict resolution strategy when local and remote define same package
    #[serde(default)]
    pub on_conflict: ConflictStrategy,

    /// Trusted Ed25519 public keys (32 bytes, hex-encoded)
    #[serde(default)]
    pub trusted_keys: Vec<String>,
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
            system: SystemConfig::default(),
            overrides: HashMap::new(),
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

    /// Check if a deferred assistant feature flag is enabled.
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
        self.config.exclude.iter().any(|p| p == package)
    }

    /// Check if a package is explicitly installed (not just a dependency)
    pub fn is_explicit(&self, package: &str) -> bool {
        self.config.install.iter().any(|p| p == package)
    }

    /// Check if a package is optional
    pub fn is_optional(&self, package: &str) -> bool {
        self.optional.packages.iter().any(|p| p == package)
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

        let mut has_remote_include = false;
        for include in &self.include.models {
            let (_, label) = super::parse_trove_spec(include)?;
            has_remote_include |= label.is_some();
        }
        if has_remote_include && self.include.trusted_keys.is_empty() {
            return Err(ModelError::InvalidSearchPath(
                "Remote model includes require at least one trusted Ed25519 public key".to_string(),
            ));
        }
        let mut unique_trusted_keys = std::collections::HashSet::new();
        for key_hex in &self.include.trusted_keys {
            let key_bytes = hex::decode(key_hex).map_err(|error| {
                ModelError::InvalidSearchPath(format!(
                    "Invalid trusted Ed25519 public key '{key_hex}': {error}"
                ))
            })?;
            let key: [u8; 32] = key_bytes.try_into().map_err(|bytes: Vec<u8>| {
                ModelError::InvalidSearchPath(format!(
                    "Invalid trusted Ed25519 public key length: expected 32 bytes, found {}",
                    bytes.len()
                ))
            })?;
            ed25519_dalek::VerifyingKey::from_bytes(&key).map_err(|error| {
                ModelError::InvalidSearchPath(format!(
                    "Invalid trusted Ed25519 public key '{key_hex}': {error}"
                ))
            })?;
            if !unique_trusted_keys.insert(key) {
                return Err(ModelError::InvalidSearchPath(format!(
                    "Duplicate trusted Ed25519 public key '{key_hex}'"
                )));
            }
        }

        if let Some(profile) = self.system.profile.as_deref()
            && selection_mode_from_profile(profile).is_none()
        {
            return Err(ModelError::InvalidSourcePolicy(format!(
                "Unknown source profile '{}'",
                profile
            )));
        }

        if let Some(selection_mode) = self.system.selection_mode.as_deref()
            && selection_mode_from_string(selection_mode).is_none()
        {
            return Err(ModelError::InvalidSourcePolicy(format!(
                "Unknown selection mode '{}'",
                selection_mode
            )));
        }

        Ok(())
    }

    /// Resolve which override (if any) applies to a given package name.
    ///
    /// Checks override scope in priority order: exact > family > class.
    /// - "exact" (or None): override key must match `package_name` exactly
    /// - "family": override key matches `canonical_family` (if provided)
    /// - "class": override key matches `package_class` (if provided)
    ///
    /// Returns the override key and config for the first match at the
    /// highest-priority scope level.
    pub fn resolve_override(
        &self,
        package_name: &str,
        canonical_family: Option<&str>,
        package_class: Option<&str>,
    ) -> Option<(&str, &PackageOverrideConfig)> {
        // Priority 1: exact match (scope is None or "exact")
        for (key, config) in &self.overrides {
            let scope = config.scope.as_deref().unwrap_or("exact");
            if scope == "exact" && key == package_name {
                return Some((key.as_str(), config));
            }
        }

        // Priority 2: family match
        if let Some(family) = canonical_family {
            for (key, config) in &self.overrides {
                if config.scope.as_deref() == Some("family") && key == family {
                    return Some((key.as_str(), config));
                }
            }
        }

        // Priority 3: class match
        if let Some(class) = package_class {
            for (key, config) in &self.overrides {
                if config.scope.as_deref() == Some("class") && key == class {
                    return Some((key.as_str(), config));
                }
            }
        }

        None
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
mod tests;
