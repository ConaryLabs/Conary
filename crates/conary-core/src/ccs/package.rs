// conary-core/src/ccs/package.rs

//! CCS package parser implementing PackageFormat trait
//!
//! This module provides a PackageFormat implementation for CCS packages,
//! enabling them to be installed using the same infrastructure as RPM/DEB/Arch.

mod v2_projection;

use crate::ccs::builder::{ComponentData, FileEntry};
use crate::ccs::manifest::CcsManifest;
use crate::db::models::{InstallReason, InstallSource, Trove, TroveType};
use crate::error::{Error, Result};
use crate::hash;
use crate::packages::traits::{
    ConfigFileInfo, Dependency, ExtractedFile, PackageFile, PackageFormat,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::debug;
use v2_projection::{
    files_from_v2_authority, install_manifest_from_v2, provides_from_v2_authority,
};

/// A parsed CCS package ready for installation
#[derive(Debug)]
pub struct CcsPackage {
    /// Path to the .ccs package file
    package_path: PathBuf,
    /// Parsed manifest
    manifest: CcsManifest,
    /// Parsed v2 authority, when this package is native CCS v2.
    v2_authority: Option<crate::ccs::v2::AuthorityDocumentV2>,
    /// Parsed v2 build attestation envelope from MANIFEST.attestation.json.
    v2_build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    /// Parsed v2 foreign conversion boundary from MANIFEST.conversion-boundary.json.
    v2_foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    /// File entries from FILES.json
    files: Vec<FileEntry>,
    /// Component data
    components: HashMap<String, ComponentData>,
    /// Payload objects authenticated by the verification capability.
    blobs: HashMap<String, Vec<u8>>,
    /// Cached PackageFile list for the trait
    package_files: Vec<PackageFile>,
    /// Cached exact positive requirements for the trait.
    requirements: Vec<crate::repository::dependency_model::RepositoryRequirementGroup>,
    /// Cached exact capability providers for the trait
    provides: Vec<Dependency>,
    /// Cached config files for the trait
    config_files_cache: Vec<ConfigFileInfo>,
}

impl CcsPackage {
    fn validate_file_content(file: &FileEntry, content: &[u8]) -> Result<()> {
        let authority = file.content.as_ref().ok_or_else(|| {
            Error::IoError(format!(
                "Regular payload node {} has no content authority",
                file.path
            ))
        })?;
        let actual_size = u64::try_from(content.len()).map_err(|_| {
            Error::IoError(format!("File content too large to validate: {}", file.path))
        })?;
        if actual_size != authority.size {
            if actual_size < authority.size {
                return Err(Error::IoError(format!(
                    "File size mismatch (truncated) for {}: expected {} bytes, got {}",
                    file.path, authority.size, actual_size
                )));
            }
            return Err(Error::IoError(format!(
                "File size mismatch for {}: expected {}, got {}",
                file.path, authority.size, actual_size
            )));
        }

        let actual_hash = hash::sha256(content);
        if actual_hash != authority.sha256 {
            return Err(Error::ChecksumMismatch {
                expected: authority.sha256.clone(),
                actual: actual_hash,
            });
        }

        Ok(())
    }

    /// Get the manifest
    pub fn manifest(&self) -> &CcsManifest {
        &self.manifest
    }

    /// Get the parsed v2 authority, when present.
    pub fn v2_authority(&self) -> Option<&crate::ccs::v2::AuthorityDocumentV2> {
        self.v2_authority.as_ref()
    }

    pub fn v2_build_attestation(
        &self,
    ) -> Option<&crate::ccs::attestation::BuildAttestationEnvelope> {
        self.v2_build_attestation.as_ref()
    }

    pub fn v2_foreign_conversion_boundary(
        &self,
    ) -> Option<&crate::ccs::attestation::ForeignConversionBoundary> {
        self.v2_foreign_conversion_boundary.as_ref()
    }

    pub fn from_verified_archive(
        path: &str,
        verification: &crate::ccs::verify::VerifiedCcsArchive,
    ) -> Result<Self> {
        let package_path = PathBuf::from(path);
        let contents = verification.archive();
        let authority = verification.authority();
        let manifest = install_manifest_from_v2(
            authority,
            contents.v2_build_attestation.clone(),
            contents.v2_foreign_conversion_boundary.clone(),
        )?;
        let files = files_from_v2_authority(authority)?;
        let requirements = Self::convert_requirements(&manifest);
        let provides = provides_from_v2_authority(authority);
        let package_files = Self::convert_files(&files);
        Ok(Self {
            package_path,
            manifest,
            v2_authority: Some(authority.clone()),
            v2_build_attestation: contents.v2_build_attestation.clone(),
            v2_foreign_conversion_boundary: contents.v2_foreign_conversion_boundary.clone(),
            files,
            components: contents.components.clone(),
            blobs: contents.blobs.clone(),
            package_files,
            requirements,
            provides,
            config_files_cache: Vec::new(),
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

    /// Extract file contents from the package
    ///
    /// This re-reads the archive and returns the content blobs by hash.
    pub fn extract_all_content(&self) -> Result<HashMap<String, Vec<u8>>> {
        debug!(
            "Extracted {} content blobs from {}",
            self.blobs.len(),
            self.package_path.display()
        );

        Ok(self.blobs.clone())
    }
}

#[cfg(test)]
impl CcsPackage {
    pub(crate) fn from_v2_authority_for_tests(
        authority: crate::ccs::v2::AuthorityDocumentV2,
        build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
        foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    ) -> Result<Self> {
        let manifest = install_manifest_from_v2(
            &authority,
            build_attestation.clone(),
            foreign_conversion_boundary.clone(),
        )?;
        let files = files_from_v2_authority(&authority)?;
        let requirements = Self::convert_requirements(&manifest);
        let provides = provides_from_v2_authority(&authority);
        let package_files = Self::convert_files(&files);
        Ok(Self {
            package_path: PathBuf::from("v2-test.ccs"),
            manifest,
            v2_authority: Some(authority),
            v2_build_attestation: build_attestation,
            v2_foreign_conversion_boundary: foreign_conversion_boundary,
            files,
            components: HashMap::new(),
            blobs: HashMap::new(),
            package_files,
            requirements,
            provides,
            config_files_cache: Vec::new(),
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

    fn description(&self) -> Option<&str> {
        Some(&self.manifest.package.description)
    }

    fn files(&self) -> &[PackageFile] {
        &self.package_files
    }

    fn requirements(&self) -> &[crate::repository::dependency_model::RepositoryRequirementGroup] {
        &self.requirements
    }

    fn provides(&self) -> &[Dependency] {
        &self.provides
    }

    fn relations(&self) -> &[crate::repository::dependency_model::RepositoryRequirementGroup] {
        &self.manifest.relations
    }

    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>> {
        let blobs = self.extract_all_content()?;
        let mut extracted = Vec::with_capacity(self.files.len());

        for file in &self.files {
            let content = if !file.node.kind.is_regular() {
                Vec::new()
            } else if let Some(chunk_hashes) = &file.chunks {
                // File is chunked - reassemble from chunks
                let capacity = file
                    .content
                    .as_ref()
                    .map_or(0, |authority| authority.size as usize);
                let mut reassembled = Vec::with_capacity(capacity);
                for chunk_hash in chunk_hashes {
                    let chunk_data = blobs.get(chunk_hash).ok_or_else(|| {
                        crate::Error::Io(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!("Chunk {} not found for file {}", chunk_hash, file.path),
                        ))
                    })?;
                    reassembled.extend_from_slice(chunk_data);
                }
                Self::validate_file_content(file, &reassembled)?;
                reassembled
            } else {
                // Non-chunked file - look up by file hash
                let authority = file.content.as_ref().ok_or_else(|| {
                    crate::Error::IoError(format!(
                        "Regular payload node {} has no content authority",
                        file.path
                    ))
                })?;
                let content = blobs.get(&authority.sha256).cloned().ok_or_else(|| {
                    crate::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "Content not found for file {} (hash: {})",
                            file.path, authority.sha256
                        ),
                    ))
                })?;
                Self::validate_file_content(file, &content)?;
                content
            };

            extracted.push(ExtractedFile {
                path: file.path.clone(),
                node: file.node.clone(),
                content,
                content_authority: file.content.clone(),
            });
        }

        debug!("Extracted {} files from CCS package", extracted.len());

        Ok(extracted)
    }

    fn config_files(&self) -> &[ConfigFileInfo] {
        &self.config_files_cache
    }

    fn to_trove(&self) -> Trove {
        Trove {
            id: None,
            name: self.manifest.package.name.clone(),
            version: self.manifest.package.version.clone(),
            trove_type: TroveType::Package,
            architecture: self.architecture().map(String::from),
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
            source_distro: None,
            version_scheme: self.manifest.package.version_scheme,
            native_package_identity: None,
            installed_from_repository_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::VerifiedCcsArchive;
    use crate::ccs::builder::{CcsBuilder, write_signed_current_ccs_package};
    use crate::ccs::signing::SigningKeyPair;
    use crate::filesystem::CasStore;
    use crate::payload::PayloadNodeKind;
    use flate2::Compression;
    use flate2::read::GzDecoder;
    use flate2::write::GzEncoder;
    use std::fs::{self, File};
    use tar::{Archive, Builder};
    use tempfile::TempDir;

    fn build_test_package() -> (TempDir, std::path::PathBuf, SigningKeyPair) {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("src");
        fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
        fs::write(source_dir.join("usr/bin/hello"), b"hello world\n").unwrap();

        let manifest = CcsManifest::parse(
            r#"
[package]
name = "test-package"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "test fixture"
license = "MIT"
"#,
        )
        .unwrap();

        let result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
        let package_path = temp.path().join("test-package.ccs");
        let signing_key = SigningKeyPair::generate();
        write_signed_current_ccs_package(&result, &package_path, &signing_key, false).unwrap();
        (temp, package_path, signing_key)
    }

    fn verify_test_package(path: &Path, signing_key: &SigningKeyPair) -> VerifiedCcsArchive {
        crate::ccs::verify::verify_package(
            path,
            &crate::ccs::verify::TrustPolicy::strict(vec![signing_key.public_key_base64()]),
        )
        .unwrap()
    }

    fn mutate_package(source_path: &Path, output_path: &Path, mutator: impl FnOnce(&Path)) {
        let unpack_dir = tempfile::tempdir().unwrap();
        let source_file = File::open(source_path).unwrap();
        let decoder = GzDecoder::new(source_file);
        let mut archive = Archive::new(decoder);
        archive.unpack(unpack_dir.path()).unwrap();
        mutator(unpack_dir.path());

        let output_file = File::create(output_path).unwrap();
        let encoder = GzEncoder::new(output_file, Compression::default());
        let mut builder = Builder::new(encoder);
        builder.append_dir_all(".", unpack_dir.path()).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();
    }

    #[test]
    fn test_symlink_hash_consistency() {
        // Verify we use consistent symlink hashing
        let target = "/usr/lib/libfoo.so.1";
        let hash = CasStore::compute_symlink_hash(target);
        assert_eq!(hash.len(), 64);
    }

    #[cfg(unix)]
    #[test]
    fn test_extract_preserves_symlink_target() {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("src");
        fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
        fs::write(source_dir.join("usr/bin/bash"), b"bash\n").unwrap();
        std::os::unix::fs::symlink("bash", source_dir.join("usr/bin/sh")).unwrap();

        let manifest = CcsManifest::parse(
            r#"
[package]
name = "symlink-package"
version = "1.0.0"
version_scheme = "conary"
release = "1"
kind = "package"
description = "symlink fixture"
license = "MIT"
"#,
        )
        .unwrap();

        let result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
        let package_path = temp.path().join("symlink-package.ccs");
        let signing_key = SigningKeyPair::generate();
        write_signed_current_ccs_package(&result, &package_path, &signing_key, false).unwrap();

        let verification = verify_test_package(&package_path, &signing_key);
        let package =
            CcsPackage::from_verified_archive(package_path.to_str().unwrap(), &verification)
                .unwrap();
        let files = package.extract_file_contents().unwrap();
        let sh = files
            .iter()
            .find(|file| file.path == "/usr/bin/sh")
            .expect("expected /usr/bin/sh symlink");

        assert_eq!(
            sh.node.kind,
            PayloadNodeKind::Symlink {
                target: "bash".to_string()
            }
        );
    }

    #[test]
    fn test_extract_rejects_truncated_content() {
        let (_temp, package_path, signing_key) = build_test_package();
        let corrupted_path = package_path.with_file_name("truncated.ccs");
        mutate_package(&package_path, &corrupted_path, |root| {
            let object = fs::read_dir(root.join("objects"))
                .unwrap()
                .flatten()
                .find(|entry| entry.path().is_dir())
                .unwrap()
                .path();
            let object_file = fs::read_dir(object)
                .unwrap()
                .flatten()
                .find(|entry| entry.path().is_file())
                .unwrap()
                .path();
            let original = fs::read(&object_file).unwrap();
            fs::write(&object_file, &original[..original.len() / 2]).unwrap();
        });

        let err = crate::ccs::verify::verify_package(
            &corrupted_path,
            &crate::ccs::verify::TrustPolicy::strict(vec![signing_key.public_key_base64()]),
        )
        .unwrap_err();
        let err = format!("{err:#}");
        assert!(
            err.contains("object path hash mismatch")
                || err.contains("payload hash mismatch")
                || err.contains("payload size mismatch"),
            "{err}"
        );
    }

    #[test]
    fn current_writer_rejects_declared_size_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let package_path = temp.path().join("size-lie.ccs");
        let mut authority =
            crate::ccs::v2::test_support::package_authority_with_one_file("size-lie");
        let crate::ccs::v2::schema::PackageKindV2::Package(data) = &mut authority.kind else {
            panic!("fixture must be a package");
        };
        data.files[0].content.as_mut().unwrap().size += 1;
        authority.components.get_mut("main").unwrap().total_size += 1;
        let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
        let signing_key = SigningKeyPair::generate();

        let error = crate::ccs::builder::write_v2_ccs_package(
            &authority,
            &payloads,
            &package_path,
            &signing_key,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not match signed v2 authority")
        );
    }

    #[test]
    fn v2_packages_do_not_reconstruct_missing_authority_from_projections() {
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("adapter-v2");
        let package =
            CcsPackage::from_v2_authority_for_tests(authority.clone(), None, None).unwrap();
        assert_eq!(package.manifest().package.name, "adapter-v2");
        assert!(package.v2_authority().is_some());
    }

    #[test]
    fn v2_positive_dependency_kinds_reach_package_format_without_string_guessing() {
        use crate::ccs::v2::schema::{DependencyEntryV2, DependencyKindV2};
        use crate::repository::dependency_model::{
            RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementGroup,
            RepositoryRequirementKind,
        };

        let requirement = |name: &str, capability_kind| {
            RepositoryRequirementGroup::simple(
                RepositoryRequirementKind::Depends,
                RepositoryRequirementClause {
                    name: name.to_string(),
                    capability_kind: Some(capability_kind),
                    version_constraint: None,
                    native_text: None,
                },
            )
        };

        let mut authority =
            crate::ccs::v2::test_support::package_authority_with_one_file("typed-v2");
        authority.requirements = vec![
            requirement("web-server", RepositoryCapabilityKind::Virtual),
            requirement("soname(libssl.so.3)", RepositoryCapabilityKind::Soname),
            requirement("binary(sh)", RepositoryCapabilityKind::Generic),
        ];
        authority.provides = vec![
            DependencyEntryV2 {
                kind: DependencyKindV2::Capability,
                name: "http-client".to_string(),
                version_constraint: None,
                target: None,
                component: None,
            },
            DependencyEntryV2 {
                kind: DependencyKindV2::PkgConfig,
                name: "typed-v2".to_string(),
                version_constraint: None,
                target: None,
                component: None,
            },
        ];

        let package =
            CcsPackage::from_v2_authority_for_tests(authority.clone(), None, None).unwrap();
        let provides = package
            .provides()
            .iter()
            .map(|provide| provide.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(package.requirements(), authority.requirements.as_slice());
        assert_eq!(provides, vec!["http-client", "pkgconfig(typed-v2)"]);
        assert_eq!(
            package.manifest().provides.capabilities,
            vec!["http-client"]
        );
        assert_eq!(package.manifest().provides.pkgconfig, vec!["typed-v2"]);
    }

    #[test]
    fn v2_projection_preserves_attestation_metadata() {
        let authority =
            crate::ccs::v2::test_support::package_authority_with_one_file("attested-v2");
        let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("publish");
        let envelope = crate::ccs::attestation::test_support::sample_envelope_for_tests(&key);
        let package =
            CcsPackage::from_v2_authority_for_tests(authority, Some(envelope.clone()), None)
                .unwrap();
        let provenance = package.manifest().provenance.as_ref().unwrap();
        assert_eq!(provenance.build_attestation.as_ref(), Some(&envelope));
    }

    #[test]
    fn parse_rejects_native_v2_and_verified_parse_accepts_after_verification() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("adapter-v2.ccs");
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("adapter-v2");
        let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
        let key = crate::ccs::signing::SigningKeyPair::generate();
        crate::ccs::builder::write_v2_ccs_package(
            &authority, &payloads, &path, &key, None, None, None,
        )
        .unwrap();

        let plain_error = CcsPackage::parse(path.to_str().unwrap()).unwrap_err();
        assert!(
            plain_error
                .to_string()
                .contains("requires a VerifiedCcsArchive capability")
        );

        let verification = crate::ccs::verify::verify_package(
            &path,
            &crate::ccs::verify::TrustPolicy::strict(vec![key.public_key_base64()]),
        )
        .unwrap();
        let package =
            CcsPackage::from_verified_archive(path.to_str().unwrap(), &verification).unwrap();
        assert!(package.v2_authority().is_some());

        let untrusted_key = crate::ccs::signing::SigningKeyPair::generate();
        let verified_error = crate::ccs::verify::verify_package(
            &path,
            &crate::ccs::verify::TrustPolicy::strict(vec![untrusted_key.public_key_base64()]),
        )
        .unwrap_err();
        assert!(format!("{verified_error:#}").contains("not trusted"));
    }
}
