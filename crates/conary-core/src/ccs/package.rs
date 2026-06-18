// conary-core/src/ccs/package.rs

//! CCS package parser implementing PackageFormat trait
//!
//! This module provides a PackageFormat implementation for CCS packages,
//! enabling them to be installed using the same infrastructure as RPM/DEB/Arch.

use crate::ccs::archive_reader::read_ccs_archive;
use crate::ccs::binary_manifest::BinaryManifest;
use crate::ccs::builder::{ComponentData, FileEntry, FileType as CcsFileType};
use crate::ccs::manifest::{CcsManifest, Redirects};
use crate::ccs::policy::BuildPolicyConfig;
use crate::db::models::{InstallReason, InstallSource, Trove, TroveType};
use crate::error::{Error, Result};
use crate::filesystem::CasStore;
use crate::hash;
use crate::packages::traits::{
    ConfigFileInfo, Dependency, DependencyType, ExtractedFile, PackageFile, PackageFormat,
    Scriptlet,
};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use tracing::debug;

/// A parsed CCS package ready for installation
#[derive(Debug)]
pub struct CcsPackage {
    /// Path to the .ccs package file
    package_path: PathBuf,
    /// Parsed manifest
    manifest: CcsManifest,
    /// Parsed CBOR manifest, when the package carries the binary format
    binary_manifest: Option<BinaryManifest>,
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
    /// Cached PackageFile list for the trait
    package_files: Vec<PackageFile>,
    /// Cached dependencies for the trait
    dependencies: Vec<Dependency>,
    /// Cached config files for the trait
    config_files_cache: Vec<ConfigFileInfo>,
}

/// Convert a BinaryManifest to CcsManifest for internal compatibility.
///
/// This is also exposed publicly for use by the verify and inspector modules.
///
/// Note: The binary manifest format does not carry all CcsManifest fields.
/// The following fields are unavailable from CBOR and will use defaults:
/// `homepage`, `repository`, `authors`, `build.environment`, `build.commands`,
/// `suggests`, `components`, `scriptlets`, `legacy_scriptlets`, `config`,
/// `legacy`, `policy`, `provenance`, and `redirects`.
pub fn convert_binary_to_ccs_manifest(
    bin: &crate::ccs::binary_manifest::BinaryManifest,
) -> CcsManifest {
    use crate::ccs::manifest::{
        AlternativeHook, BuildInfo, Capability, Components, Config, DirectoryHook, GroupHook,
        Hooks, Package, PackageDep, Platform, Provides, Requires, ScriptHook,
        ScriptletDeclarations, Service, Suggests, SysctlHook, SystemdHook, TmpfilesHook, UserHook,
    };

    let platform = bin.platform.as_ref().map(|p| Platform {
        os: p.os.clone(),
        arch: p.arch.clone(),
        libc: p.libc.clone(),
        abi: p.abi.clone(),
    });

    let provides = Provides {
        capabilities: bin.provides.iter().map(|c| c.name.clone()).collect(),
        sonames: Vec::new(),
        binaries: Vec::new(),
        pkgconfig: Vec::new(),
    };

    let requires = Requires {
        capabilities: bin
            .requires
            .iter()
            .filter(|r| r.kind == "capability")
            .map(|r| {
                if let Some(ver) = &r.version {
                    Capability::Versioned {
                        name: r.name.clone(),
                        version: ver.clone(),
                    }
                } else {
                    Capability::Simple(r.name.clone())
                }
            })
            .collect(),
        packages: bin
            .requires
            .iter()
            .filter(|r| r.kind == "package")
            .map(|r| PackageDep {
                name: r.name.clone(),
                version: r.version.clone(),
            })
            .collect(),
    };

    let hooks = bin
        .hooks
        .as_ref()
        .map(|h| Hooks {
            users: h
                .users
                .iter()
                .map(|u| UserHook {
                    name: u.name.clone(),
                    system: u.system,
                    home: u.home.clone(),
                    shell: u.shell.clone(),
                    group: u.group.clone(),
                    reversible: u.reversible,
                })
                .collect(),
            groups: h
                .groups
                .iter()
                .map(|g| GroupHook {
                    name: g.name.clone(),
                    system: g.system,
                    reversible: g.reversible,
                })
                .collect(),
            directories: h
                .directories
                .iter()
                .map(|d| DirectoryHook {
                    path: d.path.clone(),
                    mode: format!("{:04o}", d.mode),
                    owner: d.owner.clone(),
                    group: d.group.clone(),
                    cleanup: None,
                    reversible: d.reversible,
                })
                .collect(),
            services: h
                .services
                .iter()
                .map(|s| Service {
                    name: s.name.clone(),
                    action: s.action.clone(),
                    reversible: s.reversible,
                })
                .collect(),
            systemd: h
                .systemd
                .iter()
                .map(|s| SystemdHook {
                    unit: s.unit.clone(),
                    enable: s.enable,
                    reversible: s.reversible,
                })
                .collect(),
            tmpfiles: h
                .tmpfiles
                .iter()
                .map(|t| TmpfilesHook {
                    entry_type: t.entry_type.clone(),
                    path: t.path.clone(),
                    mode: format!("{:04o}", t.mode),
                    owner: t.owner.clone(),
                    group: t.group.clone(),
                    reversible: t.reversible,
                })
                .collect(),
            sysctl: h
                .sysctl
                .iter()
                .map(|s| SysctlHook {
                    key: s.key.clone(),
                    value: s.value.clone(),
                    only_if_lower: s.only_if_lower,
                    reversible: s.reversible,
                })
                .collect(),
            alternatives: h
                .alternatives
                .iter()
                .map(|a| AlternativeHook {
                    name: a.name.clone(),
                    path: a.path.clone(),
                    priority: a.priority,
                    reversible: a.reversible,
                })
                .collect(),
            post_install: h.post_install.as_ref().map(|s| ScriptHook {
                script: s.clone(),
                reversible: h.post_install_reversible,
            }),
            pre_remove: h.pre_remove.as_ref().map(|s| ScriptHook {
                script: s.clone(),
                reversible: h.pre_remove_reversible,
            }),
        })
        .unwrap_or_default();

    let build = bin.build.as_ref().map(|b| BuildInfo {
        source: b.source.clone(),
        commit: b.commit.clone(),
        timestamp: b.timestamp.clone(),
        environment: std::collections::HashMap::new(),
        commands: Vec::new(),
        reproducible: b.reproducible,
    });

    CcsManifest {
        package: Package {
            name: bin.name.clone(),
            version: bin.version.clone(),
            description: bin.description.clone(),
            release: None,
            kind: None,
            license: bin.license.clone(),
            homepage: None,
            repository: None,
            platform,
            authors: None,
        },
        provides,
        requires,
        suggests: Suggests::default(),
        components: Components::default(),
        hooks,
        scriptlets: ScriptletDeclarations::default(),
        legacy_scriptlets: None,
        config: Config::default(),
        build,
        legacy: None,
        policy: BuildPolicyConfig::default(),
        provenance: None,
        capabilities: bin.capabilities.clone(),
        redirects: Redirects::default(),
    }
}

fn compatibility_manifest_from_v2(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
    build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
) -> Result<CcsManifest> {
    let mut manifest =
        CcsManifest::new_minimal(&authority.identity.name, &authority.identity.version);
    manifest.package.description = format!("CCS v2 {}", authority.identity.name);
    let provenance = manifest.provenance.get_or_insert_with(Default::default);
    provenance.origin_class = authority.provenance.origin_class.clone();
    provenance.hardening_level = authority.provenance.hardening_level.clone();
    provenance.build_attestation = build_attestation;
    provenance.foreign_conversion_boundary = foreign_conversion_boundary;
    Ok(manifest)
}

fn files_from_v2_authority(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
) -> Result<Vec<crate::ccs::builder::FileEntry>> {
    use crate::ccs::builder::{FileEntry, FileType};
    use crate::ccs::v2::schema::{FileTypeV2, PackageKindV2};

    let PackageKindV2::Package(data) = &authority.kind else {
        return Err(Error::ParseError(
            "group and redirect v2 packages are not installable in M4a".to_string(),
        ));
    };

    Ok(data
        .files
        .iter()
        .map(|file| FileEntry {
            path: file.path.clone(),
            hash: file.sha256.clone(),
            size: file.size,
            mode: file.mode,
            component: file.component.clone(),
            file_type: match file.file_type {
                FileTypeV2::Regular => FileType::Regular,
                FileTypeV2::Directory => FileType::Directory,
                FileTypeV2::Symlink => FileType::Symlink,
            },
            target: file.symlink_target.clone(),
            chunks: None,
        })
        .collect())
}

fn dependencies_from_v2_authority(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
) -> Vec<Dependency> {
    use crate::ccs::v2::schema::DependencyKindV2;

    authority
        .requires
        .iter()
        .filter_map(|dependency| match dependency.kind {
            DependencyKindV2::Package => Some(Dependency {
                name: dependency.name.clone(),
                version: dependency.version_constraint.clone(),
                dep_type: DependencyType::Runtime,
                description: None,
            }),
            DependencyKindV2::Capability => Some(Dependency {
                name: format!("capability:{}", dependency.name),
                version: dependency.version_constraint.clone(),
                dep_type: DependencyType::Runtime,
                description: None,
            }),
            _ => None,
        })
        .collect()
}

impl CcsPackage {
    fn validate_file_content(file: &FileEntry, content: &[u8]) -> Result<()> {
        let actual_size = u64::try_from(content.len()).map_err(|_| {
            Error::IoError(format!("File content too large to validate: {}", file.path))
        })?;
        if actual_size != file.size {
            if actual_size < file.size {
                return Err(Error::IoError(format!(
                    "File size mismatch (truncated) for {}: expected {} bytes, got {}",
                    file.path, file.size, actual_size
                )));
            }
            return Err(Error::IoError(format!(
                "File size mismatch for {}: expected {}, got {}",
                file.path, file.size, actual_size
            )));
        }

        let actual_hash = hash::sha256(content);
        if actual_hash != file.hash {
            return Err(Error::ChecksumMismatch {
                expected: file.hash.clone(),
                actual: actual_hash,
            });
        }

        Ok(())
    }

    /// Get the manifest
    pub fn manifest(&self) -> &CcsManifest {
        &self.manifest
    }

    /// Get the parsed binary manifest, when present
    pub fn binary_manifest(&self) -> Option<&BinaryManifest> {
        self.binary_manifest.as_ref()
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

    pub fn parse_verified_v2(
        path: &str,
        verification: &crate::ccs::verify::VerificationResult,
    ) -> Result<Self> {
        if !verification.valid
            || !matches!(
                &verification.content_status,
                crate::ccs::verify::ContentStatus::Valid { .. }
            )
        {
            return Err(Error::ParseError(
                "native CCS v2 package did not pass signature and payload verification".to_string(),
            ));
        }

        let package_path = PathBuf::from(path);
        let file = File::open(&package_path)?;
        let contents =
            read_ccs_archive(file).map_err(|error| Error::ParseError(error.to_string()))?;
        let Some(authority) = contents.v2_authority.as_ref() else {
            return <Self as PackageFormat>::parse(path);
        };
        let manifest = compatibility_manifest_from_v2(
            authority,
            contents.v2_build_attestation.clone(),
            contents.v2_foreign_conversion_boundary.clone(),
        )?;
        let files = files_from_v2_authority(authority)?;
        let dependencies = dependencies_from_v2_authority(authority);
        let package_files = Self::convert_files(&files);
        Ok(Self {
            package_path,
            manifest,
            binary_manifest: None,
            v2_authority: Some(authority.clone()),
            v2_build_attestation: contents.v2_build_attestation,
            v2_foreign_conversion_boundary: contents.v2_foreign_conversion_boundary,
            files,
            components: contents.components,
            package_files,
            dependencies,
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

    /// Convert CCS dependencies to trait dependencies
    fn convert_dependencies(manifest: &CcsManifest) -> Vec<Dependency> {
        let mut deps = Vec::new();

        // Add capability requirements
        for cap in &manifest.requires.capabilities {
            deps.push(Dependency {
                name: cap.name().to_string(),
                version: cap.version().map(|s| s.to_string()),
                dep_type: DependencyType::Runtime,
                description: None,
            });
        }

        // Add package fallback dependencies
        for pkg_dep in &manifest.requires.packages {
            deps.push(Dependency {
                name: pkg_dep.name.clone(),
                version: pkg_dep.version.clone(),
                dep_type: DependencyType::Runtime,
                description: None,
            });
        }

        deps
    }

    /// Convert CCS file entries to PackageFile list
    fn convert_files(files: &[FileEntry]) -> Vec<PackageFile> {
        files
            .iter()
            .filter(|f| f.file_type != CcsFileType::Directory)
            .map(|f| PackageFile {
                path: f.path.clone(),
                size: f.size as i64,
                mode: f.mode as i32,
                sha256: if f.file_type == CcsFileType::Symlink {
                    // For symlinks, compute the symlink hash
                    f.target.as_ref().map(|t| CasStore::compute_symlink_hash(t))
                } else {
                    Some(f.hash.clone())
                },
                symlink_target: None,
            })
            .collect()
    }

    /// Extract file contents from the package
    ///
    /// This re-reads the archive and returns the content blobs by hash.
    pub fn extract_all_content(&self) -> Result<HashMap<String, Vec<u8>>> {
        let file = File::open(&self.package_path)?;
        let contents = read_ccs_archive(file).map_err(|e| Error::IoError(e.to_string()))?;

        debug!(
            "Extracted {} content blobs from {}",
            contents.blobs.len(),
            self.package_path.display()
        );

        Ok(contents.blobs)
    }
}

#[cfg(test)]
impl CcsPackage {
    pub(crate) fn from_v2_authority_for_tests(
        authority: crate::ccs::v2::AuthorityDocumentV2,
        build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
        foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    ) -> Result<Self> {
        let manifest = compatibility_manifest_from_v2(
            &authority,
            build_attestation.clone(),
            foreign_conversion_boundary.clone(),
        )?;
        let files = files_from_v2_authority(&authority)?;
        let dependencies = dependencies_from_v2_authority(&authority);
        let package_files = Self::convert_files(&files);
        Ok(Self {
            package_path: PathBuf::from("v2-test.ccs"),
            manifest,
            binary_manifest: None,
            v2_authority: Some(authority),
            v2_build_attestation: build_attestation,
            v2_foreign_conversion_boundary: foreign_conversion_boundary,
            files,
            components: HashMap::new(),
            package_files,
            dependencies,
            config_files_cache: Vec::new(),
        })
    }
}

impl PackageFormat for CcsPackage {
    fn parse(path: &str) -> Result<Self>
    where
        Self: Sized,
    {
        let package_path = PathBuf::from(path);
        let file = File::open(&package_path)?;
        let contents = read_ccs_archive(file).map_err(|e| Error::ParseError(e.to_string()))?;

        if contents.v2_authority.is_some() {
            return Err(Error::ParseError(
                "native CCS v2 packages require verified parsing; call CcsPackage::parse_verified_v2"
                    .to_string(),
            ));
        }

        let manifest = &contents.manifest;

        // Log which manifest format was used
        if let Some(ref bin_manifest) = contents.binary_manifest {
            debug!(
                "Using CBOR manifest v{} for {} v{}",
                bin_manifest.format_version, bin_manifest.name, bin_manifest.version
            );
        } else {
            debug!(
                "Using TOML manifest (legacy) for {} v{}",
                manifest.package.name, manifest.package.version
            );
        }

        // Collect files from components (spec says files live in components/*.json)
        let files: Vec<FileEntry> = contents
            .components
            .values()
            .flat_map(|c| c.files.clone())
            .collect();

        // Pre-compute trait data
        let package_files = Self::convert_files(&files);
        let dependencies = Self::convert_dependencies(manifest);
        let config_files_cache: Vec<ConfigFileInfo> = manifest
            .config
            .files
            .iter()
            .map(|p| ConfigFileInfo {
                path: p.clone(),
                noreplace: manifest.config.noreplace,
                ghost: false,
            })
            .collect();

        debug!(
            "Parsed CCS package: {} v{} ({} files, {} deps, {} components)",
            manifest.package.name,
            manifest.package.version,
            files.len(),
            dependencies.len(),
            contents.components.len()
        );

        Ok(Self {
            package_path,
            manifest: contents.manifest,
            binary_manifest: contents.binary_manifest,
            v2_authority: None,
            v2_build_attestation: None,
            v2_foreign_conversion_boundary: None,
            files,
            components: contents.components,
            package_files,
            dependencies,
            config_files_cache,
        })
    }

    fn name(&self) -> &str {
        &self.manifest.package.name
    }

    fn version(&self) -> &str {
        &self.manifest.package.version
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

    fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>> {
        let blobs = self.extract_all_content()?;
        let mut extracted = Vec::with_capacity(self.files.len());

        for file in &self.files {
            // Skip directories - they're created automatically
            if file.file_type == CcsFileType::Directory {
                continue;
            }

            let symlink_target = if file.file_type == CcsFileType::Symlink {
                file.target.clone()
            } else {
                None
            };
            let content = if file.file_type == CcsFileType::Symlink {
                Vec::new()
            } else if let Some(chunk_hashes) = &file.chunks {
                // File is chunked - reassemble from chunks
                let mut reassembled = Vec::with_capacity(file.size as usize);
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
                let content = blobs.get(&file.hash).cloned().ok_or_else(|| {
                    crate::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "Content not found for file {} (hash: {})",
                            file.path, file.hash
                        ),
                    ))
                })?;
                Self::validate_file_content(file, &content)?;
                content
            };

            let sha256 = if file.file_type == CcsFileType::Symlink {
                file.target
                    .as_ref()
                    .map(|t| CasStore::compute_symlink_hash(t))
            } else {
                Some(file.hash.clone())
            };

            extracted.push(ExtractedFile {
                path: file.path.clone(),
                content,
                size: file.size as i64,
                mode: file.mode as i32,
                sha256,
                symlink_target,
            });
        }

        debug!("Extracted {} files from CCS package", extracted.len());

        Ok(extracted)
    }

    fn scriptlets(&self) -> &[Scriptlet] {
        // CCS uses declarative hooks, not scriptlets
        // Hooks are handled separately by HookExecutor
        &[]
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
            version_scheme: None,
            installed_from_repository_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::builder::{CcsBuilder, write_ccs_package};
    use flate2::Compression;
    use flate2::read::GzDecoder;
    use flate2::write::GzEncoder;
    use std::fs;
    use tar::{Archive, Builder};
    use tempfile::TempDir;

    fn build_test_package() -> (TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("src");
        fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
        fs::write(source_dir.join("usr/bin/hello"), b"hello world\n").unwrap();

        let manifest = CcsManifest::parse(
            r#"
[package]
name = "test-package"
version = "1.0.0"
description = "test fixture"
license = "MIT"
"#,
        )
        .unwrap();

        let result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
        let package_path = temp.path().join("test-package.ccs");
        write_ccs_package(&result, &package_path).unwrap();
        (temp, package_path)
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
description = "symlink fixture"
license = "MIT"
"#,
        )
        .unwrap();

        let result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
        let package_path = temp.path().join("symlink-package.ccs");
        write_ccs_package(&result, &package_path).unwrap();

        let package = CcsPackage::parse(package_path.to_str().unwrap()).unwrap();
        let files = package.extract_file_contents().unwrap();
        let sh = files
            .iter()
            .find(|file| file.path == "/usr/bin/sh")
            .expect("expected /usr/bin/sh symlink");

        assert_eq!(sh.symlink_target.as_deref(), Some("bash"));
    }

    #[test]
    fn test_extract_rejects_truncated_content() {
        let (_temp, package_path) = build_test_package();
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

        let package = CcsPackage::parse(corrupted_path.to_str().unwrap()).unwrap();
        let err = package.extract_file_contents().unwrap_err().to_string();
        assert!(
            err.contains("Checksum mismatch")
                || err.contains("File size mismatch")
                || err.contains("File truncated"),
            "{err}"
        );
    }

    #[test]
    fn test_extract_rejects_declared_size_mismatch() {
        use crate::ccs::binary_manifest::{BinaryManifest, Hash};

        let (_temp, package_path) = build_test_package();
        let corrupted_path = package_path.with_file_name("size-lie.ccs");
        mutate_package(&package_path, &corrupted_path, |root| {
            // Step 1: Mutate the component JSON (lie about file size)
            let component_path = root.join("components/runtime.json");
            let mut component: ComponentData =
                serde_json::from_slice(&fs::read(&component_path).unwrap()).unwrap();
            component.files[0].size = 1024;
            component.size = 1024;
            let new_component_bytes = serde_json::to_vec_pretty(&component).unwrap();
            fs::write(&component_path, &new_component_bytes).unwrap();

            // Step 2: Update the MANIFEST's component hash to match the
            // mutated JSON, so parsing succeeds and the size-mismatch
            // check is actually exercised during extraction.
            let manifest_path = root.join("MANIFEST");
            let manifest_bytes = fs::read(&manifest_path).unwrap();
            let mut manifest = BinaryManifest::from_cbor(&manifest_bytes).unwrap();
            if let Some(comp_ref) = manifest.components.get_mut("runtime") {
                comp_ref.hash = Hash::sha256(&new_component_bytes);
            }
            fs::write(&manifest_path, manifest.to_cbor().unwrap()).unwrap();
        });

        let package = CcsPackage::parse(corrupted_path.to_str().unwrap()).unwrap();
        let err = package.extract_file_contents().unwrap_err().to_string();
        assert!(
            err.contains("File size mismatch") || err.contains("File truncated"),
            "{err}"
        );
    }

    #[test]
    fn v2_packages_do_not_use_binary_manifest_default_reconstruction() {
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("adapter-v2");
        let package =
            CcsPackage::from_v2_authority_for_tests(authority.clone(), None, None).unwrap();
        assert_eq!(package.manifest().package.name, "adapter-v2");
        assert!(package.binary_manifest().is_none());
        assert!(package.v2_authority().is_some());
    }

    #[test]
    fn v2_compatibility_manifest_preserves_attestation_metadata() {
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
        assert!(plain_error.to_string().contains("verified parsing"));

        let mut verification = crate::ccs::verify::verify_package(
            &path,
            &crate::ccs::verify::TrustPolicy::strict(vec![key.public_key_base64()]),
        )
        .unwrap();
        let package = CcsPackage::parse_verified_v2(path.to_str().unwrap(), &verification).unwrap();
        assert!(package.v2_authority().is_some());

        verification.valid = false;
        let verified_error =
            CcsPackage::parse_verified_v2(path.to_str().unwrap(), &verification).unwrap_err();
        assert!(
            verified_error
                .to_string()
                .contains("did not pass signature and payload verification")
        );
    }
}
