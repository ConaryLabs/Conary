// src/ccs/package.rs

//! CCS package parser implementing PackageFormat trait
//!
//! This module provides a PackageFormat implementation for CCS packages,
//! enabling them to be installed using the same infrastructure as RPM/DEB/Arch.

use crate::ccs::builder::{ComponentData, FileEntry, FileType as CcsFileType};
use crate::ccs::manifest::CcsManifest;
use crate::db::models::{InstallReason, InstallSource, Trove, TroveType};
use crate::error::{Error, Result};
use crate::filesystem::CasStore;
use crate::packages::traits::{
    ConfigFileInfo, Dependency, DependencyType, ExtractedFile, PackageFile, PackageFormat,
    Scriptlet,
};
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use tar::Archive;
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
}

impl CcsPackage {
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
            })
            .collect()
    }

    /// Extract file contents from the package
    ///
    /// This extracts the objects/ directory and maps content by hash.
    pub fn extract_all_content(&self) -> Result<HashMap<String, Vec<u8>>> {
        let file = File::open(&self.package_path)?;
        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);

        let mut blobs: HashMap<String, Vec<u8>> = HashMap::new();

        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_path = entry.path()?;
            let entry_path_str = entry_path.to_string_lossy();

            // Extract objects (format: objects/ab/cdef123...)
            if entry_path_str.starts_with("objects/") || entry_path_str.starts_with("./objects/") {
                let path_str = entry_path_str
                    .strip_prefix("./")
                    .unwrap_or(&entry_path_str)
                    .strip_prefix("objects/")
                    .unwrap_or("");

                // Reconstruct hash from path: ab/cdef123 -> abcdef123
                if let Some((prefix, suffix)) = path_str.split_once('/') {
                    let hash = format!("{}{}", prefix, suffix);
                    let mut content = Vec::new();
                    entry.read_to_end(&mut content)?;
                    blobs.insert(hash, content);
                }
            }
        }

        debug!(
            "Extracted {} content blobs from {}",
            blobs.len(),
            self.package_path.display()
        );

        Ok(blobs)
    }
}

impl PackageFormat for CcsPackage {
    fn parse(path: &str) -> Result<Self>
    where
        Self: Sized,
    {
        let package_path = PathBuf::from(path);
        let file = File::open(&package_path)?;
        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);

        let mut manifest: Option<CcsManifest> = None;
        let mut files: Option<Vec<FileEntry>> = None;
        let mut components: HashMap<String, ComponentData> = HashMap::new();

        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_path = entry.path()?;
            let entry_path_str = entry_path.to_string_lossy();

            // Read MANIFEST.toml
            if entry_path_str == "MANIFEST.toml" || entry_path_str == "./MANIFEST.toml" {
                let mut content = String::new();
                entry.read_to_string(&mut content)?;
                manifest = Some(
                    CcsManifest::parse(&content)
                        .map_err(|e| Error::ParseError(format!("Invalid MANIFEST.toml: {}", e)))?,
                );
            }
            // Read FILES.json
            else if entry_path_str == "FILES.json" || entry_path_str == "./FILES.json" {
                let mut content = String::new();
                entry.read_to_string(&mut content)?;
                files = Some(
                    serde_json::from_str(&content)
                        .map_err(|e| Error::ParseError(format!("Invalid FILES.json: {}", e)))?,
                );
            }
            // Read component files
            else if (entry_path_str.starts_with("components/")
                || entry_path_str.starts_with("./components/"))
                && entry_path_str.ends_with(".json")
            {
                let mut content = String::new();
                entry.read_to_string(&mut content)?;
                let comp: ComponentData = serde_json::from_str(&content)
                    .map_err(|e| Error::ParseError(format!("Invalid component JSON: {}", e)))?;
                components.insert(comp.name.clone(), comp);
            }
        }

        let manifest =
            manifest.ok_or_else(|| crate::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "CCS package missing MANIFEST.toml",
            )))?;
        let files = files.unwrap_or_default();

        // Pre-compute trait data
        let package_files = Self::convert_files(&files);
        let dependencies = Self::convert_dependencies(&manifest);

        debug!(
            "Parsed CCS package: {} v{} ({} files, {} deps)",
            manifest.package.name,
            manifest.package.version,
            files.len(),
            dependencies.len()
        );

        Ok(Self {
            package_path,
            manifest,
            files,
            components,
            package_files,
            dependencies,
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
            } else {
                // For regular files, look up by hash
                blobs
                    .get(&file.hash)
                    .cloned()
                    .ok_or_else(|| {
                        crate::Error::Io(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!(
                                "Content not found for file {} (hash: {})",
                                file.path, file.hash
                            ),
                        ))
                    })?
            };

            let sha256 = if file.file_type == CcsFileType::Symlink {
                file.target.as_ref().map(|t| CasStore::compute_symlink_hash(t))
            } else {
                Some(file.hash.clone())
            };

            extracted.push(ExtractedFile {
                path: file.path.clone(),
                content,
                size: file.size as i64,
                mode: file.mode as i32,
                sha256,
            });
        }

        debug!(
            "Extracted {} files from CCS package",
            extracted.len()
        );

        Ok(extracted)
    }

    fn scriptlets(&self) -> Vec<Scriptlet> {
        // CCS uses declarative hooks, not scriptlets
        // Hooks are handled separately by HookExecutor
        Vec::new()
    }

    fn config_files(&self) -> Vec<ConfigFileInfo> {
        self.manifest
            .config
            .files
            .iter()
            .map(|path| ConfigFileInfo {
                path: path.clone(),
                noreplace: self.manifest.config.noreplace,
                ghost: false,
            })
            .collect()
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symlink_hash_consistency() {
        // Verify we use consistent symlink hashing
        let target = "/usr/lib/libfoo.so.1";
        let hash = CasStore::compute_symlink_hash(target);
        assert_eq!(hash.len(), 64);
    }
}
