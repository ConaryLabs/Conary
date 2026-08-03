// conary-core/src/ccs/convert/input.rs

//! Fallible package-format projection consumed only by foreign conversion.

use crate::packages::source_authority::{CcsPackageAuthority, SourcePackageAuthority};
use crate::packages::traits::{
    DiagnosticScriptletEvidence, NativeScriptletEntry, PackageFile, PackageFormat,
};
use crate::repository::dependency_model::{
    DebianMultiArch, ProvidedCapability, RepositoryRequirementGroup,
};
use crate::repository::versioning::VersionScheme;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ForeignConversionInput {
    pub package_path: PathBuf,
    pub source_authority: SourcePackageAuthority,
    pub description: Option<String>,
    pub files: Vec<PackageFile>,
    pub requirements: Vec<RepositoryRequirementGroup>,
    pub relations: Vec<RepositoryRequirementGroup>,
    pub diagnostic_scriptlet_evidence: Vec<DiagnosticScriptletEvidence>,
    pub native_scriptlet_abi: Vec<NativeScriptletEntry>,
}

impl ForeignConversionInput {
    pub fn from_package(package_path: PathBuf, package: &dyn PackageFormat) -> crate::Result<Self> {
        Ok(Self {
            package_path,
            source_authority: package.source_authority()?,
            description: package.description().map(str::to_string),
            files: package.files().to_vec(),
            requirements: package.requirements().to_vec(),
            relations: package.relations().to_vec(),
            diagnostic_scriptlet_evidence: Vec::new(),
            native_scriptlet_abi: package.native_scriptlet_abi().to_vec(),
        })
    }

    #[must_use]
    pub fn new(
        package_path: PathBuf,
        name: String,
        version: String,
        version_scheme: VersionScheme,
    ) -> Self {
        Self {
            package_path,
            source_authority: SourcePackageAuthority::Ccs(CcsPackageAuthority {
                name,
                version,
                version_scheme,
                architecture: None,
                debian_multi_arch: None,
                capabilities: Vec::new(),
                config: Vec::new(),
            }),
            description: None,
            files: Vec::new(),
            requirements: Vec::new(),
            relations: Vec::new(),
            diagnostic_scriptlet_evidence: Vec::new(),
            native_scriptlet_abi: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        self.source_authority.name()
    }

    pub fn version(&self) -> &str {
        self.source_authority.version()
    }

    pub fn version_scheme(&self) -> VersionScheme {
        self.source_authority.version_scheme()
    }

    pub fn architecture(&self) -> Option<&str> {
        self.source_authority.architecture()
    }

    pub fn debian_multi_arch(&self) -> Option<DebianMultiArch> {
        self.source_authority.debian_multi_arch()
    }

    pub fn resolution_capabilities(&self) -> crate::Result<Vec<ProvidedCapability>> {
        self.source_authority.resolution_capabilities()
    }

    pub fn package_path(&self) -> &Path {
        &self.package_path
    }
}
