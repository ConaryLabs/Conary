// conary-core/src/packages/traits.rs

//! Common traits for package format parsers

use crate::db::models::Trove;
use crate::error::Result;
use crate::packages::config_authority::SourceConfigDeclaration;
use crate::packages::payload::PackagePayload;
use crate::packages::source_authority::{CcsPackageAuthority, SourcePackageAuthority};
use crate::payload::{PayloadContentAuthority, PayloadNode};
use crate::repository::dependency_model::{DebianMultiArch, RepositoryRequirementGroup};
use crate::repository::versioning::VersionScheme;

pub use crate::packages::native_abi::*;
pub use crate::repository::dependency_model::ProvidedCapability;

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

    /// Monotonic signed CCS build release, absent for a source-native
    /// artifact that has not been wrapped in a CCS envelope.
    fn package_release(&self) -> Option<&str> {
        None
    }

    /// Native version algebra for package requirements and provides.
    fn version_scheme(&self) -> VersionScheme;

    /// Get the package architecture (e.g., "x86_64", "aarch64")
    fn architecture(&self) -> Option<&str>;

    /// Exact Debian `Multi-Arch` behavior, when the source format defines it.
    fn debian_multi_arch(&self) -> Option<DebianMultiArch> {
        None
    }

    /// Exact source-format identity and declared capability records.
    fn source_authority(&self) -> Result<SourcePackageAuthority> {
        let capabilities = self
            .resolution_capabilities()?
            .into_iter()
            .filter(|capability| {
                capability.provenance
                    != crate::repository::dependency_model::CapabilityProvenance::ExactIdentity
            })
            .collect();
        Ok(SourcePackageAuthority::Ccs(CcsPackageAuthority {
            name: self.name().to_string(),
            version: self.version().to_string(),
            version_scheme: self.version_scheme(),
            architecture: self.architecture().map(str::to_string),
            debian_multi_arch: self.debian_multi_arch(),
            capabilities,
            config: Vec::new(),
        }))
    }

    /// Get the package summary/description
    fn description(&self) -> Option<&str>;

    /// Get the list of files in the package
    fn files(&self) -> &[PackageFile];

    /// Get exact positive package requirements.
    ///
    /// This is the sole install and resolution authority; Boolean structure
    /// must remain intact through conversion and persistence.
    fn requirements(&self) -> &[RepositoryRequirementGroup];

    /// Project exact identity and declared capabilities for resolution.
    fn resolution_capabilities(&self) -> Result<Vec<ProvidedCapability>> {
        Ok(Vec::new())
    }

    /// Get source-native conflict, break, replacement, and obsolete relations.
    ///
    /// These are persisted in the same typed requirement-group authority as
    /// positive requirements.
    fn relations(&self) -> &[RepositoryRequirementGroup] {
        &[]
    }

    /// Open the complete package payload as independently reopenable streams.
    ///
    /// This is the sole payload authority for install, conversion, trusted
    /// intake, and future package backends.
    fn package_payload(&self) -> Result<PackagePayload>;

    /// Explicitly materialize the complete payload in memory.
    ///
    /// This convenience is only for tests and tooling whose input is already
    /// bounded. Install, conversion, and trusted intake must use
    /// [`Self::package_payload`].
    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>> {
        self.package_payload()?.to_extracted_in_memory()
    }

    /// Get byte-preserving native package-manager scriptlet ABI entries.
    ///
    /// Defaults to an empty slice for package formats or test doubles that do
    /// not expose native ABI metadata.
    fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
        &[]
    }

    /// Fallibly project exact signed/source config declarations for planning.
    fn config_declarations(&self) -> Result<Vec<SourceConfigDeclaration>> {
        self.source_authority()?.config_declarations()
    }

    /// Convert this package to a Trove representation
    fn to_trove(&self) -> Trove;
}
