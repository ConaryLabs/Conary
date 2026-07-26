// conary-core/src/ccs/manifest.rs
//! CCS Manifest (ccs.toml) parsing and data structures
//!
//! This module defines the structure of a CCS package manifest and provides
//! parsing from TOML format.

mod hooks;

pub use hooks::*;

use crate::capability::CapabilityDeclaration;
use crate::ccs::hooks::{
    is_denied_sysctl_key, is_safe_declarative_unit_name, validate_shell, validate_tmpfiles_fields,
    validate_username,
};
pub use crate::ccs::manifest_provenance::{
    ManifestProvenance, ProvenanceDep, ProvenancePatch, ProvenanceSignature,
};
use crate::ccs::native_lifecycle::NativeLifecycleBundle;
use crate::ccs::policy::BuildPolicyConfig;
use crate::ccs::v2::PackageKindTagV2;
use crate::filesystem::path::sanitize_path;
use crate::repository::versioning::VersionScheme;
use crate::repository::{
    dependency_model::{DebianMultiArch, RepositoryRequirementGroup},
    package_relation::validate_native_relation,
    requirement::validate_requirement_group,
};
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
#[serde(deny_unknown_fields)]
pub struct CcsManifest {
    pub package: Package,

    #[serde(default)]
    pub provides: Provides,

    /// Exact positive requirement authority. Native conversions and CCS v2
    /// use this field for Boolean expressions.
    #[serde(default)]
    pub requirements: Vec<RepositoryRequirementGroup>,

    /// Exact source-native conflict, break, replacement, and obsolescence
    /// authority. These relations are deliberately separate from positive
    /// dependencies and provides.
    #[serde(default)]
    pub relations: Vec<RepositoryRequirementGroup>,

    #[serde(default)]
    pub suggests: Suggests,

    #[serde(default)]
    pub components: Components,

    #[serde(default)]
    pub hooks: Hooks,

    /// Scriptlet execution declarations and host-integration capabilities
    #[serde(default)]
    pub scriptlets: ScriptletDeclarations,

    /// Passive native lifecycle semantics bundle for converted packages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lifecycle: Option<NativeLifecycleBundle>,

    #[serde(default)]
    pub config: Config,

    #[serde(default)]
    pub build: Option<BuildInfo>,

    #[serde(default)]
    pub native_export: Option<NativeExport>,

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
        crate::repository::versioning::validate_repo_version(
            self.package.version_scheme,
            &self.package.version,
        )
        .map_err(|error| {
            ManifestError::Invalid(format!("invalid package.version contract: {error}"))
        })?;
        if self.package.release.is_empty() {
            return Err(ManifestError::MissingField("package.release".to_string()));
        }
        match (self.package.version_scheme, self.package.debian_multi_arch) {
            (VersionScheme::Debian, Some(_))
            | (VersionScheme::Conary | VersionScheme::Rpm | VersionScheme::Arch, None) => {}
            (VersionScheme::Debian, None) => {
                return Err(ManifestError::MissingField(
                    "package.debian_multi_arch".to_string(),
                ));
            }
            (_, Some(_)) => {
                return Err(ManifestError::Invalid(
                    "package.debian_multi_arch is valid only for Debian packages".to_string(),
                ));
            }
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
            validate_tmpfiles_fields(
                &entry.entry_type,
                &entry.path,
                &entry.mode,
                &entry.user,
                &entry.group,
                &entry.age,
                &entry.argument,
            )
            .map_err(|error| {
                ManifestError::Invalid(format!(
                    "invalid hooks.tmpfiles entry for '{}': {}",
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
            if !is_safe_declarative_unit_name(&unit.unit) {
                return Err(ManifestError::Invalid(format!(
                    "hooks.systemd unit '{}' must be a pathless, nonempty declarative unit name without NUL bytes",
                    unit.unit
                )));
            }
        }
        for service in &self.hooks.services {
            if !is_safe_declarative_unit_name(&service.name) {
                return Err(ManifestError::Invalid(format!(
                    "hooks.services name '{}' must be a pathless, nonempty declarative service name without NUL bytes",
                    service.name
                )));
            }
        }

        self.scriptlets.validate()?;
        for requirement in &self.requirements {
            if requirement.kind.is_negative_relation() {
                return Err(ManifestError::Invalid(
                    "negative package relation stored in positive requirements".to_string(),
                ));
            }
            validate_requirement_group(requirement, self.package.version_scheme).map_err(
                |error| {
                    ManifestError::Invalid(format!(
                        "invalid positive package requirement authority: {error}"
                    ))
                },
            )?;
        }
        for relation in &self.relations {
            validate_native_relation(relation, self.package.version_scheme).map_err(|error| {
                ManifestError::Invalid(format!("invalid package relation authority: {error}"))
            })?;
        }
        for capability in &self.file_capabilities {
            capability.validate()?;
        }
        if let Some(capabilities) = &self.capabilities {
            capabilities
                .validate_for_target_arch(
                    self.package.version_scheme,
                    self.package
                        .platform
                        .as_ref()
                        .and_then(|platform| platform.arch.as_deref()),
                )
                .map_err(|error| {
                    ManifestError::Invalid(format!(
                        "capability declaration validation failed: {error}"
                    ))
                })?;
        }
        if let Some(bundle) = &self.native_lifecycle {
            bundle.validate().map_err(|error| {
                ManifestError::Invalid(format!(
                    "native lifecycle bundle validation failed: {error}"
                ))
            })?;
            let bundle_scheme = bundle.version_scheme.repository_scheme();
            if self.package.version_scheme != bundle_scheme {
                return Err(ManifestError::Invalid(format!(
                    "package version scheme '{}' disagrees with native lifecycle bundle scheme '{}'",
                    self.package.version_scheme.as_str(),
                    bundle_scheme.as_str()
                )));
            }
        }

        Ok(())
    }

    /// Generate a minimal manifest for a new project
    pub fn new_minimal(name: &str, version: &str) -> Self {
        CcsManifest {
            package: Package {
                name: name.to_string(),
                version: version.to_string(),
                version_scheme: VersionScheme::Conary,
                description: format!("A new CCS package: {}", name),
                release: "1".to_string(),
                kind: PackageKindTagV2::Package,
                debian_multi_arch: None,
                license: None,
                homepage: None,
                repository: None,
                platform: None,
                authors: None,
            },
            provides: Provides::default(),
            requirements: Vec::new(),
            relations: Vec::new(),
            suggests: Suggests::default(),
            components: Components::default(),
            hooks: Hooks::default(),
            scriptlets: ScriptletDeclarations::default(),
            native_lifecycle: None,
            config: Config::default(),
            build: None,
            native_export: None,
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
#[serde(deny_unknown_fields)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub version_scheme: VersionScheme,
    pub description: String,

    pub release: String,

    pub kind: PackageKindTagV2,

    /// Exact Debian `Multi-Arch` control-field behavior.
    #[serde(default)]
    pub debian_multi_arch: Option<DebianMultiArch>,

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
#[serde(deny_unknown_fields)]
pub struct Provides {
    #[serde(default)]
    pub capabilities: Vec<String>,

    /// Exact shared-library capabilities declared by package metadata.
    #[serde(default)]
    pub sonames: Vec<String>,

    /// Exact executable capabilities declared by package metadata.
    #[serde(default)]
    pub binaries: Vec<String>,

    /// Exact pkg-config capabilities declared by package metadata.
    #[serde(default)]
    pub pkgconfig: Vec<String>,
}

/// Optional/suggested dependencies
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Suggests {
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Component configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Components {
    /// Exact author-declared glob rules for component assignment.
    #[serde(default)]
    pub rules: Vec<ComponentRule>,

    /// Exact author-declared file path assignments.
    #[serde(default)]
    pub files: HashMap<String, String>,

    /// Which components install by default
    #[serde(default = "default_components")]
    pub default: Vec<String>,
}

fn default_components() -> Vec<String> {
    vec!["runtime".to_string()]
}

impl Default for Components {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            files: HashMap::new(),
            default: default_components(),
        }
    }
}

/// An exact glob-to-component assignment rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentRule {
    pub path: String,
    pub component: String,
}

/// Configuration file tracking
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub files: Vec<crate::packages::traits::ConfigFileInfo>,
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

/// Native package export settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct NativeExport {
    #[serde(default)]
    pub rpm: Option<RpmExport>,

    #[serde(default)]
    pub deb: Option<DebExport>,

    #[serde(default)]
    pub arch: Option<ArchExport>,
}

/// RPM-specific export overrides.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RpmExport {
    #[serde(default)]
    pub group: Option<String>,

    #[serde(default)]
    pub requires: Vec<String>,

    #[serde(default)]
    pub provides: Vec<String>,
}

/// Debian-specific export overrides.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DebExport {
    #[serde(default)]
    pub section: Option<String>,

    #[serde(default)]
    pub priority: Option<String>,

    #[serde(default)]
    pub depends: Vec<String>,
}

/// Arch-specific export overrides.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ArchExport {
    #[serde(default)]
    pub groups: Vec<String>,

    /// Exact ALPM dependency strings for native export.
    #[serde(default)]
    pub depends: Vec<String>,
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
