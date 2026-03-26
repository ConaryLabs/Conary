// conary-core/src/ccs/package.rs

//! CCS package parser implementing PackageFormat trait
//!
//! This module provides a PackageFormat implementation for CCS packages,
//! enabling them to be installed using the same infrastructure as RPM/DEB/Arch.

use crate::ccs::archive_reader::read_ccs_archive;
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
/// `suggests`, `components`, `config`, `legacy`, `policy`, `provenance`,
/// and `redirects`.
pub fn convert_binary_to_ccs_manifest(
    bin: &crate::ccs::binary_manifest::BinaryManifest,
) -> CcsManifest {
    use crate::ccs::manifest::{
        AlternativeHook, BuildInfo, Capability, Components, Config, DirectoryHook, GroupHook,
        Hooks, Package, PackageDep, Platform, Provides, Requires, ScriptHook, Suggests, SysctlHook,
        SystemdHook, TmpfilesHook, UserHook,
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
                })
                .collect(),
            groups: h
                .groups
                .iter()
                .map(|g| GroupHook {
                    name: g.name.clone(),
                    system: g.system,
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
                })
                .collect(),
            services: Vec::new(), // Not yet supported in binary format
            systemd: h
                .systemd
                .iter()
                .map(|s| SystemdHook {
                    unit: s.unit.clone(),
                    enable: s.enable,
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
                })
                .collect(),
            sysctl: h
                .sysctl
                .iter()
                .map(|s| SysctlHook {
                    key: s.key.clone(),
                    value: s.value.clone(),
                    only_if_lower: s.only_if_lower,
                })
                .collect(),
            alternatives: h
                .alternatives
                .iter()
                .map(|a| AlternativeHook {
                    name: a.name.clone(),
                    path: a.path.clone(),
                    priority: a.priority,
                })
                .collect(),
            post_install: h
                .post_install
                .as_ref()
                .map(|s| ScriptHook { script: s.clone() }),
            pre_remove: h
                .pre_remove
                .as_ref()
                .map(|s| ScriptHook { script: s.clone() }),
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
        config: Config::default(),
        build,
        legacy: None,
        policy: BuildPolicyConfig::default(),
        provenance: None,
        capabilities: bin.capabilities.clone(),
        redirects: Redirects::default(),
    }
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

impl PackageFormat for CcsPackage {
    fn parse(path: &str) -> Result<Self>
    where
        Self: Sized,
    {
        let package_path = PathBuf::from(path);
        let file = File::open(&package_path)?;
        let contents = read_ccs_archive(file).map_err(|e| Error::ParseError(e.to_string()))?;

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

            let content = if file.file_type == CcsFileType::Symlink {
                // For symlinks, content is the target path
                file.target
                    .as_ref()
                    .map(|t| t.as_bytes().to_vec())
                    .unwrap_or_default()
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
                symlink_target: None,
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
}
