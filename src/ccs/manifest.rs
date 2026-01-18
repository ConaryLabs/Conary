// src/ccs/manifest.rs
//! CCS Manifest (ccs.toml) parsing and data structures
//!
//! This module defines the structure of a CCS package manifest and provides
//! parsing from TOML format.

use crate::ccs::policy::BuildPolicyConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ManifestError {
    #[error("Failed to read manifest file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Failed to parse manifest: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid manifest: {0}")]
    Invalid(String),
}

/// Root structure of ccs.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CcsManifest {
    pub package: Package,

    #[serde(default)]
    pub provides: Provides,

    #[serde(default)]
    pub requires: Requires,

    #[serde(default)]
    pub suggests: Suggests,

    #[serde(default)]
    pub components: Components,

    #[serde(default)]
    pub hooks: Hooks,

    #[serde(default)]
    pub config: Config,

    #[serde(default)]
    pub build: Option<BuildInfo>,

    #[serde(default)]
    pub legacy: Option<Legacy>,

    /// Build policy configuration
    #[serde(default)]
    pub policy: BuildPolicyConfig,
}

impl CcsManifest {
    /// Load manifest from a file path
    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Parse manifest from a TOML string
    pub fn parse(content: &str) -> Result<Self, ManifestError> {
        let manifest: CcsManifest = toml::from_str(content)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate the manifest for required fields and consistency
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.package.name.is_empty() {
            return Err(ManifestError::MissingField("package.name".to_string()));
        }
        if self.package.version.is_empty() {
            return Err(ManifestError::MissingField("package.version".to_string()));
        }
        Ok(())
    }

    /// Generate a minimal manifest for a new project
    pub fn new_minimal(name: &str, version: &str) -> Self {
        CcsManifest {
            package: Package {
                name: name.to_string(),
                version: version.to_string(),
                description: format!("A new CCS package: {}", name),
                license: None,
                homepage: None,
                repository: None,
                platform: None,
                authors: None,
            },
            provides: Provides::default(),
            requires: Requires::default(),
            suggests: Suggests::default(),
            components: Components::default(),
            hooks: Hooks::default(),
            config: Config::default(),
            build: None,
            legacy: None,
            policy: BuildPolicyConfig::default(),
        }
    }

    /// Serialize to TOML string
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

/// Package metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub description: String,

    #[serde(default)]
    pub license: Option<String>,

    #[serde(default)]
    pub homepage: Option<String>,

    #[serde(default)]
    pub repository: Option<String>,

    #[serde(default)]
    pub platform: Option<Platform>,

    #[serde(default)]
    pub authors: Option<Authors>,
}

/// Platform targeting
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Platform {
    #[serde(default = "default_os")]
    pub os: String,

    #[serde(default)]
    pub arch: Option<String>,

    #[serde(default = "default_libc")]
    pub libc: String,

    #[serde(default)]
    pub abi: Option<String>,
}

fn default_os() -> String {
    "linux".to_string()
}

fn default_libc() -> String {
    "gnu".to_string()
}

/// Package authors/maintainers
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Authors {
    #[serde(default)]
    pub maintainers: Vec<String>,

    #[serde(default)]
    pub upstream: Option<String>,
}

/// What this package provides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Provides {
    #[serde(default)]
    pub capabilities: Vec<String>,

    /// Auto-detected shared library sonames
    #[serde(default)]
    pub sonames: Vec<String>,

    /// Auto-detected executable paths
    #[serde(default)]
    pub binaries: Vec<String>,

    /// Auto-detected pkg-config files
    #[serde(default)]
    pub pkgconfig: Vec<String>,
}

/// What this package requires
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Requires {
    #[serde(default)]
    pub capabilities: Vec<Capability>,

    /// Fallback package dependencies (name-based)
    #[serde(default)]
    pub packages: Vec<PackageDep>,
}

/// A capability requirement with optional version constraint
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Capability {
    Simple(String),
    Versioned { name: String, version: String },
}

impl Capability {
    pub fn name(&self) -> &str {
        match self {
            Capability::Simple(s) => s,
            Capability::Versioned { name, .. } => name,
        }
    }

    pub fn version(&self) -> Option<&str> {
        match self {
            Capability::Simple(_) => None,
            Capability::Versioned { version, .. } => Some(version),
        }
    }
}

/// A package dependency with version constraint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDep {
    pub name: String,

    #[serde(default)]
    pub version: Option<String>,
}

/// Optional/suggested dependencies
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Suggests {
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Component configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Components {
    /// Glob pattern overrides for component assignment
    #[serde(default)]
    pub overrides: Vec<ComponentOverride>,

    /// Exact file path overrides
    #[serde(default)]
    pub files: HashMap<String, String>,

    /// Which components install by default
    #[serde(default = "default_components")]
    pub default: Vec<String>,
}

fn default_components() -> Vec<String> {
    vec![
        "runtime".to_string(),
        "lib".to_string(),
        "config".to_string(),
    ]
}

/// A component override rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentOverride {
    pub path: String,
    pub component: String,
}

/// Declarative hooks
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Hooks {
    #[serde(default)]
    pub users: Vec<UserHook>,

    #[serde(default)]
    pub groups: Vec<GroupHook>,

    #[serde(default)]
    pub directories: Vec<DirectoryHook>,

    #[serde(default)]
    pub services: Vec<Service>,

    #[serde(default)]
    pub systemd: Vec<SystemdHook>,

    #[serde(default)]
    pub tmpfiles: Vec<TmpfilesHook>,

    #[serde(default)]
    pub sysctl: Vec<SysctlHook>,

    #[serde(default)]
    pub alternatives: Vec<AlternativeHook>,
}

pub type User = UserHook;
pub type Group = GroupHook;

/// Generic service management hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub action: ServiceAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceAction {
    Enable,
    Disable,
    Start,
    Stop,
    Restart,
}

/// User creation hook (sysusers-style)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserHook {
    pub name: String,

    #[serde(default)]
    pub system: bool,

    #[serde(default)]
    pub home: Option<String>,

    #[serde(default)]
    pub shell: Option<String>,

    #[serde(default)]
    pub group: Option<String>,
}

/// Group creation hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupHook {
    pub name: String,

    #[serde(default)]
    pub system: bool,
}

/// Directory creation hook (tmpfiles-style)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryHook {
    pub path: String,

    #[serde(default = "default_mode")]
    pub mode: String,

    #[serde(default = "default_owner")]
    pub owner: String,

    #[serde(default = "default_group")]
    pub group: String,

    #[serde(default)]
    pub cleanup: Option<String>,
}

fn default_mode() -> String {
    "0755".to_string()
}

fn default_owner() -> String {
    "root".to_string()
}

fn default_group() -> String {
    "root".to_string()
}

/// Systemd unit hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemdHook {
    pub unit: String,

    #[serde(default)]
    pub enable: bool,
}

/// tmpfiles.d entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmpfilesHook {
    #[serde(rename = "type")]
    pub entry_type: String,

    pub path: String,

    #[serde(default = "default_mode")]
    pub mode: String,

    #[serde(default = "default_owner")]
    pub owner: String,

    #[serde(default = "default_group")]
    pub group: String,
}

/// sysctl setting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysctlHook {
    pub key: String,
    pub value: String,

    #[serde(default)]
    pub only_if_lower: bool,
}

/// Alternatives system hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeHook {
    pub name: String,
    pub path: String,

    #[serde(default = "default_priority")]
    pub priority: i32,
}

fn default_priority() -> i32 {
    50
}

/// Configuration file tracking
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub files: Vec<String>,

    #[serde(default = "default_true")]
    pub noreplace: bool,
}

fn default_true() -> bool {
    true
}

/// Build provenance information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildInfo {
    #[serde(default)]
    pub source: Option<String>,

    #[serde(default)]
    pub commit: Option<String>,

    #[serde(default)]
    pub timestamp: Option<String>,

    #[serde(default)]
    pub environment: HashMap<String, String>,

    #[serde(default)]
    pub commands: Vec<String>,

    #[serde(default)]
    pub reproducible: bool,
}

/// Legacy format generation settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Legacy {
    #[serde(default)]
    pub rpm: Option<RpmLegacy>,

    #[serde(default)]
    pub deb: Option<DebLegacy>,

    #[serde(default)]
    pub arch: Option<ArchLegacy>,
}

/// RPM-specific overrides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RpmLegacy {
    #[serde(default)]
    pub group: Option<String>,

    #[serde(default)]
    pub requires: Vec<String>,

    #[serde(default)]
    pub provides: Vec<String>,
}

/// DEB-specific overrides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DebLegacy {
    #[serde(default)]
    pub section: Option<String>,

    #[serde(default)]
    pub priority: Option<String>,

    #[serde(default)]
    pub depends: Vec<String>,
}

/// Arch-specific overrides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArchLegacy {
    #[serde(default)]
    pub groups: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_manifest() {
        let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "A test package"
"#;
        let manifest = CcsManifest::parse(toml).unwrap();
        assert_eq!(manifest.package.name, "test");
        assert_eq!(manifest.package.version, "1.0.0");
    }

    #[test]
    fn test_full_manifest() {
        let toml = r#"
[package]
name = "myapp"
version = "1.2.3"
description = "My application"
license = "MIT"

[package.platform]
os = "linux"
arch = "x86_64"
libc = "gnu"

[provides]
capabilities = ["cli-tool", "json-parsing"]

[requires]
capabilities = [
    "glibc",
    { name = "tls", version = ">=1.2" },
]
packages = [
    { name = "openssl", version = ">=3.0" },
]

[components]
default = ["runtime", "lib"]

[components.files]
"/usr/bin/helper" = "lib"

[[hooks.users]]
name = "myapp"
system = true
home = "/var/lib/myapp"

[[hooks.directories]]
path = "/var/lib/myapp"
mode = "0750"
owner = "myapp"

[[hooks.systemd]]
unit = "myapp.service"
enable = false

[config]
files = ["/etc/myapp/config.toml"]
"#;
        let manifest = CcsManifest::parse(toml).unwrap();
        assert_eq!(manifest.package.name, "myapp");
        assert_eq!(manifest.provides.capabilities.len(), 2);
        assert_eq!(manifest.requires.capabilities.len(), 2);
        assert_eq!(manifest.hooks.users.len(), 1);
        assert_eq!(manifest.hooks.users[0].name, "myapp");
        assert!(manifest.hooks.users[0].system);
    }

    #[test]
    fn test_generate_minimal() {
        let manifest = CcsManifest::new_minimal("test", "0.1.0");
        let toml = manifest.to_toml().unwrap();
        assert!(toml.contains("name = \"test\""));
        assert!(toml.contains("version = \"0.1.0\""));
    }
}
