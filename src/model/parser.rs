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
}
