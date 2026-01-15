// src/ccs/builder.rs
//! CCS package builder
//!
//! Builds .ccs packages from a manifest and source directory.
//! Handles file scanning, component classification, and package creation.

use crate::ccs::manifest::CcsManifest;
use crate::components::ComponentClassifier;
use crate::hash;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A file entry in a CCS package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Installation path (absolute)
    pub path: String,
    /// Content hash (SHA-256)
    pub hash: String,
    /// File size in bytes
    pub size: u64,
    /// Unix mode (permissions)
    pub mode: u32,
    /// Component assignment
    pub component: String,
    /// File type
    #[serde(rename = "type")]
    pub file_type: FileType,
    /// Symlink target (if type is symlink)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// File types in CCS packages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileType {
    Regular,
    Symlink,
    Directory,
}

/// Component data in a built package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentData {
    pub name: String,
    pub files: Vec<FileEntry>,
    /// Combined hash of all files in component (merkle-ish)
    pub hash: String,
    /// Total size of component
    pub size: u64,
}

/// Result of building a CCS package
#[derive(Debug)]
pub struct BuildResult {
    /// Package manifest (enhanced with auto-detected data)
    pub manifest: CcsManifest,
    /// Components with their files
    pub components: HashMap<String, ComponentData>,
    /// All file entries
    pub files: Vec<FileEntry>,
    /// Content blobs (hash -> content)
    pub blobs: HashMap<String, Vec<u8>>,
    /// Total package size
    pub total_size: u64,
}

/// CCS package builder
pub struct CcsBuilder {
    manifest: CcsManifest,
    source_dir: PathBuf,
    install_prefix: PathBuf,
    no_classify: bool,
}

impl CcsBuilder {
    /// Create a new builder
    pub fn new(manifest: CcsManifest, source_dir: &Path) -> Self {
        Self {
            manifest,
            source_dir: source_dir.to_path_buf(),
            install_prefix: PathBuf::from("/"),
            no_classify: false,
        }
    }

    /// Set the install prefix (default: /)
    pub fn with_install_prefix(mut self, prefix: &Path) -> Self {
        self.install_prefix = prefix.to_path_buf();
        self
    }

    /// Disable automatic component classification
    pub fn no_classify(mut self) -> Self {
        self.no_classify = true;
        self
    }

    /// Build the package
    pub fn build(&self) -> Result<BuildResult> {
        // Scan source directory for files
        let source_files = self.scan_source_files()?;

        // Process files: hash, classify, collect
        let mut files = Vec::new();
        let mut blobs = HashMap::new();
        let mut components: HashMap<String, Vec<FileEntry>> = HashMap::new();

        for source_path in source_files {
            let entry = self.process_file(&source_path, &mut blobs)?;

            components
                .entry(entry.component.clone())
                .or_default()
                .push(entry.clone());

            files.push(entry);
        }

        // Build component data
        let mut component_data = HashMap::new();
        for (name, comp_files) in components {
            let size: u64 = comp_files.iter().map(|f| f.size).sum();
            let hash = self.compute_component_hash(&comp_files);

            component_data.insert(
                name.clone(),
                ComponentData {
                    name,
                    files: comp_files,
                    hash,
                    size,
                },
            );
        }

        let total_size = files.iter().map(|f| f.size).sum();

        Ok(BuildResult {
            manifest: self.manifest.clone(),
            components: component_data,
            files,
            blobs,
            total_size,
        })
    }

    /// Scan source directory for all files
    fn scan_source_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        self.scan_dir_recursive(&self.source_dir, &mut files)?;
        files.sort();
        Ok(files)
    }

    fn scan_dir_recursive(&self, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;

            if metadata.is_dir() {
                self.scan_dir_recursive(&path, files)?;
            } else if metadata.is_file() || metadata.file_type().is_symlink() {
                files.push(path);
            }
        }

        Ok(())
    }

    /// Process a single file
    fn process_file(
        &self,
        source_path: &Path,
        blobs: &mut HashMap<String, Vec<u8>>,
    ) -> Result<FileEntry> {
        // Calculate install path
        let relative = source_path
            .strip_prefix(&self.source_dir)
            .context("File not under source directory")?;
        let install_path = self.install_prefix.join(relative);
        let install_path_str = install_path.to_string_lossy().to_string();

        // Get metadata
        let metadata = fs::symlink_metadata(source_path)?;
        let mode = metadata.permissions().mode();
        let is_symlink = metadata.file_type().is_symlink();

        // Determine file type and content
        let (file_type, hash, size, target) = if is_symlink {
            let target = fs::read_link(source_path)?;
            let target_str = target.to_string_lossy().to_string();
            let content = format!("symlink:{}", target_str);
            let hash = hash::sha256(content.as_bytes());
            blobs.insert(hash.clone(), content.into_bytes());
            (FileType::Symlink, hash, 0, Some(target_str))
        } else {
            let content = fs::read(source_path)?;
            let size = content.len() as u64;
            let hash = hash::sha256(&content);
            blobs.insert(hash.clone(), content);
            (FileType::Regular, hash, size, None)
        };

        // Classify component
        let component = self.classify_file(&install_path_str);

        Ok(FileEntry {
            path: install_path_str,
            hash,
            size,
            mode,
            component,
            file_type,
            target,
        })
    }

    /// Classify a file into a component
    fn classify_file(&self, path: &str) -> String {
        // First check exact file overrides from manifest
        if let Some(comp) = self.manifest.components.files.get(path) {
            return comp.clone();
        }

        // Check glob pattern overrides
        for override_rule in &self.manifest.components.overrides {
            if self.matches_glob(path, &override_rule.path) {
                return override_rule.component.clone();
            }
        }

        // If classification is disabled, default to runtime
        if self.no_classify {
            return "runtime".to_string();
        }

        // Auto-classify using ComponentClassifier
        let comp_type = ComponentClassifier::classify(Path::new(path));
        comp_type.as_str().to_string()
    }

    /// Simple glob matching (supports * and **)
    fn matches_glob(&self, path: &str, pattern: &str) -> bool {
        // Simple implementation - just check prefix and suffix
        if pattern.contains("**") {
            let parts: Vec<&str> = pattern.split("**").collect();
            if parts.len() == 2 {
                return path.starts_with(parts[0]) && path.ends_with(parts[1]);
            }
        } else if pattern.contains('*') {
            let parts: Vec<&str> = pattern.split('*').collect();
            if parts.len() == 2 {
                return path.starts_with(parts[0]) && path.ends_with(parts[1]);
            }
        }

        // Exact match
        path == pattern
    }

    /// Compute a combined hash for a component
    fn compute_component_hash(&self, files: &[FileEntry]) -> String {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        for file in files {
            hasher.update(file.path.as_bytes());
            hasher.update(b":");
            hasher.update(file.hash.as_bytes());
            hasher.update(b"\n");
        }
        format!("{:x}", hasher.finalize())
    }
}

/// Write a CCS package to disk
pub fn write_ccs_package(result: &BuildResult, output_path: &Path) -> Result<()> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use tar::Builder;

    // Create temp directory for package contents
    let temp_dir = tempfile::tempdir()?;

    // Write MANIFEST.toml (human-readable)
    let manifest_toml = result.manifest.to_toml()?;
    fs::write(temp_dir.path().join("MANIFEST.toml"), &manifest_toml)?;

    // Write FILES.json (file listing with hashes)
    let files_json = serde_json::to_string_pretty(&result.files)?;
    fs::write(temp_dir.path().join("FILES.json"), &files_json)?;

    // Write component metadata
    let components_dir = temp_dir.path().join("components");
    fs::create_dir_all(&components_dir)?;

    for (name, component) in &result.components {
        let component_json = serde_json::to_string_pretty(component)?;
        fs::write(components_dir.join(format!("{}.json", name)), &component_json)?;
    }

    // Write content blobs
    let objects_dir = temp_dir.path().join("objects");
    fs::create_dir_all(&objects_dir)?;

    for (hash, content) in &result.blobs {
        // Store as {first2}/{rest}
        let (prefix, suffix) = hash.split_at(2);
        let blob_dir = objects_dir.join(prefix);
        fs::create_dir_all(&blob_dir)?;
        fs::write(blob_dir.join(suffix), content)?;
    }

    // Create compressed tar archive
    let output_file = fs::File::create(output_path)?;
    let encoder = GzEncoder::new(output_file, Compression::default());
    let mut archive = Builder::new(encoder);

    // Add all files from temp directory
    archive.append_dir_all(".", temp_dir.path())?;

    let encoder = archive.into_inner()?;
    encoder.finish()?;

    Ok(())
}

/// Print build summary
pub fn print_build_summary(result: &BuildResult) {
    println!();
    println!("Build Summary");
    println!("=============");
    println!();
    println!("Package: {} v{}", result.manifest.package.name, result.manifest.package.version);
    println!("Total files: {}", result.files.len());
    println!("Total size: {} bytes", result.total_size);
    println!();
    println!("Components:");

    let mut comp_names: Vec<_> = result.components.keys().collect();
    comp_names.sort();

    for name in comp_names {
        let comp = &result.components[name];
        println!("  :{} - {} files ({} bytes)", name, comp.files.len(), comp.size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_manifest() -> CcsManifest {
        CcsManifest::new_minimal("test-package", "1.0.0")
    }

    #[test]
    fn test_builder_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let manifest = create_test_manifest();

        let builder = CcsBuilder::new(manifest, temp_dir.path());
        let result = builder.build().unwrap();

        assert!(result.files.is_empty());
        assert!(result.components.is_empty());
    }

    #[test]
    fn test_builder_single_file() {
        let temp_dir = TempDir::new().unwrap();

        // Create a test file
        let bin_dir = temp_dir.path().join("usr/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("myapp"), b"#!/bin/bash\necho hello").unwrap();

        let manifest = create_test_manifest();
        let builder = CcsBuilder::new(manifest, temp_dir.path());
        let result = builder.build().unwrap();

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "/usr/bin/myapp");
        assert_eq!(result.files[0].component, "runtime");
    }

    #[test]
    fn test_builder_classification() {
        let temp_dir = TempDir::new().unwrap();

        // Create files in different categories
        let dirs = [
            ("usr/bin", "myapp"),
            ("usr/lib", "libfoo.so.1"),
            ("usr/include", "foo.h"),
            ("etc/myapp", "config.conf"),
            ("usr/share/doc/myapp", "README"),
        ];

        for (dir, file) in &dirs {
            let path = temp_dir.path().join(dir);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join(file), b"test content").unwrap();
        }

        let manifest = create_test_manifest();
        let builder = CcsBuilder::new(manifest, temp_dir.path());
        let result = builder.build().unwrap();

        assert_eq!(result.files.len(), 5);

        // Check components
        assert!(result.components.contains_key("runtime"));
        assert!(result.components.contains_key("lib"));
        assert!(result.components.contains_key("devel"));
        assert!(result.components.contains_key("config"));
        assert!(result.components.contains_key("doc"));
    }

    #[test]
    fn test_builder_file_override() {
        let temp_dir = TempDir::new().unwrap();

        // Create a test file
        let bin_dir = temp_dir.path().join("usr/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("helper"), b"helper script").unwrap();

        // Create manifest with override
        let mut manifest = create_test_manifest();
        manifest.components.files.insert("/usr/bin/helper".to_string(), "lib".to_string());

        let builder = CcsBuilder::new(manifest, temp_dir.path());
        let result = builder.build().unwrap();

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].component, "lib"); // Should be overridden
    }

    #[test]
    fn test_builder_symlink() {
        let temp_dir = TempDir::new().unwrap();

        // Create a file and symlink
        let lib_dir = temp_dir.path().join("usr/lib");
        fs::create_dir_all(&lib_dir).unwrap();
        fs::write(lib_dir.join("libfoo.so.1.0.0"), b"library content").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink("libfoo.so.1.0.0", lib_dir.join("libfoo.so.1")).unwrap();

        let manifest = create_test_manifest();
        let builder = CcsBuilder::new(manifest, temp_dir.path());
        let result = builder.build().unwrap();

        #[cfg(unix)]
        {
            assert_eq!(result.files.len(), 2);

            let symlink = result.files.iter().find(|f| f.path == "/usr/lib/libfoo.so.1").unwrap();
            assert_eq!(symlink.file_type, FileType::Symlink);
            assert_eq!(symlink.target, Some("libfoo.so.1.0.0".to_string()));
        }
    }
}
