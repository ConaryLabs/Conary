// conary-core/src/packages/traits.rs

//! Common traits for package format parsers

use crate::db::models::Trove;
use crate::error::Result;
use crate::payload::{PayloadContentAuthority, PayloadNode};
use crate::repository::dependency_model::RepositoryRequirementGroup;
use crate::repository::versioning::VersionScheme;

pub use crate::packages::native_abi::*;

/// Metadata about a file within a package
#[derive(Debug, Clone)]
pub struct PackageFile {
    pub path: String,
    pub node: PayloadNode,
    pub content: Option<PayloadContentAuthority>,
}

/// A file extracted from a package with its content
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    pub path: String,
    pub node: PayloadNode,
    pub content: Vec<u8>,
    pub content_authority: Option<PayloadContentAuthority>,
}

/// Dependency information
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub version: Option<String>,
    pub dep_type: DependencyType,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyType {
    Runtime,
    Build,
    Optional,
}

impl DependencyType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Build => "build",
            Self::Optional => "optional",
        }
    }
}

/// Non-authoritative lifecycle label attached to flattened diagnostic evidence.
///
/// Exact native lifecycle authority uses [`NativeLifecyclePath`] and
/// [`NativeScriptletEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticScriptletPhase {
    /// Before package installation
    PreInstall,
    /// After package installation
    PostInstall,
    /// Before package removal
    PreRemove,
    /// After package removal
    PostRemove,
    /// Before package upgrade (RPM-specific)
    PreUpgrade,
    /// After package upgrade (RPM-specific)
    PostUpgrade,
    /// Before transaction (RPM-specific)
    PreTransaction,
    /// After transaction (RPM-specific)
    PostTransaction,
    /// Trigger scripts (RPM-specific)
    Trigger,
}

impl std::fmt::Display for DiagnosticScriptletPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreInstall => write!(f, "pre-install"),
            Self::PostInstall => write!(f, "post-install"),
            Self::PreRemove => write!(f, "pre-remove"),
            Self::PostRemove => write!(f, "post-remove"),
            Self::PreUpgrade => write!(f, "pre-upgrade"),
            Self::PostUpgrade => write!(f, "post-upgrade"),
            Self::PreTransaction => write!(f, "pre-transaction"),
            Self::PostTransaction => write!(f, "post-transaction"),
            Self::Trigger => write!(f, "trigger"),
        }
    }
}

/// Non-authoritative flattened script text retained only for conversion diagnostics.
///
/// Install, remove, publication, and lifecycle planning must use
/// [`NativeScriptletEntry`] instead.
#[derive(Debug, Clone)]
pub struct DiagnosticScriptletEvidence {
    pub phase: DiagnosticScriptletPhase,
    pub interpreter: String,
    pub content: String,
    pub flags: Option<String>,
}

/// Configuration file information from a package
#[derive(Debug, Clone)]
pub struct ConfigFileInfo {
    /// Path to the config file
    pub path: String,
    /// If true, preserve user's version on upgrade (RPM %config(noreplace))
    pub noreplace: bool,
    /// If true, this is a ghost config (not in payload, just tracked)
    pub ghost: bool,
    /// Debian control declaration: remove the old conffile on the next
    /// upgrade, preserving a locally modified copy as `.dpkg-old`.
    pub remove_on_upgrade: bool,
}

/// Common interface for all package formats (RPM, DEB, Arch, etc.)
pub trait PackageFormat {
    /// Parse a package file from the given path
    fn parse(path: &str) -> Result<Self>
    where
        Self: Sized;

    /// Get the package name
    fn name(&self) -> &str;

    /// Get the package version
    fn version(&self) -> &str;

    /// Native version algebra for package requirements and provides.
    fn version_scheme(&self) -> VersionScheme;

    /// Get the package architecture (e.g., "x86_64", "aarch64")
    fn architecture(&self) -> Option<&str>;

    /// Get the package summary/description
    fn description(&self) -> Option<&str>;

    /// Get the list of files in the package
    fn files(&self) -> &[PackageFile];

    /// Get exact positive package requirements.
    ///
    /// This is the sole install and resolution authority; Boolean structure
    /// must remain intact through conversion and persistence.
    fn requirements(&self) -> &[RepositoryRequirementGroup];

    /// Get the list of native capabilities this package provides.
    ///
    /// Defaults to an empty slice for formats that do not expose native
    /// provide metadata.
    fn provides(&self) -> &[Dependency] {
        &[]
    }

    /// Get source-native conflict, break, replacement, and obsolete relations.
    ///
    /// These are persisted in the same typed requirement-group authority as
    /// positive requirements.
    fn relations(&self) -> &[RepositoryRequirementGroup] {
        &[]
    }

    /// Extract all file contents from the package
    ///
    /// Returns a vector of ExtractedFile containing file metadata and content.
    /// This is used during package installation to get the actual file data.
    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>>;

    /// Get byte-preserving native package-manager scriptlet ABI entries.
    ///
    /// Defaults to an empty slice for package formats or test doubles that do
    /// not expose native ABI metadata.
    fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
        &[]
    }

    /// Get the list of configuration files declared by the package
    ///
    /// For RPM: files marked with %config or %config(noreplace)
    /// For DEB: files listed in DEBIAN/conffiles
    /// For Arch: files listed in backup array in .PKGINFO
    ///
    /// Default implementation returns empty slice - config files detected
    /// automatically by path (e.g., /etc/*) during installation.
    fn config_files(&self) -> &[ConfigFileInfo] {
        &[]
    }

    /// Convert this package to a Trove representation
    fn to_trove(&self) -> Trove;
}
