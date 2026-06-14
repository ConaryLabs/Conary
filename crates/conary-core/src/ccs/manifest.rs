// conary-core/src/ccs/manifest.rs
//! CCS Manifest (ccs.toml) parsing and data structures
//!
//! This module defines the structure of a CCS package manifest and provides
//! parsing from TOML format.

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
#[serde(deny_unknown_fields)]
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

    /// Post-install script hook (runs after files are deployed)
    #[serde(default)]
    pub post_install: Option<ScriptHook>,

    /// Pre-remove script hook (runs before files are removed)
    #[serde(default)]
    pub pre_remove: Option<ScriptHook>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookExecutionRoot {
    TryRoot,
    GenerationRoot,
    HostRoot,
}

impl Hooks {
    pub fn has_script_hooks(&self) -> bool {
        self.post_install.is_some() || self.pre_remove.is_some()
    }

    pub fn has_service_hooks(&self) -> bool {
        !self.services.is_empty()
    }

    pub fn has_declarative_hooks(&self) -> bool {
        !self.users.is_empty()
            || !self.groups.is_empty()
            || !self.directories.is_empty()
            || !self.systemd.is_empty()
            || !self.tmpfiles.is_empty()
            || !self.sysctl.is_empty()
            || !self.alternatives.is_empty()
    }

    pub fn has_irreversible_hooks_for_try_root(&self, execution_root: HookExecutionRoot) -> bool {
        if matches!(execution_root, HookExecutionRoot::HostRoot) {
            return self.has_script_hooks()
                || self.has_service_hooks()
                || self.has_declarative_hooks();
        }

        self.services
            .iter()
            .any(|hook| !hook.reversible.unwrap_or(false))
            || self
                .post_install
                .as_ref()
                .is_some_and(|hook| !hook.reversible.unwrap_or(false))
            || self
                .pre_remove
                .as_ref()
                .is_some_and(|hook| !hook.reversible.unwrap_or(false))
            || self
                .users
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .groups
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .directories
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .systemd
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .tmpfiles
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .sysctl
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .alternatives
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
    }
}

/// Scriptlet-scoped declarations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScriptletDeclarations {
    /// Narrow host-integration capabilities requested by scriptlets.
    #[serde(default)]
    pub capabilities: Vec<ScriptletCapabilityDeclaration>,
}

impl ScriptletDeclarations {
    /// Whether any scriptlet capability declarations are present.
    pub fn has_capability_declarations(&self) -> bool {
        !self.capabilities.is_empty()
    }

    fn validate(&self) -> Result<(), ManifestError> {
        for capability in &self.capabilities {
            capability.validate()?;
        }
        Ok(())
    }
}

/// A narrow host-integration capability requested by a package scriptlet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptletCapabilityDeclaration {
    pub name: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

impl ScriptletCapabilityDeclaration {
    fn validate(&self) -> Result<(), ManifestError> {
        let Some(allowed_paths) = supported_scriptlet_capability_paths(&self.name) else {
            return Err(ManifestError::Invalid(format!(
                "unknown scriptlet capability '{}'; declare a supported capability or run in a VM until enforcement exists",
                self.name
            )));
        };

        for path in &self.paths {
            if !path.starts_with('/') {
                return Err(ManifestError::Invalid(format!(
                    "relative path not allowed in scriptlets.capabilities '{}': {}",
                    self.name, path
                )));
            }
            if !allowed_paths.contains(&path.as_str()) {
                return Err(ManifestError::Invalid(format!(
                    "unsupported path '{}' for scriptlet capability '{}'; supported paths: {}",
                    path,
                    self.name,
                    allowed_paths.join(", ")
                )));
            }
        }

        Ok(())
    }
}

fn supported_scriptlet_capability_paths(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "systemd-service-registration" => Some(&["/etc/systemd/system"]),
        "tmpfiles-registration" => Some(&["/usr/lib/tmpfiles.d", "/etc/tmpfiles.d"]),
        "dbus-service-registration" => {
            Some(&["/usr/share/dbus-1/system-services", "/etc/dbus-1/system.d"])
        }
        _ => None,
    }
}

/// Script hook -- an arbitrary shell command run during install/remove
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptHook {
    pub script: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

pub type User = UserHook;
pub type Group = GroupHook;

/// Generic service management hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub action: ServiceAction,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

/// Group creation hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupHook {
    pub name: String,

    #[serde(default)]
    pub system: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

/// sysctl setting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysctlHook {
    pub key: String,
    pub value: String,

    #[serde(default)]
    pub only_if_lower: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

/// Alternatives system hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeHook {
    pub name: String,
    pub path: String,

    #[serde(default = "default_priority")]
    pub priority: i32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
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

    #[test]
    fn manifest_provenance_serializes_m1a_origin_and_hardening() {
        let provenance = ManifestProvenance {
            origin_class: Some("native-built".to_string()),
            hardening_level: Some("sandboxed".to_string()),
            ..Default::default()
        };
        let toml = toml::to_string(&provenance).unwrap();
        assert!(toml.contains("origin_class"));
        assert!(toml.contains("hardening_level"));
    }

    #[test]
    fn test_redirects_section_parsing() {
        let toml = r#"
[package]
name = "nginx"
version = "1.24.0"
description = "High-performance HTTP server"

[[redirects.renames]]
old_name = "nginx-mainline"
message = "Consolidated with mainline"

[[redirects.obsoletes]]
package = "nginx-legacy"
version = "<1.20"
message = "Legacy branch no longer supported"
"#;
        let manifest = CcsManifest::parse(toml).unwrap();
        assert_eq!(manifest.redirects.renames.len(), 1);
        assert_eq!(manifest.redirects.renames[0].old_name, "nginx-mainline");

        assert_eq!(manifest.redirects.obsoletes.len(), 1);
        assert_eq!(manifest.redirects.obsoletes[0].package, "nginx-legacy");
        assert_eq!(
            manifest.redirects.obsoletes[0].version,
            Some("<1.20".to_string())
        );
    }

    #[test]
    fn test_redirects_merge_split() {
        let toml = r#"
[package]
name = "foo-combined"
version = "2.0.0"
description = "Combined package"

[[redirects.merges]]
package = "foo-core"
message = "Merged foo-core into main package"

[[redirects.merges]]
package = "foo-extras"
message = "Merged foo-extras into main package"

[[redirects.splits]]
from_package = "monolithic-foo"
component = "core"
"#;
        let manifest = CcsManifest::parse(toml).unwrap();
        assert_eq!(manifest.redirects.merges.len(), 2);
        assert_eq!(manifest.redirects.splits.len(), 1);
        assert_eq!(manifest.redirects.splits[0].from_package, "monolithic-foo");
        assert_eq!(
            manifest.redirects.splits[0].component,
            Some("core".to_string())
        );
    }

    #[test]
    fn test_redirects_is_empty() {
        let redirects = Redirects::default();
        assert!(redirects.is_empty());
        assert_eq!(redirects.len(), 0);

        let toml = r#"
[package]
name = "simple"
version = "1.0.0"
description = "No redirects"
"#;
        let manifest = CcsManifest::parse(toml).unwrap();
        assert!(manifest.redirects.is_empty());
    }

    #[test]
    fn test_manifest_rejects_non_system_user_hooks() {
        let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[hooks.users]]
name = "daemon"
system = false
"#;

        let err = CcsManifest::parse(toml).unwrap_err();
        assert!(err.to_string().contains("system user"));
    }

    #[test]
    fn test_manifest_rejects_unsafe_tmpfiles_entries() {
        let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[hooks.tmpfiles]]
entry_type = "L"
path = "../etc/shadow"
mode = "0755"
owner = "root"
group = "root"
"#;

        let err = CcsManifest::parse(toml).unwrap_err();
        assert!(
            err.to_string().contains("tmpfiles")
                || err.to_string().contains("path")
                || err.to_string().contains("entry")
        );
    }

    #[test]
    fn test_manifest_rejects_denied_sysctl_keys() {
        let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[hooks.sysctl]]
key = "kernel.modules_disabled"
value = "0"
"#;

        let err = CcsManifest::parse(toml).unwrap_err();
        assert!(err.to_string().contains("sysctl"));
    }

    #[test]
    fn test_manifest_rejects_unsafe_systemd_unit_names() {
        let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[hooks.systemd]]
unit = "../evil.service"
enable = true
"#;

        let err = CcsManifest::parse(toml).unwrap_err();
        assert!(err.to_string().contains("systemd"));
    }

    #[test]
    fn test_manifest_accepts_supported_scriptlet_capabilities() {
        let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[scriptlets.capabilities]]
name = "systemd-service-registration"
paths = ["/etc/systemd/system"]

[[scriptlets.capabilities]]
name = "tmpfiles-registration"
paths = ["/usr/lib/tmpfiles.d", "/etc/tmpfiles.d"]
"#;

        let manifest = CcsManifest::parse(toml).unwrap();
        assert_eq!(manifest.scriptlets.capabilities.len(), 2);
        assert!(manifest.scriptlets.has_capability_declarations());
    }

    #[test]
    fn test_manifest_rejects_unknown_scriptlet_capability() {
        let toml = r#"
[package]
name = "test"
version = "1.0.0"
description = "test"

[[scriptlets.capabilities]]
name = "pam-live-edit"
paths = ["/etc/pam.d"]
"#;

        let err = CcsManifest::parse(toml).unwrap_err();
        assert!(
            err.to_string().contains(
                "unknown scriptlet capability 'pam-live-edit'; declare a supported capability or run in a VM until enforcement exists"
            ),
            "unexpected error: {err}"
        );
    }

    fn manifest_with_legacy_scriptlet_bundle(body: &str, body_sha256: &str) -> String {
        format!(
            r#"
[package]
name = "nginx"
version = "1.28.0"
description = "nginx converted from RPM"

[legacy_scriptlets]
schema = "conary.legacy-scriptlets.v1"
schema_revision = 1
source_format = "rpm"
source_family = "fedora-rhel"
source_distro = "fedora"
source_release = "44"
source_arch = "x86_64"
source_package = "nginx"
source_version = "1.28.0-1.fc44"
source_checksum = "sha256:3333333333333333333333333333333333333333333333333333333333333333"
version_scheme = "rpm"
conversion_tool = "remi"
conversion_tool_version = "0.8.0"
conversion_policy = "safe-or-legacy"
target_compatibility = "source-native"
allowed_targets = ["rpm/fedora/44/x86_64"]
foreign_replay_policy = "deny"
publication_policy = "public-if-no-blocked"
publication_status = "private-review"
scriptlet_fidelity = "legacy-replay"

[legacy_scriptlets.decision_counts]
legacy = 1

[[legacy_scriptlets.entries]]
id = "rpm:%post"
native_slot = "%post"
phase = "post-install"
lifecycle_paths = ["install:first"]
interpreter = "/bin/sh"
interpreter_args = ["-e"]
body_sha256 = "{body_sha256}"
body = "{body}"
native_invocation = {{ args = ["1"], environment = ["RPM_INSTALL_PREFIX=/"], stdin = "none", chroot = "install-root" }}
transaction_order = {{ position = "after-payload", after = ["payload"] }}
timeout_ms = 30000
decision = "legacy"
reason_code = "protected-replay-required"

[[legacy_scriptlets.entries.effects]]
kind = "ldconfig"
source = "static-signal"
confidence = "declared"
replacement = "complete"
"#
        )
    }

    #[test]
    fn manifest_toml_round_trips_legacy_scriptlet_bundle() {
        let body = "ldconfig";
        let body_sha256 = crate::hash::sha256_prefixed(body.as_bytes());
        let toml = manifest_with_legacy_scriptlet_bundle(body, &body_sha256);

        let manifest = CcsManifest::parse(&toml).expect("parse manifest");
        let bundle = manifest
            .legacy_scriptlets
            .as_ref()
            .expect("legacy scriptlet bundle");

        assert_eq!(bundle.source_package, "nginx");
        assert_eq!(bundle.entries.len(), 1);
        assert_eq!(bundle.entries[0].id, "rpm:%post");

        let encoded = manifest.to_toml().expect("serialize manifest");
        assert!(encoded.contains("[legacy_scriptlets]"));
        let decoded = CcsManifest::parse(&encoded).expect("parse serialized manifest");
        assert_eq!(
            decoded
                .legacy_scriptlets
                .as_ref()
                .expect("legacy bundle")
                .entries[0]
                .effects[0]
                .kind,
            "ldconfig"
        );
    }

    #[test]
    fn manifest_validation_rejects_invalid_legacy_scriptlet_bundle() {
        let body_sha256 = crate::hash::sha256_prefixed(b"ldconfig");
        let toml = manifest_with_legacy_scriptlet_bundle("ldconfig && echo tampered", &body_sha256);

        let err = CcsManifest::parse(&toml).expect_err("tampered bundle must fail");

        assert!(err.to_string().contains("legacy scriptlet bundle"));
        assert!(err.to_string().contains("body_sha256 mismatch"));
    }

    #[test]
    fn manifest_rejects_unknown_hook_keys() {
        let toml = r#"
[package]
name = "future-hook"
version = "1.0.0"
description = "future hook"

[[hooks.some_new_hook]]
name = "must-not-be-dropped"
"#;

        let err = CcsManifest::parse(toml).expect_err("unknown hook must be rejected");
        assert!(
            err.to_string().contains("some_new_hook") || err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn hooks_classify_script_service_and_declarative_entries() {
        let mut hooks = Hooks::default();
        assert!(!hooks.has_script_hooks());
        assert!(!hooks.has_service_hooks());
        assert!(!hooks.has_declarative_hooks());
        assert!(!hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::HostRoot));

        hooks.directories.push(DirectoryHook {
            path: "/var/lib/conary-test".to_string(),
            mode: "0755".to_string(),
            owner: "root".to_string(),
            group: "root".to_string(),
            cleanup: None,
            reversible: None,
        });
        assert!(hooks.has_declarative_hooks());
        assert!(!hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::TryRoot));
        assert!(!hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::GenerationRoot));
        assert!(hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::HostRoot));

        hooks.services.push(Service {
            name: "conary-test.service".to_string(),
            action: ServiceAction::Restart,
            reversible: None,
        });
        assert!(hooks.has_service_hooks());
        assert!(hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::TryRoot));

        hooks.post_install = Some(ScriptHook {
            script: "echo post-install".to_string(),
            reversible: None,
        });
        assert!(hooks.has_script_hooks());
        assert!(hooks.has_irreversible_hooks_for_try_root(HookExecutionRoot::GenerationRoot));
    }

    #[test]
    fn omitted_reversible_fields_keep_wire_compatibility_and_m1b_defaults() {
        let toml = r#"
[package]
name = "hook-defaults"
version = "1.0.0"
description = "hook defaults"

[[hooks.users]]
name = "hookuser"
system = true

[[hooks.services]]
name = "hook-defaults.service"
action = "restart"

[hooks.post_install]
script = "echo post-install"
"#;

        let manifest = CcsManifest::parse(toml).expect("parse manifest without reversible fields");

        assert_eq!(manifest.hooks.users[0].reversible, None);
        assert_eq!(manifest.hooks.services[0].reversible, None);
        assert_eq!(
            manifest
                .hooks
                .post_install
                .as_ref()
                .expect("post-install hook")
                .reversible,
            None
        );
        assert!(
            manifest
                .hooks
                .has_irreversible_hooks_for_try_root(HookExecutionRoot::TryRoot)
        );

        let encoded = manifest.to_toml().expect("serialize manifest");
        assert!(!encoded.contains("reversible"));

        let declarative_only = CcsManifest::parse(
            r#"
[package]
name = "declarative-defaults"
version = "1.0.0"
description = "declarative defaults"

[[hooks.groups]]
name = "hookgroup"
system = true
"#,
        )
        .expect("parse declarative manifest");

        assert!(
            !declarative_only
                .hooks
                .has_irreversible_hooks_for_try_root(HookExecutionRoot::TryRoot)
        );
        assert!(
            !declarative_only
                .hooks
                .has_irreversible_hooks_for_try_root(HookExecutionRoot::GenerationRoot)
        );
        assert!(
            declarative_only
                .hooks
                .has_irreversible_hooks_for_try_root(HookExecutionRoot::HostRoot)
        );
    }
}
