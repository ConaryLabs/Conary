// conary-core/src/ccs/package.rs

//! CCS package parser implementing PackageFormat trait
//!
//! This module provides a PackageFormat implementation for CCS packages,
//! enabling them to be installed using the same infrastructure as RPM/DEB/Arch/eopkg.

mod v3_projection;

use crate::ccs::builder::{ComponentData, FileEntry};
use crate::ccs::manifest::CcsManifest;
use crate::db::models::{InstallReason, InstallSource, Trove, TroveType};
use crate::error::{Error, Result};
use crate::packages::config_authority::SourceConfigDeclaration;
use crate::packages::payload::PackagePayload;
use crate::packages::traits::{PackageFile, PackageFormat};
use crate::repository::dependency_model::ProvidedCapability;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use v3_projection::{
    capabilities_from_v3_authority, config_declarations_from_v3_authority, files_from_v3_authority,
    install_manifest_from_v3,
};

/// A parsed CCS package ready for installation
#[derive(Debug)]
pub struct CcsPackage {
    /// Path to the .ccs package file
    package_path: PathBuf,
    /// Parsed manifest
    manifest: CcsManifest,
    /// Parsed v3 authority, when this package is native CCS v3.
    v3_authority: Option<crate::ccs::v3::AuthorityDocumentV3>,
    /// Parsed v3 build attestation envelope from MANIFEST.attestation.json.
    v3_build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    /// Parsed v3 foreign conversion boundary from MANIFEST.conversion-boundary.json.
    v3_foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    /// File entries from FILES.json
    files: Vec<FileEntry>,
    /// Component data
    components: HashMap<String, ComponentData>,
    /// Reopenable payload objects authenticated by the verification capability.
    payload: PackagePayload,
    /// Cached PackageFile list for the trait
    package_files: Vec<PackageFile>,
    /// Cached exact positive requirements for the trait.
    requirements: Vec<crate::repository::dependency_model::RepositoryRequirementGroup>,
    /// Cached exact capability providers for the trait
    resolution_capabilities: Vec<ProvidedCapability>,
    /// Cached exact signed config declarations for transaction projection.
    config_declarations: Vec<SourceConfigDeclaration>,
}

impl CcsPackage {
    /// Get the manifest
    pub fn manifest(&self) -> &CcsManifest {
        &self.manifest
    }

    /// Get the parsed v3 authority, when present.
    pub fn v3_authority(&self) -> Option<&crate::ccs::v3::AuthorityDocumentV3> {
        self.v3_authority.as_ref()
    }

    pub fn v3_build_attestation(
        &self,
    ) -> Option<&crate::ccs::attestation::BuildAttestationEnvelope> {
        self.v3_build_attestation.as_ref()
    }

    pub fn v3_foreign_conversion_boundary(
        &self,
    ) -> Option<&crate::ccs::attestation::ForeignConversionBoundary> {
        self.v3_foreign_conversion_boundary.as_ref()
    }

    pub fn from_verified_archive(
        path: &str,
        verification: &crate::ccs::verify::VerifiedCcsArchive,
    ) -> Result<Self> {
        let package_path = PathBuf::from(path);
        let authority = verification.authority();
        let manifest = install_manifest_from_v3(
            authority,
            verification.build_attestation().cloned(),
            verification.foreign_conversion_boundary().cloned(),
        )?;
        let files = files_from_v3_authority(authority)?;
        let requirements = Self::convert_requirements(&manifest);
        let resolution_capabilities = capabilities_from_v3_authority(authority);
        let package_files = Self::convert_files(&files);
        let config_declarations = config_declarations_from_v3_authority(authority)?;
        Ok(Self {
            package_path,
            manifest,
            v3_authority: Some(authority.clone()),
            v3_build_attestation: verification.build_attestation().cloned(),
            v3_foreign_conversion_boundary: verification.foreign_conversion_boundary().cloned(),
            files,
            components: verification.components().clone(),
            payload: verification.payload().clone(),
            package_files,
            requirements,
            resolution_capabilities,
            config_declarations,
        })
    }

    #[cfg(test)]
    pub(crate) fn manifest_mut_for_tests(&mut self) -> &mut CcsManifest {
        &mut self.manifest
    }

    /// Get the file entries
    pub fn file_entries(&self) -> &[FileEntry] {
        &self.files
    }

    /// Get the components
    pub fn components(&self) -> &HashMap<String, ComponentData> {
        &self.components
    }

    /// Get the package path
    pub fn package_path(&self) -> &Path {
        &self.package_path
    }

    /// Authenticated, independently reopenable payload sources.
    pub(crate) fn payload(&self) -> &PackagePayload {
        &self.payload
    }

    fn convert_requirements(
        manifest: &CcsManifest,
    ) -> Vec<crate::repository::dependency_model::RepositoryRequirementGroup> {
        manifest.requirements.clone()
    }

    /// Convert CCS file entries to PackageFile list
    fn convert_files(files: &[FileEntry]) -> Vec<PackageFile> {
        files
            .iter()
            .map(|f| PackageFile {
                path: f.path.clone(),
                node: f.node.clone(),
                content: f.content.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
impl CcsPackage {
    pub(crate) fn from_v3_authority_for_tests(
        authority: crate::ccs::v3::AuthorityDocumentV3,
        build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
        foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    ) -> Result<Self> {
        let manifest = install_manifest_from_v3(
            &authority,
            build_attestation.clone(),
            foreign_conversion_boundary.clone(),
        )?;
        let files = files_from_v3_authority(&authority)?;
        let requirements = Self::convert_requirements(&manifest);
        let resolution_capabilities = capabilities_from_v3_authority(&authority);
        let package_files = Self::convert_files(&files);
        let config_declarations = config_declarations_from_v3_authority(&authority)?;
        Ok(Self {
            package_path: PathBuf::from("v3-test.ccs"),
            manifest,
            v3_authority: Some(authority),
            v3_build_attestation: build_attestation,
            v3_foreign_conversion_boundary: foreign_conversion_boundary,
            files,
            components: HashMap::new(),
            payload: PackagePayload::default(),
            package_files,
            requirements,
            resolution_capabilities,
            config_declarations,
        })
    }
}

impl PackageFormat for CcsPackage {
    fn parse(path: &str) -> Result<Self>
    where
        Self: Sized,
    {
        Err(Error::ParseError(format!(
            "CCS package {} requires a VerifiedCcsArchive capability; call verify_package before constructing CcsPackage",
            path
        )))
    }

    fn name(&self) -> &str {
        &self.manifest.package.name
    }

    fn version(&self) -> &str {
        &self.manifest.package.version
    }

    fn package_release(&self) -> Option<&str> {
        Some(&self.manifest.package.release)
    }

    fn version_scheme(&self) -> crate::repository::versioning::VersionScheme {
        self.manifest.package.version_scheme
    }

    fn architecture(&self) -> Option<&str> {
        self.manifest
            .package
            .platform
            .as_ref()
            .and_then(|p| p.arch.as_deref())
    }

    fn debian_multi_arch(&self) -> Option<crate::repository::dependency_model::DebianMultiArch> {
        self.v3_authority
            .as_ref()
            .and_then(|authority| authority.identity.debian_multi_arch)
    }

    fn description(&self) -> Option<&str> {
        Some(&self.manifest.package.description)
    }

    fn files(&self) -> &[PackageFile] {
        &self.package_files
    }

    fn requirements(&self) -> &[crate::repository::dependency_model::RepositoryRequirementGroup] {
        &self.requirements
    }

    fn resolution_capabilities(&self) -> Result<Vec<ProvidedCapability>> {
        Ok(self.resolution_capabilities.clone())
    }

    fn relations(&self) -> &[crate::repository::dependency_model::RepositoryRequirementGroup] {
        &self.manifest.relations
    }

    fn package_payload(&self) -> Result<PackagePayload> {
        Ok(self.payload.clone())
    }

    fn config_declarations(&self) -> Result<Vec<SourceConfigDeclaration>> {
        Ok(self.config_declarations.clone())
    }

    fn to_trove(&self) -> Trove {
        Trove {
            id: None,
            name: self.manifest.package.name.clone(),
            version: self.manifest.package.version.clone(),
            package_release: Some(self.manifest.package.release.clone()),
            trove_type: TroveType::Package,
            architecture: self.architecture().map(String::from),
            debian_multi_arch: self.debian_multi_arch(),
            description: Some(self.manifest.package.description.clone()),
            installed_at: None,
            installed_by_changeset_id: None,
            install_source: InstallSource::File,
            install_reason: InstallReason::Explicit,
            flavor_spec: None,
            pinned: false,
            selection_reason: None,
            label_id: None,
            orphan_since: None,
            source_profile: None,
            version_scheme: self.manifest.package.version_scheme,
            native_package_identity: None,
            installed_from_repository_id: None,
        }
    }
}

#[cfg(test)]
mod tests;
