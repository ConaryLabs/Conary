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

        for entry in WalkDir::new(&self.source_dir).into_iter().filter_map(|e| {
            match e {
                Ok(entry) => Some(entry),
                Err(err) => {
                    tracing::warn!("WalkDir error scanning source directory: {err}");
                    None
                }
            }
        }) {
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

/// Write a CCS package to disk (unsigned)
pub fn write_ccs_package(result: &BuildResult, output_path: &Path) -> Result<()> {
    write_ccs_package_internal(result, output_path, None)
}

/// Write a signed CCS package to disk
pub fn write_signed_ccs_package(
    result: &BuildResult,
    output_path: &Path,
    signing_key: &super::signing::SigningKeyPair,
) -> Result<()> {
    write_ccs_package_internal(result, output_path, Some(signing_key))
}

/// Internal function to write CCS package with optional signing
fn write_ccs_package_internal(
    result: &BuildResult,
    output_path: &Path,
    signing_key: Option<&super::signing::SigningKeyPair>,
) -> Result<()> {
    use crate::ccs::binary_manifest::{ComponentRef, Hash, MerkleTree};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::collections::BTreeMap;
    use tar::Builder;

    // Create temp directory for package contents
    let temp_dir = tempfile::tempdir()?;

    // Write component metadata and collect component refs
    let components_dir = temp_dir.path().join("components");
    fs::create_dir_all(&components_dir)?;

    let mut component_refs: BTreeMap<String, ComponentRef> = BTreeMap::new();
    let default_components = &result.manifest.components.default;

    for (name, component) in &result.components {
        let component_json = serde_json::to_string_pretty(component)?;
        let component_path = components_dir.join(format!("{}.json", name));
        fs::write(&component_path, &component_json)?;

        // Calculate hash of component JSON file
        let hash = Hash::sha256(component_json.as_bytes());

        component_refs.insert(
            name.clone(),
            ComponentRef {
                hash,
                file_count: component.files.len() as u32,
                total_size: component.size,
                default: default_components.contains(name),
            },
        );
    }

    // Calculate Merkle root
    let content_root = MerkleTree::calculate_root(&component_refs);

    // Build binary manifest
    let mut binary_manifest = build_binary_manifest(result, component_refs, content_root)?;

    // Serialize MANIFEST.toml first so we can embed its hash in the CBOR manifest
    let manifest_toml = result.manifest.to_toml()?;

    // Compute SHA-256 of TOML content and embed in binary manifest before signing.
    // This binds TOML-only fields (provenance, redirects, policy, capabilities,
    // enhancements) into the signed CBOR envelope.
    binary_manifest.toml_integrity_hash = Some(hash::sha256(manifest_toml.as_bytes()));

    // Write MANIFEST (CBOR-encoded binary manifest)
    let manifest_cbor = binary_manifest
        .to_cbor()
        .map_err(|e| BuilderError::ManifestEncoding(e.to_string()))?;
    fs::write(temp_dir.path().join("MANIFEST"), &manifest_cbor)?;

    // Write MANIFEST.toml (human-readable)
    fs::write(temp_dir.path().join("MANIFEST.toml"), &manifest_toml)?;

    // Sign the CBOR manifest if a signing key is provided
    if let Some(key) = signing_key {
        let signature = key.sign(&manifest_cbor);
        let sig_json = serde_json::to_string_pretty(&signature)?;
        fs::write(temp_dir.path().join("MANIFEST.sig"), &sig_json)?;
    }

    // Write content blobs
    let objects_dir = temp_dir.path().join("objects");
    fs::create_dir_all(&objects_dir)?;

    for (hash, content) in &result.blobs {
        let blob_path = crate::filesystem::object_path(&objects_dir, hash);
        if let Some(parent) = blob_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&blob_path, content)?;
    }

    // Create compressed tar archive
    let output_file = fs::File::create(output_path)?;
    let encoder = GzEncoder::new(output_file, Compression::default());
    let mut archive = Builder::new(encoder);

    // Check if we should normalize timestamps
    if result.manifest.policy.normalize_timestamps {
        // Get timestamp from SOURCE_DATE_EPOCH or use default
        let timestamp = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(1704067200); // 2024-01-01 00:00:00 UTC

        // Add files with normalized timestamps
        append_dir_with_mtime(&mut archive, temp_dir.path(), "", timestamp)?;
    } else {
        // Add all files from temp directory (preserves original timestamps)
        archive.append_dir_all(".", temp_dir.path())?;
    }

    let encoder = archive.into_inner()?;
    encoder.finish()?;

    Ok(())
}

/// Recursively append directory contents with a fixed mtime
fn append_dir_with_mtime<W: std::io::Write>(
    archive: &mut tar::Builder<W>,
    base_path: &Path,
    archive_path: &str,
    mtime: u64,
) -> Result<()> {
    for entry in fs::read_dir(base_path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();

        let entry_archive_path = if archive_path.is_empty() {
            file_name_str.to_string()
        } else {
            format!("{}/{}", archive_path, file_name_str)
        };

        if file_type.is_dir() {
            // Create directory entry
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            header.set_mtime(mtime);
            header.set_cksum();

            archive.append_data(&mut header, &entry_archive_path, std::io::empty())?;

            // Recurse into directory
            append_dir_with_mtime(archive, &entry.path(), &entry_archive_path, mtime)?;
        } else if file_type.is_file() {
            let content = fs::read(entry.path())?;
            let metadata = entry.metadata()?;

            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(metadata.permissions().mode());
            header.set_size(content.len() as u64);
            header.set_mtime(mtime);
            header.set_cksum();

            archive.append_data(&mut header, &entry_archive_path, content.as_slice())?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(entry.path())?;

            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_mode(0o777);
            header.set_size(0);
            header.set_mtime(mtime);
            header.set_cksum();

            archive.append_link(&mut header, &entry_archive_path, &target)?;
        }
    }

    Ok(())
}

/// Build a BinaryManifest from BuildResult
fn build_binary_manifest(
    result: &BuildResult,
    components: std::collections::BTreeMap<String, super::binary_manifest::ComponentRef>,
    content_root: super::binary_manifest::Hash,
) -> Result<super::binary_manifest::BinaryManifest> {
    use crate::ccs::binary_manifest::{
        BinaryBuildInfo, BinaryCapability, BinaryManifest, BinaryPlatform, BinaryRequirement,
        FORMAT_VERSION,
    };

    let manifest = &result.manifest;

    let platform = manifest.package.platform.as_ref().map(|p| BinaryPlatform {
        os: p.os.clone(),
        arch: p.arch.clone(),
        libc: p.libc.clone(),
        abi: p.abi.clone(),
    });

    let provides: Vec<BinaryCapability> = manifest
        .provides
        .capabilities
        .iter()
        .map(|cap| BinaryCapability {
            name: cap.clone(),
            version: None,
        })
        .collect();

    let requires: Vec<BinaryRequirement> = manifest
        .requires
        .capabilities
        .iter()
        .map(|cap| BinaryRequirement {
            name: cap.name().to_string(),
            version: cap.version().map(String::from),
            kind: "capability".to_string(),
        })
        .chain(
            manifest
                .requires
                .packages
                .iter()
                .map(|pkg| BinaryRequirement {
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                    kind: "package".to_string(),
                }),
        )
        .collect();

    let binary_hooks = convert_hooks_to_binary(&manifest.hooks)?;

    let build = manifest.build.as_ref().map(|b| BinaryBuildInfo {
        source: b.source.clone(),
        commit: b.commit.clone(),
        timestamp: b.timestamp.clone(),
        reproducible: b.reproducible,
    });

    Ok(BinaryManifest {
        format_version: FORMAT_VERSION,
        name: manifest.package.name.clone(),
        version: manifest.package.version.clone(),
        description: manifest.package.description.clone(),
        license: manifest.package.license.clone(),
        platform,
        provides,
        requires,
        components,
        hooks: binary_hooks,
        build,
        capabilities: manifest.capabilities.clone(),
        content_root,
        toml_integrity_hash: None,
    })
}

use crate::ccs::manifest::parse_octal_mode;

fn convert_hooks_to_binary(
    hooks: &crate::ccs::manifest::Hooks,
) -> crate::Result<Option<super::binary_manifest::BinaryHooks>> {
    use crate::ccs::binary_manifest::{
        BinaryAlternativeHook, BinaryDirectoryHook, BinaryGroupHook, BinaryHooks, BinarySysctlHook,
        BinarySystemdHook, BinaryTmpfilesHook, BinaryUserHook,
    };

    let binary = BinaryHooks {
        users: hooks
            .users
            .iter()
            .map(|u| BinaryUserHook {
                name: u.name.clone(),
                system: u.system,
                home: u.home.clone(),
                shell: u.shell.clone(),
                group: u.group.clone(),
            })
            .collect(),
        groups: hooks
            .groups
            .iter()
            .map(|g| BinaryGroupHook {
                name: g.name.clone(),
                system: g.system,
            })
            .collect(),
        directories: hooks
            .directories
            .iter()
            .map(|d| {
                Ok(BinaryDirectoryHook {
                    path: d.path.clone(),
                    mode: parse_octal_mode(&d.mode)?,
                    owner: d.owner.clone(),
                    group: d.group.clone(),
                })
            })
            .collect::<crate::Result<Vec<_>>>()?,
        systemd: hooks
            .systemd
            .iter()
            .map(|s| BinarySystemdHook {
                unit: s.unit.clone(),
                enable: s.enable,
            })
            .collect(),
        tmpfiles: hooks
            .tmpfiles
            .iter()
            .map(|t| {
                Ok(BinaryTmpfilesHook {
                    entry_type: t.entry_type.clone(),
                    path: t.path.clone(),
                    mode: parse_octal_mode(&t.mode)?,
                    owner: t.owner.clone(),
                    group: t.group.clone(),
                })
            })
            .collect::<crate::Result<Vec<_>>>()?,
        sysctl: hooks
            .sysctl
            .iter()
            .map(|s| BinarySysctlHook {
                key: s.key.clone(),
                value: s.value.clone(),
                only_if_lower: s.only_if_lower,
            })
            .collect(),
        alternatives: hooks
            .alternatives
            .iter()
            .map(|a| BinaryAlternativeHook {
                name: a.name.clone(),
                path: a.path.clone(),
                priority: a.priority,
            })
            .collect(),
        post_install: hooks.post_install.as_ref().map(|h| h.script.clone()),
        pre_remove: hooks.pre_remove.as_ref().map(|h| h.script.clone()),
    };

    if binary.is_empty() {
        Ok(None)
    } else {
        Ok(Some(binary))
    }
}

/// Print build summary
pub fn print_build_summary(result: &BuildResult) {
    println!();
    println!("Build Summary");
    println!("=============");
    println!();
    println!(
        "Package: {} v{}",
        result.manifest.package.name, result.manifest.package.version
    );
    println!("Total files: {}", result.files.len());
    println!("Total size: {} bytes", result.total_size);
    println!("Blobs: {} objects", result.blobs.len());

    // Print CDC chunk stats if chunking was enabled
    if let Some(ref stats) = result.chunk_stats {
        println!();
        println!("CDC Chunking:");
        println!("  Chunked files: {} (files >16KB)", stats.chunked_files);
        println!("  Whole files: {} (files ≤16KB)", stats.whole_files);
        println!("  Total chunks: {}", stats.total_chunks);
        println!("  Unique chunks: {}", stats.unique_chunks);
        if stats.dedup_savings > 0 {
            println!("  Intra-package dedup: {} bytes saved", stats.dedup_savings);
        }
    }

    println!();
    println!("Components:");

    let mut comp_names: Vec<_> = result.components.keys().collect();
    comp_names.sort();

    for name in comp_names {
        let comp = &result.components[name];
        println!(
            "  :{} - {} files ({} bytes)",
            name,
            comp.files.len(),
            comp.size
        );
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
}
