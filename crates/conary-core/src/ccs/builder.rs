// conary-core/src/ccs/builder.rs
//! CCS package builder
//!
//! Builds .ccs packages from a manifest and source directory.
//! Handles file scanning, component classification, and package creation.
//! Supports Content-Defined Chunking (CDC) for efficient delta updates.

use crate::ccs::chunking::{Chunker, MIN_CHUNK_SIZE};
use crate::ccs::manifest::CcsManifest;
use crate::ccs::policy::{PolicyAction, PolicyChain};
use crate::components::ComponentClassifier;
use crate::filesystem::CasStore;
use crate::hash;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

mod package_writer;

pub use package_writer::{print_build_summary, write_ccs_package, write_signed_ccs_package};

/// Typed errors for the CCS builder pipeline
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    /// File is not under the expected source directory
    #[error("file not under source directory: {0}")]
    FileNotUnderSource(PathBuf),

    /// A build policy rejected a file
    #[error("policy rejected file {path}: {reason}")]
    PolicyRejected { path: String, reason: String },

    /// Chunker was expected but not initialized
    #[error("chunker not initialized even though chunking is enabled")]
    ChunkerNotInitialized,

    /// CBOR encoding of the binary manifest failed
    #[error("failed to encode binary manifest as CBOR: {0}")]
    ManifestEncoding(String),

    /// I/O error during build (file read/write, directory creation)
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// A file entry in a CCS package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Installation path (absolute)
    pub path: String,
    /// Content hash (SHA-256 of full file content)
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
    /// Chunk hashes for CDC (if file is chunked)
    /// When present, the file content is stored as chunks instead of a single blob.
    /// Chunks are stored by their hash in objects/ and must be concatenated in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<String>>,
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
    /// With CDC enabled, this contains chunks; without CDC, it contains whole files.
    pub blobs: HashMap<String, Vec<u8>>,
    /// Total package size
    pub total_size: u64,
    /// Whether CDC chunking was used
    pub chunked: bool,
    /// CDC statistics (if chunking was used)
    pub chunk_stats: Option<ChunkStats>,
}

/// Statistics about CDC chunking in a build
#[derive(Debug, Clone, Default)]
pub struct ChunkStats {
    /// Number of files that were chunked
    pub chunked_files: usize,
    /// Number of files stored as whole blobs (too small to chunk)
    pub whole_files: usize,
    /// Total number of chunks created
    pub total_chunks: usize,
    /// Number of unique chunks (after dedup within package)
    pub unique_chunks: usize,
    /// Bytes saved by intra-package deduplication
    pub dedup_savings: u64,
}

/// CCS package builder
pub struct CcsBuilder {
    manifest: CcsManifest,
    source_dir: PathBuf,
    install_prefix: PathBuf,
    no_classify: bool,
    policy_chain: Option<PolicyChain>,
    /// Enable CDC chunking for delta-efficient packages
    use_chunking: bool,
    /// Chunker instance (created lazily if chunking is enabled)
    chunker: Option<Chunker>,
}

fn is_suspicious_component_executable(component: &str, mode: u32, file_type: FileType) -> bool {
    file_type == FileType::Regular
        && mode & 0o111 != 0
        && matches!(component, "doc" | "config" | "data")
}

impl CcsBuilder {
    /// Create a new builder
    pub fn new(manifest: CcsManifest, source_dir: &Path) -> Self {
        // Create policy chain from manifest configuration
        let policy_chain = PolicyChain::from_config(&manifest.policy).ok();

        Self {
            manifest,
            source_dir: source_dir.to_path_buf(),
            install_prefix: PathBuf::from("/"),
            no_classify: false,
            policy_chain,
            use_chunking: false,
            chunker: None,
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

    /// Set custom policy chain (overrides manifest config)
    pub fn with_policies(mut self, chain: PolicyChain) -> Self {
        self.policy_chain = Some(chain);
        self
    }

    /// Enable CDC chunking for delta-efficient packages
    ///
    /// When enabled, large files are split into content-defined chunks.
    /// This enables efficient delta updates - clients only download
    /// changed chunks instead of entire files.
    pub fn with_chunking(mut self) -> Self {
        self.use_chunking = true;
        self.chunker = Some(Chunker::new());
        self
    }

    /// Build the package
    pub fn build(&self) -> Result<BuildResult> {
        // Scan source directory for files
        let source_files = self.scan_source_files()?;

        // Phase 1: Collect files with content (before policies)
        let mut raw_files: Vec<(PathBuf, FileEntry, Vec<u8>)> = Vec::new();

        for source_path in source_files {
            let (entry, content) = self.collect_file(&source_path)?;
            raw_files.push((source_path, entry, content));
        }

        // Phase 2: Apply policies and optionally chunk files
        let mut files = Vec::new();
        let mut blobs = HashMap::new();
        let mut components: HashMap<String, Vec<FileEntry>> = HashMap::new();
        let mut chunk_stats = ChunkStats::default();

        for (source_path, mut entry, content) in raw_files {
            // Apply policy chain if configured
            let final_content = if let Some(ref chain) = self.policy_chain {
                let (action, new_content) =
                    chain.apply(&mut entry, content, &source_path, &self.manifest.policy)?;

                match action {
                    PolicyAction::Skip => {
                        // Policy says skip this file
                        continue;
                    }
                    PolicyAction::Reject(msg) => {
                        return Err(BuilderError::PolicyRejected {
                            path: entry.path.clone(),
                            reason: msg,
                        }
                        .into());
                    }
                    PolicyAction::Keep => {
                        // Content unchanged by policy, no rehash needed
                        new_content
                    }
                    PolicyAction::Replace(_) => {
                        // Content was modified by policy, recompute hash
                        let new_hash = hash::sha256(&new_content);
                        if new_hash != entry.hash {
                            entry.hash = new_hash;
                            entry.size = new_content.len() as u64;
                        }
                        new_content
                    }
                }
            } else {
                content
            };

            if is_suspicious_component_executable(&entry.component, entry.mode, entry.file_type) {
                tracing::warn!(
                    "Suspicious executable file {} classified into '{}' component",
                    entry.path,
                    entry.component
                );
            }

            // Phase 3: Store content (chunked or whole)
            if self.use_chunking && self.should_chunk(&entry, &final_content) {
                // Chunk the file
                let chunker = self
                    .chunker
                    .as_ref()
                    .ok_or(BuilderError::ChunkerNotInitialized)?;
                let chunks = chunker.chunk_bytes(&final_content);

                let mut chunk_hashes = Vec::with_capacity(chunks.len());
                for chunk in &chunks {
                    let chunk_hash = hex::encode(chunk.hash);
                    chunk_hashes.push(chunk_hash.clone());
                    chunk_stats.total_chunks += 1;

                    // Store chunk (may deduplicate)
                    use std::collections::hash_map::Entry;
                    match blobs.entry(chunk_hash) {
                        Entry::Vacant(e) => {
                            e.insert(chunk.data.clone());
                            chunk_stats.unique_chunks += 1;
                        }
                        Entry::Occupied(_) => {
                            chunk_stats.dedup_savings += chunk.length as u64;
                        }
                    }
                }

                entry.chunks = Some(chunk_hashes);
                chunk_stats.chunked_files += 1;
            } else {
                // Store as whole blob
                blobs.insert(entry.hash.clone(), final_content);
                chunk_stats.whole_files += 1;
            }

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
            chunked: self.use_chunking,
            chunk_stats: if self.use_chunking {
                Some(chunk_stats)
            } else {
                None
            },
        })
    }

    /// Determine if a file should be chunked
    ///
    /// Files are chunked if:
    /// - They are regular files (not symlinks/directories)
    /// - They are larger than the minimum chunk size (16KB)
    fn should_chunk(&self, entry: &FileEntry, content: &[u8]) -> bool {
        // Only chunk regular files
        if entry.file_type != FileType::Regular {
            return false;
        }

        // Only chunk files larger than minimum chunk size
        // (smaller files wouldn't benefit from CDC)
        content.len() >= MIN_CHUNK_SIZE as usize
    }

    /// Collect a file's metadata and content without hashing
    fn collect_file(&self, source_path: &Path) -> Result<(FileEntry, Vec<u8>)> {
        // Calculate install path
        let relative = source_path
            .strip_prefix(&self.source_dir)
            .map_err(|_| BuilderError::FileNotUnderSource(source_path.to_path_buf()))?;
        let install_path = self.install_prefix.join(relative);
        let install_path_str = install_path.to_string_lossy().to_string();

        // Get metadata
        let metadata = fs::symlink_metadata(source_path)?;
        let mode = metadata.permissions().mode();
        let is_symlink = metadata.file_type().is_symlink();

        // Determine file type and content
        let (file_type, content, target) = if is_symlink {
            let target = fs::read_link(source_path)?;
            let target_str = target.to_string_lossy().to_string();
            (
                FileType::Symlink,
                target_str.as_bytes().to_vec(),
                Some(target_str),
            )
        } else {
            let content = fs::read(source_path)?;
            (FileType::Regular, content, None)
        };

        // Compute initial hash.
        // For symlinks use CasStore::compute_symlink_hash so the hash stored in
        // the FileEntry matches what CasStore::store_symlink/retrieve_symlink
        // produce (both are sha256 of the raw target bytes, but using the
        // canonical helper makes the invariant explicit and guards against
        // future divergence).
        let hash_val = if let Some(ref target_str) = target {
            CasStore::compute_symlink_hash(target_str)
        } else {
            hash::sha256(&content)
        };
        let size = content.len() as u64;

        // Classify component
        let component = self.classify_file(&install_path_str);

        let entry = FileEntry {
            path: install_path_str,
            hash: hash_val,
            size,
            mode,
            component,
            file_type,
            target,
            chunks: None, // Set during build if chunking is enabled
        };

        Ok((entry, content))
    }

    /// Scan source directory for all files
    fn scan_source_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        for entry in WalkDir::new(&self.source_dir)
            .into_iter()
            .filter_map(|e| match e {
                Ok(entry) => Some(entry),
                Err(err) => {
                    tracing::warn!("WalkDir error scanning source directory: {err}");
                    None
                }
            })
        {
            let path = entry.path();
            let metadata = entry.metadata()?;

            // Skip the source directory itself
            if path == self.source_dir {
                continue;
            }

            // Skip manifest files - these are metadata, not package content
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str())
                && (file_name == "ccs.toml" || file_name == "MANIFEST.toml")
            {
                continue;
            }

            // We only collect files and symlinks (directories are handled by their children)
            if metadata.is_file() || metadata.file_type().is_symlink() {
                files.push(path.to_path_buf());
            }
        }

        files.sort();
        Ok(files)
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

    /// Check if path matches glob pattern using glob crate
    fn matches_glob(&self, path: &str, pattern: &str) -> bool {
        use glob::Pattern;

        match Pattern::new(pattern) {
            Ok(compiled) => compiled.matches(path),
            Err(e) => {
                tracing::warn!("Invalid glob pattern '{}': {}", pattern, e);
                false
            }
        }
    }

    /// Compute a combined hash for a component
    fn compute_component_hash(&self, files: &[FileEntry]) -> String {
        let mut hasher = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
        for file in files {
            hasher.update(file.path.as_bytes());
            hasher.update(b":");
            hasher.update(file.hash.as_bytes());
            hasher.update(b"\n");
        }
        hasher.finalize().value
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
        manifest
            .components
            .files
            .insert("/usr/bin/helper".to_string(), "lib".to_string());

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

            let symlink = result
                .files
                .iter()
                .find(|f| f.path == "/usr/lib/libfoo.so.1")
                .unwrap();
            assert_eq!(symlink.file_type, FileType::Symlink);
            assert_eq!(symlink.target, Some("libfoo.so.1.0.0".to_string()));
        }
    }

    #[test]
    fn test_suspicious_executable_component_audit_targets_doc_config_and_data() {
        for component in ["doc", "config", "data"] {
            assert!(
                is_suspicious_component_executable(component, 0o755, FileType::Regular),
                "{component} executables should be flagged as suspicious"
            );
        }

        assert!(!is_suspicious_component_executable(
            "runtime",
            0o755,
            FileType::Regular
        ));
        assert!(!is_suspicious_component_executable(
            "doc",
            0o644,
            FileType::Regular
        ));
        assert!(!is_suspicious_component_executable(
            "doc",
            0o755,
            FileType::Symlink
        ));
    }
}
