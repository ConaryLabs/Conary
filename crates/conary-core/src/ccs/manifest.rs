// conary-core/src/ccs/manifest.rs
//! CCS Manifest (ccs.toml) parsing and data structures
//!
//! This module defines the structure of a CCS package manifest and provides
//! parsing from TOML format.

mod hooks;

pub use hooks::*;

use crate::capability::CapabilityDeclaration;
use crate::ccs::hooks::{
    is_denied_sysctl_key, is_safe_unit_name, validate_shell, validate_tmpfiles_entry_type,
    validate_username,
};
use crate::ccs::legacy_scriptlets::LegacyScriptletBundle;
pub use crate::ccs::manifest_provenance::{
    ManifestProvenance, ProvenanceDep, ProvenancePatch, ProvenanceSignature,
};
use crate::ccs::policy::BuildPolicyConfig;
use crate::ccs::v2::PackageKindTagV2;
use crate::filesystem::path::sanitize_path;
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

    /// Scriptlet execution declarations and host-integration capabilities
    #[serde(default)]
    pub scriptlets: ScriptletDeclarations,

    /// Passive legacy scriptlet semantics bundle for converted packages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_scriptlets: Option<LegacyScriptletBundle>,

    #[serde(default)]
    pub config: Config,

    #[serde(default)]
    pub build: Option<BuildInfo>,

    #[serde(default)]
    pub legacy: Option<Legacy>,

    /// Build policy configuration
    #[serde(default)]
    pub policy: BuildPolicyConfig,

    /// Linux file capabilities to apply to shipped payload files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_capabilities: Vec<FileCapability>,

    /// Full provenance / Package DNA information
    #[serde(default)]
    pub provenance: Option<ManifestProvenance>,

    /// Capability declarations for sandboxing/enforcement
    #[serde(default)]
    pub capabilities: Option<CapabilityDeclaration>,

    /// Redirect declarations for package evolution
    ///
    /// Allows packages to declare that they rename, obsolete, or supersede
    /// other packages. This enables clean package evolution over time.
    #[serde(default)]
    pub redirects: Redirects,
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

        for user in &self.hooks.users {
            validate_username(&user.name).map_err(|error| {
                ManifestError::Invalid(format!(
                    "invalid hooks.users name '{}': {}",
                    user.name, error
                ))
            })?;
            if !user.system {
                return Err(ManifestError::Invalid(format!(
                    "hooks.users '{}' must be a system user",
                    user.name
                )));
            }
            if let Some(group) = &user.group {
                validate_username(group).map_err(|error| {
                    ManifestError::Invalid(format!(
                        "invalid hooks.users group '{}': {}",
                        group, error
                    ))
                })?;
            }
            if let Some(shell) = &user.shell {
                validate_shell(shell).map_err(|error| {
                    ManifestError::Invalid(format!(
                        "invalid hooks.users shell '{}': {}",
                        shell, error
                    ))
                })?;
            }
            if let Some(home) = &user.home {
                sanitize_path(home).map_err(|error| {
                    ManifestError::Invalid(format!(
                        "invalid hooks.users home '{}': {}",
                        home, error
                    ))
                })?;
            }
        }

        for group in &self.hooks.groups {
            validate_username(&group.name).map_err(|error| {
                ManifestError::Invalid(format!(
                    "invalid hooks.groups name '{}': {}",
                    group.name, error
                ))
            })?;
            if !group.system {
                return Err(ManifestError::Invalid(format!(
                    "hooks.groups '{}' must be a system group",
                    group.name
                )));
            }
        }

        for dir in &self.hooks.directories {
            sanitize_path(&dir.path).map_err(|error| {
                ManifestError::Invalid(format!(
                    "invalid hooks.directories path '{}': {}",
                    dir.path, error
                ))
            })?;
        }

        for entry in &self.hooks.tmpfiles {
            validate_tmpfiles_entry_type(&entry.entry_type).map_err(|error| {
                ManifestError::Invalid(format!(
                    "invalid hooks.tmpfiles entry type '{}': {}",
                    entry.entry_type, error
                ))
            })?;
            sanitize_path(&entry.path).map_err(|error| {
                ManifestError::Invalid(format!(
                    "invalid hooks.tmpfiles path '{}': {}",
                    entry.path, error
                ))
            })?;
        }

        for entry in &self.hooks.sysctl {
            if is_denied_sysctl_key(&entry.key) {
                return Err(ManifestError::Invalid(format!(
                    "hooks.sysctl key '{}' is denied for security reasons",
                    entry.key
                )));
            }
        }

        for unit in &self.hooks.systemd {
            if !is_safe_unit_name(&unit.unit) {
                return Err(ManifestError::Invalid(format!(
                    "hooks.systemd unit '{}' is unsafe",
                    unit.unit
                )));
            }
        }

        self.scriptlets.validate()?;
        for capability in &self.file_capabilities {
            capability.validate()?;
        }
        if let Some(bundle) = &self.legacy_scriptlets {
            bundle.validate().map_err(|error| {
                ManifestError::Invalid(format!(
                    "legacy scriptlet bundle validation failed: {error}"
                ))
            })?;
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
                release: None,
                kind: None,
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
            scriptlets: ScriptletDeclarations::default(),
            legacy_scriptlets: None,
            config: Config::default(),
            build: None,
            legacy: None,
            policy: BuildPolicyConfig::default(),
            file_capabilities: Vec::new(),
            provenance: None,
            capabilities: None,
            redirects: Redirects::default(),
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
    pub release: Option<String>,

    #[serde(default)]
    pub kind: Option<PackageKindTagV2>,

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

/// Package redirects / supersedes declarations
///
/// Allows packages to declare relationships to other packages they
/// rename, obsolete, or supersede. Used for clean package evolution.
///
/// # Example
/// ```toml
/// [[redirects.obsoletes]]
/// package = "old-nginx"
/// message = "Replaced by nginx, which provides the same functionality"
///
/// [[redirects.renames]]
/// old_name = "libfoo"
/// version = "<2.0"  # Only for versions before 2.0
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Redirects {
    /// Packages this package renames (old names that now point to this)
    #[serde(default)]
    pub renames: Vec<RedirectRename>,

    /// Packages this package obsoletes (deprecated packages this replaces)
    #[serde(default)]
    pub obsoletes: Vec<RedirectObsolete>,

    /// Packages that have been merged into this one
    #[serde(default)]
    pub merges: Vec<RedirectMerge>,

    /// Packages this was split from (for split subpackages)
    #[serde(default)]
    pub splits: Vec<RedirectSplit>,
}

impl Redirects {
    /// Check if any redirects are declared
    pub fn is_empty(&self) -> bool {
        self.renames.is_empty()
            && self.obsoletes.is_empty()
            && self.merges.is_empty()
            && self.splits.is_empty()
    }

    /// Get total number of redirects
    pub fn len(&self) -> usize {
        self.renames.len() + self.obsoletes.len() + self.merges.len() + self.splits.len()
    }
}

/// A package rename redirect (old-name -> this package)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectRename {
    /// The old package name that should redirect to this package
    pub old_name: String,

    /// Optional version constraint for when this rename applies
    /// e.g., "<2.0" means only versions before 2.0 are renamed
    #[serde(default)]
    pub version: Option<String>,

    /// Optional message explaining the rename
    #[serde(default)]
    pub message: Option<String>,
}

/// A package obsolete redirect (deprecated package -> this package)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectObsolete {
    /// The deprecated package name that this package replaces
    pub package: String,

    /// Optional version constraint
    #[serde(default)]
    pub version: Option<String>,

    /// Explanation of why the package is obsoleted
    #[serde(default)]
    pub message: Option<String>,
}

/// A merge redirect (multiple packages merged into this one)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectMerge {
    /// Package that was merged into this one
    pub package: String,

    /// Optional version constraint
    #[serde(default)]
    pub version: Option<String>,

    /// Explanation of the merge
    #[serde(default)]
    pub message: Option<String>,
}

/// A split redirect (this package was split from another)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectSplit {
    /// The original monolithic package this was split from
    pub from_package: String,

    /// Which component of the original this represents
    /// e.g., "devel", "libs", "docs"
    #[serde(default)]
    pub component: Option<String>,

    /// Explanation of the split
    #[serde(default)]
    pub message: Option<String>,
}

/// Parse an octal mode string (e.g., "0755", "0o755", or "755") to a `u32`.
///
/// Returns an error if the mode string is not a valid octal number, rather
/// than silently falling back to a default that could mask typos.
pub fn parse_octal_mode(mode: &str) -> crate::Result<u32> {
    if mode.is_empty() {
        return Err(crate::Error::ParseError(
            "invalid octal mode: empty string".to_string(),
        ));
    }
    let mode_str = mode
        .strip_prefix("0o")
        .or_else(|| {
            // Only strip leading '0' if there are more characters after it,
            // so that bare "0" is parsed as octal 0 (not empty string).
            if mode.len() > 1 {
                mode.strip_prefix('0')
            } else {
                None
            }
        })
        .unwrap_or(mode);
    u32::from_str_radix(mode_str, 8)
        .map_err(|_| crate::Error::ParseError(format!("invalid octal mode: '{mode}'")))
}

#[cfg(test)]
mod tests;
