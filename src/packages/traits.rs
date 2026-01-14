// src/packages/traits.rs

//! Common traits for package format parsers

use crate::db::models::Trove;
use crate::error::Result;

/// Metadata about a file within a package
#[derive(Debug, Clone)]
pub struct PackageFile {
    pub path: String,
    pub size: i64,
    pub mode: i32,
    pub sha256: Option<String>,
}

/// A file extracted from a package with its content
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    pub path: String,
    pub content: Vec<u8>,
    pub size: i64,
    pub mode: i32,
    pub sha256: Option<String>,
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

/// When a scriptlet runs during the package lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptletPhase {
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

impl std::fmt::Display for ScriptletPhase {
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

/// A scriptlet (install/remove hook) from a package
#[derive(Debug, Clone)]
pub struct Scriptlet {
    /// When this scriptlet runs
    pub phase: ScriptletPhase,
    /// The interpreter to use (e.g., "/bin/sh", "/bin/bash", "/usr/bin/lua")
    pub interpreter: String,
    /// The script content
    pub content: String,
    /// Optional flags/arguments for the interpreter
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

    /// Get the package architecture (e.g., "x86_64", "aarch64")
    fn architecture(&self) -> Option<&str>;

    /// Get the package summary/description
    fn description(&self) -> Option<&str>;

    /// Get the list of files in the package
    fn files(&self) -> &[PackageFile];

    /// Get the list of dependencies
    fn dependencies(&self) -> &[Dependency];

    /// Extract all file contents from the package
    ///
    /// Returns a vector of ExtractedFile containing file metadata and content.
    /// This is used during package installation to get the actual file data.
    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>>;

    /// Get the scriptlets (install/remove hooks) from the package
    ///
    /// Returns a vector of Scriptlet containing phase, interpreter, and content.
    /// Default implementation returns empty vec for formats that don't support scriptlets.
    fn scriptlets(&self) -> Vec<Scriptlet> {
        Vec::new()
    }

    /// Get the list of configuration files declared by the package
    ///
    /// For RPM: files marked with %config or %config(noreplace)
    /// For DEB: files listed in DEBIAN/conffiles
    /// For Arch: files listed in backup array in .PKGINFO
    ///
    /// Default implementation returns empty vec - config files detected
    /// automatically by path (e.g., /etc/*) during installation.
    fn config_files(&self) -> Vec<ConfigFileInfo> {
        Vec::new()
    }

    /// Convert this package to a Trove representation
    fn to_trove(&self) -> Trove;
}
