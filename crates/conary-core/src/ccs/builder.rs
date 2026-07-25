// conary-core/src/ccs/builder.rs
//! CCS package builder
//!
//! Builds .ccs packages from a manifest and source directory.
//! Handles file scanning, explicit component assignment, and package creation.
//! Supports Content-Defined Chunking (CDC) for efficient delta updates.

use crate::ccs::chunking::{Chunker, MIN_CHUNK_SIZE};
use crate::ccs::manifest::CcsManifest;
use crate::ccs::policy::{PolicyAction, PolicyChain};
use crate::hash;
use crate::packages::traits::ExtractedFile;
use crate::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

mod package_writer;

pub use package_writer::{
    print_build_summary, write_signed_current_ccs_package, write_v2_ccs_package,
};

#[cfg(test)]
pub(crate) mod test_support;

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

    /// I/O error during build (file read/write, directory creation)
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// A file entry in a CCS package
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    /// Installation path (absolute)
    pub path: String,
    /// Exact POSIX node authority.
    pub node: PayloadNode,
    /// Content authority, present only for regular files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<PayloadContentAuthority>,
    /// Component assignment
    pub component: String,
    /// Chunk hashes for CDC (if file is chunked)
    /// When present, the file content is stored as chunks instead of a single blob.
    /// Chunks are stored by their hash in objects/ and must be concatenated in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<String>>,
}

/// Component data in a built package
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Package manifest with exact author and source-package metadata.
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
    policy_chain: Option<PolicyChain>,
    /// Enable CDC chunking for delta-efficient packages
    use_chunking: bool,
    /// Chunker instance (created lazily if chunking is enabled)
    chunker: Option<Chunker>,
    /// Exact source-package payload entries. When present, this is the
    /// authority for explicit directories and entry kinds; the temporary
    /// materialization tree is not rescanned for inferred parent directories.
    package_entries: Option<Vec<ExtractedFile>>,
}

fn is_suspicious_component_executable(component: &str, node: &PayloadNode) -> bool {
    node.kind.is_regular()
        && node.mode & 0o111 != 0
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
            policy_chain,
            use_chunking: false,
            chunker: None,
            package_entries: None,
        }
    }

    /// Set the install prefix (default: /)
    pub fn with_install_prefix(mut self, prefix: &Path) -> Self {
        self.install_prefix = prefix.to_path_buf();
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

    /// Build from exact source-package payload entries instead of inferring
    /// package structure from the materialized source tree.
    pub fn with_package_entries(mut self, entries: &[ExtractedFile]) -> Self {
        self.package_entries = Some(entries.to_vec());
        self
    }

    /// Build the package
    pub fn build(&self) -> Result<BuildResult> {
        self.validate_component_contract()?;
        // Phase 1: Collect files with content (before policies)
        let mut raw_files: Vec<(PathBuf, FileEntry, Vec<u8>)> = Vec::new();
        if let Some(package_entries) = &self.package_entries {
            for package_entry in package_entries {
                let source_path =
                    crate::filesystem::safe_join(&self.source_dir, &package_entry.path)?;
                let (entry, content) = self.collect_package_entry(package_entry)?;
                raw_files.push((source_path, entry, content));
            }
        } else {
            let mut hardlink_anchors = HashMap::new();
            for source_path in self.scan_source_files()? {
                let (entry, content) = self.collect_file(&source_path, &mut hardlink_anchors)?;
                raw_files.push((source_path, entry, content));
            }
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
                        let content_authority = entry.content.as_mut().ok_or_else(|| {
                            anyhow::anyhow!(
                                "policy attempted to replace non-regular payload node {}",
                                entry.path
                            )
                        })?;
                        content_authority.sha256 = hash::sha256(&new_content);
                        content_authority.size = new_content.len() as u64;
                        new_content
                    }
                }
            } else {
                content
            };

            if is_suspicious_component_executable(&entry.component, &entry.node) {
                tracing::warn!(
                    "Suspicious executable file {} assigned to '{}' component",
                    entry.path,
                    entry.component
                );
            }

            // Phase 3: Store content (chunked or whole)
            if !entry.node.kind.is_regular() {
                if !final_content.is_empty() {
                    anyhow::bail!(
                        "non-regular payload node {} carries payload bytes",
                        entry.path
                    );
                }
            } else if self.use_chunking && self.should_chunk(&entry, &final_content) {
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
                let content_authority = entry.content.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "regular payload node {} has no content authority",
                        entry.path
                    )
                })?;
                blobs.insert(content_authority.sha256.clone(), final_content);
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
            let size: u64 = comp_files
                .iter()
                .filter_map(|file| file.content.as_ref().map(|content| content.size))
                .sum();
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

        let total_size = files
            .iter()
            .filter_map(|file| file.content.as_ref().map(|content| content.size))
            .sum();

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
        if !entry.node.kind.is_regular() {
            return false;
        }

        // Only chunk files larger than minimum chunk size
        // (smaller files wouldn't benefit from CDC)
        content.len() >= MIN_CHUNK_SIZE as usize
    }

    /// Collect a file's metadata and content without hashing
    fn collect_file(
        &self,
        source_path: &Path,
        hardlink_anchors: &mut HashMap<(u64, u64), (String, String)>,
    ) -> Result<(FileEntry, Vec<u8>)> {
        // Calculate install path
        let relative = source_path
            .strip_prefix(&self.source_dir)
            .map_err(|_| BuilderError::FileNotUnderSource(source_path.to_path_buf()))?;
        let install_path = self.install_prefix.join(relative);
        let install_path_str = install_path.to_string_lossy().to_string();

        // Get metadata
        let metadata = fs::symlink_metadata(source_path)?;
        let source_kind = metadata.file_type();
        let xattrs = read_payload_xattrs(source_path)?;

        // Determine file type and content from the filesystem's typed entry
        // metadata, never from mode bits or target presence.
        let (kind, content) = if source_kind.is_symlink() {
            let target = fs::read_link(source_path)?;
            let target = target.into_os_string().into_string().map_err(|_| {
                anyhow::anyhow!(
                    "payload symlink {} has a non-UTF-8 target",
                    source_path.display()
                )
            })?;
            (PayloadNodeKind::Symlink { target }, Vec::new())
        } else if source_kind.is_dir() {
            (PayloadNodeKind::Directory, Vec::new())
        } else if source_kind.is_file() {
            let hardlink_key = (metadata.dev(), metadata.ino());
            if metadata.nlink() > 1
                && let Some((target, identity)) = hardlink_anchors.get(&hardlink_key)
            {
                (
                    PayloadNodeKind::Hardlink {
                        target: target.clone(),
                        identity: identity.clone(),
                    },
                    Vec::new(),
                )
            } else {
                let hardlink_identity = (metadata.nlink() > 1).then(|| {
                    let identity = format!("path:{install_path_str}");
                    hardlink_anchors
                        .insert(hardlink_key, (install_path_str.clone(), identity.clone()));
                    identity
                });
                (
                    PayloadNodeKind::Regular { hardlink_identity },
                    fs::read(source_path)?,
                )
            }
        } else if source_kind.is_block_device() {
            let (major, minor) = payload_device_numbers(metadata.rdev());
            (PayloadNodeKind::BlockDevice { major, minor }, Vec::new())
        } else if source_kind.is_char_device() {
            let (major, minor) = payload_device_numbers(metadata.rdev());
            (
                PayloadNodeKind::CharacterDevice { major, minor },
                Vec::new(),
            )
        } else if source_kind.is_fifo() {
            (PayloadNodeKind::Fifo, Vec::new())
        } else if source_kind.is_socket() {
            (PayloadNodeKind::Socket, Vec::new())
        } else {
            anyhow::bail!(
                "unsupported source payload entry kind at {}",
                source_path.display()
            );
        };

        let content_authority = kind.is_regular().then(|| PayloadContentAuthority {
            sha256: hash::sha256(&content),
            size: content.len() as u64,
        });
        let node = PayloadNode {
            kind,
            mode: metadata.mode(),
            user: PayloadIdentity::Numeric {
                id: u64::from(metadata.uid()),
            },
            group: PayloadIdentity::Numeric {
                id: u64::from(metadata.gid()),
            },
            mtime: PayloadTimestamp {
                seconds: metadata.mtime(),
                nanoseconds: u32::try_from(metadata.mtime_nsec()).map_err(|_| {
                    anyhow::anyhow!(
                        "payload entry {} has invalid negative mtime nanoseconds",
                        source_path.display()
                    )
                })?,
            },
            xattrs,
        };
        node.validate()?;
        node.validate_content(content_authority.as_ref())?;

        // Apply exact component assignment.
        let component = self.component_for_path(&install_path_str)?;

        let entry = FileEntry {
            path: install_path_str,
            node,
            content: content_authority,
            component,
            chunks: None, // Set during build if chunking is enabled
        };

        Ok((entry, content))
    }

    fn collect_package_entry(&self, file: &ExtractedFile) -> Result<(FileEntry, Vec<u8>)> {
        let relative = Path::new(&file.path)
            .strip_prefix("/")
            .unwrap_or_else(|_| Path::new(&file.path));
        let install_path = self.install_prefix.join(relative);
        let install_path_str = install_path.to_string_lossy().to_string();
        file.node.validate()?;
        file.node
            .validate_content(file.content_authority.as_ref())?;
        let content = if file.node.kind.is_regular() {
            let authority = file.content_authority.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "regular payload entry {} has no content authority",
                    file.path
                )
            })?;
            if authority.size != file.content.len() as u64
                || authority.sha256 != hash::sha256(&file.content)
            {
                anyhow::bail!(
                    "regular payload entry {} content does not match its exact authority",
                    file.path
                );
            }
            file.content.clone()
        } else {
            if !file.content.is_empty() {
                anyhow::bail!("non-regular payload entry {} carries content", file.path);
            }
            Vec::new()
        };

        let component = self.component_for_path(&install_path_str)?;
        Ok((
            FileEntry {
                path: install_path_str,
                node: file.node.clone(),
                content: file.content_authority.clone(),
                component,
                chunks: None,
            },
            content,
        ))
    }

    /// Scan source directory for all explicit entries.
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

            files.push(path.to_path_buf());
        }

        files.sort();
        Ok(files)
    }

    /// Resolve a file's explicit author-owned component assignment.
    ///
    /// Exact path declarations take precedence over author-declared glob
    /// grammar. A path without either declaration belongs to the single
    /// lossless runtime component; Conary never guesses from its filename.
    fn component_for_path(&self, path: &str) -> Result<String> {
        if let Some(comp) = self.manifest.components.files.get(path) {
            return Ok(comp.clone());
        }

        let mut matched_component: Option<&str> = None;
        for rule in &self.manifest.components.rules {
            let pattern = glob::Pattern::new(&rule.path).map_err(|error| {
                anyhow::anyhow!("invalid components.rules pattern '{}': {error}", rule.path)
            })?;
            if !pattern.matches(path) {
                continue;
            }
            if let Some(existing) = matched_component
                && existing != rule.component
            {
                anyhow::bail!(
                    "component assignment for {path} is ambiguous: author rules select both '{existing}' and '{}'",
                    rule.component
                );
            }
            matched_component = Some(&rule.component);
        }

        Ok(matched_component.unwrap_or("runtime").to_string())
    }

    fn validate_component_contract(&self) -> Result<()> {
        for (path, component) in &self.manifest.components.files {
            if component.trim().is_empty() {
                anyhow::bail!("components.files entry for {path} has an empty component name");
            }
        }
        for rule in &self.manifest.components.rules {
            glob::Pattern::new(&rule.path).map_err(|error| {
                anyhow::anyhow!("invalid components.rules pattern '{}': {error}", rule.path)
            })?;
            if rule.component.trim().is_empty() {
                anyhow::bail!(
                    "components.rules pattern '{}' has an empty component name",
                    rule.path
                );
            }
        }
        Ok(())
    }

    /// Compute a combined hash for a component
    fn compute_component_hash(&self, files: &[FileEntry]) -> String {
        let mut hasher = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
        for file in files {
            hasher.update(
                &serde_json::to_vec(file)
                    .expect("FileEntry contains only infallibly serializable authority"),
            );
        }
        hasher.finalize().value
    }
}

fn read_payload_xattrs(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut attributes = BTreeMap::new();
    for name in xattr::list(path)? {
        let name = name.into_string().map_err(|_| {
            anyhow::anyhow!(
                "payload entry {} has a non-UTF-8 xattr name",
                path.display()
            )
        })?;
        let value = xattr::get(path, &name)?.ok_or_else(|| {
            anyhow::anyhow!(
                "payload xattr {} disappeared while reading {}",
                name,
                path.display()
            )
        })?;
        attributes.insert(name, value);
    }
    Ok(attributes)
}

fn payload_device_numbers(device: u64) -> (u64, u64) {
    (nix::sys::stat::major(device), nix::sys::stat::minor(device))
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
    fn payload_authority_types_reject_unknown_fields() {
        let file = FileEntry {
            path: "/usr/bin/example".to_string(),
            node: PayloadNode::regular(0o755),
            content: None,
            component: "runtime".to_string(),
            chunks: None,
        };
        let mut file_value = serde_json::to_value(&file).unwrap();
        file_value
            .as_object_mut()
            .unwrap()
            .insert("invented_authority".to_string(), serde_json::json!(true));
        assert!(serde_json::from_value::<FileEntry>(file_value).is_err());

        let component = ComponentData {
            name: "runtime".to_string(),
            files: vec![file],
            hash: "sha256:test".to_string(),
            size: 0,
        };
        let mut component_value = serde_json::to_value(&component).unwrap();
        component_value
            .as_object_mut()
            .unwrap()
            .insert("invented_authority".to_string(), serde_json::json!(true));
        assert!(serde_json::from_value::<ComponentData>(component_value).is_err());
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

        let files = result
            .files
            .iter()
            .filter(|entry| entry.node.kind.is_regular())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/usr/bin/myapp");
        assert_eq!(files[0].component, "runtime");
    }

    #[test]
    fn builder_does_not_infer_components_from_payload_paths() {
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

        let payload_files = result
            .files
            .iter()
            .filter(|entry| entry.node.kind.is_regular())
            .collect::<Vec<_>>();
        assert_eq!(payload_files.len(), 5);
        assert!(
            payload_files
                .iter()
                .all(|entry| entry.component == "runtime")
        );
        assert_eq!(result.components.len(), 1);
        assert!(result.components.contains_key("runtime"));
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

        let helper = result
            .files
            .iter()
            .find(|entry| entry.path == "/usr/bin/helper")
            .unwrap();
        assert_eq!(helper.component, "lib");
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
            let symlink = result
                .files
                .iter()
                .find(|f| f.path == "/usr/lib/libfoo.so.1")
                .unwrap();
            assert_eq!(
                symlink.node.kind,
                PayloadNodeKind::Symlink {
                    target: "libfoo.so.1.0.0".to_string()
                }
            );
        }
    }

    #[test]
    fn test_suspicious_executable_component_audit_targets_doc_config_and_data() {
        let executable = PayloadNode::regular(0o755);
        for component in ["doc", "config", "data"] {
            assert!(
                is_suspicious_component_executable(component, &executable),
                "{component} executables should be flagged as suspicious"
            );
        }

        assert!(!is_suspicious_component_executable("runtime", &executable));
        assert!(!is_suspicious_component_executable(
            "doc",
            &PayloadNode::regular(0o644)
        ));
        let mut symlink = PayloadNode::regular(0o755);
        symlink.kind = PayloadNodeKind::Symlink {
            target: "target".to_string(),
        };
        symlink.mode = libc::S_IFLNK | 0o755;
        assert!(!is_suspicious_component_executable("doc", &symlink));
    }
}
