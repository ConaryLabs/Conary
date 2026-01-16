// src/ccs/builder.rs
//! CCS package builder
//!
//! Builds .ccs packages from a manifest and source directory.
//! Handles file scanning, component classification, and package creation.
//! Supports Content-Defined Chunking (CDC) for efficient delta updates.

use crate::ccs::chunking::{Chunker, MIN_CHUNK_SIZE};
use crate::ccs::manifest::CcsManifest;
use crate::ccs::policy::{PolicyAction, PolicyChain};
use crate::components::ComponentClassifier;
use crate::hash;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

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
                        // This shouldn't happen as the chain already returns an error
                        anyhow::bail!("Policy rejected file {}: {}", entry.path, msg);
                    }
                    PolicyAction::Keep | PolicyAction::Replace(_) => {
                        // Content may have been modified - recompute hash if needed
                        if new_content != entry.hash.as_bytes() {
                            let new_hash = hash::sha256(&new_content);
                            if new_hash != entry.hash {
                                entry.hash = new_hash;
                                entry.size = new_content.len() as u64;
                            }
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
                let chunker = self.chunker.as_ref()
                    .context("Chunker not initialized even though chunking is enabled")?;
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
            .context("File not under source directory")?;
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
            let content = format!("symlink:{}", target_str);
            (FileType::Symlink, content.into_bytes(), Some(target_str))
        } else {
            let content = fs::read(source_path)?;
            (FileType::Regular, content, None)
        };

        // Compute initial hash
        let hash_val = hash::sha256(&content);
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
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            let metadata = entry.metadata()?;

            // Skip the source directory itself
            if path == self.source_dir {
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
            Err(_) => {
                // Fallback to simple matching if pattern is invalid
                // or just log warning (but we don't have logger here easily accessible without importing)
                // For now, simple fallback for basic cases
                if pattern.contains('*') {
                    // Very basic fallback
                    path.starts_with(pattern.split('*').next().unwrap_or(""))
                } else {
                    path == pattern
                }
            }
        }
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
    use flate2::write::GzEncoder;
    use flate2::Compression;
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
    let binary_manifest = build_binary_manifest(result, component_refs, content_root)?;

    // Write MANIFEST (CBOR-encoded binary manifest)
    let manifest_cbor = binary_manifest
        .to_cbor()
        .context("Failed to encode binary manifest as CBOR")?;
    fs::write(temp_dir.path().join("MANIFEST"), &manifest_cbor)?;

    // Write MANIFEST.toml (human-readable, for debugging)
    let manifest_toml = result.manifest.to_toml()?;
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
        BinaryBuildInfo, BinaryManifest, BinaryCapability, BinaryHooks, BinaryPlatform,
        BinaryRequirement, FORMAT_VERSION,
        BinaryUserHook, BinaryGroupHook, BinaryDirectoryHook, BinarySystemdHook,
        BinaryTmpfilesHook, BinarySysctlHook, BinaryAlternativeHook,
    };

    let manifest = &result.manifest;

    // Convert platform
    let platform = manifest.package.platform.as_ref().map(|p| BinaryPlatform {
        os: p.os.clone(),
        arch: p.arch.clone(),
        libc: p.libc.clone(),
        abi: p.abi.clone(),
    });

    // Convert provides
    let mut provides = Vec::new();
    for cap in &manifest.provides.capabilities {
        provides.push(BinaryCapability {
            name: cap.clone(),
            version: None,
        });
    }

    // Convert requires
    let mut requires = Vec::new();
    for cap in &manifest.requires.capabilities {
        requires.push(BinaryRequirement {
            name: cap.name().to_string(),
            version: cap.version().map(String::from),
            kind: "capability".to_string(),
        });
    }
    for pkg in &manifest.requires.packages {
        requires.push(BinaryRequirement {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            kind: "package".to_string(),
        });
    }

    // Convert hooks
    let hooks = &manifest.hooks;
    let binary_hooks = if hooks.users.is_empty()
        && hooks.groups.is_empty()
        && hooks.directories.is_empty()
        && hooks.systemd.is_empty()
        && hooks.tmpfiles.is_empty()
        && hooks.sysctl.is_empty()
        && hooks.alternatives.is_empty()
    {
        None
    } else {
        Some(BinaryHooks {
            users: hooks.users.iter().map(|u| BinaryUserHook {
                name: u.name.clone(),
                system: u.system,
                home: u.home.clone(),
                shell: u.shell.clone(),
                group: u.group.clone(),
            }).collect(),
            groups: hooks.groups.iter().map(|g| BinaryGroupHook {
                name: g.name.clone(),
                system: g.system,
            }).collect(),
            directories: hooks.directories.iter().map(|d| BinaryDirectoryHook {
                path: d.path.clone(),
                mode: u32::from_str_radix(d.mode.trim_start_matches('0'), 8).unwrap_or(0o755),
                owner: d.owner.clone(),
                group: d.group.clone(),
            }).collect(),
            systemd: hooks.systemd.iter().map(|s| BinarySystemdHook {
                unit: s.unit.clone(),
                enable: s.enable,
            }).collect(),
            tmpfiles: hooks.tmpfiles.iter().map(|t| BinaryTmpfilesHook {
                entry_type: t.entry_type.clone(),
                path: t.path.clone(),
                mode: u32::from_str_radix(t.mode.trim_start_matches('0'), 8).unwrap_or(0o755),
                owner: t.owner.clone(),
                group: t.group.clone(),
            }).collect(),
            sysctl: hooks.sysctl.iter().map(|s| BinarySysctlHook {
                key: s.key.clone(),
                value: s.value.clone(),
                only_if_lower: s.only_if_lower,
            }).collect(),
            alternatives: hooks.alternatives.iter().map(|a| BinaryAlternativeHook {
                name: a.name.clone(),
                path: a.path.clone(),
                priority: a.priority,
            }).collect(),
        })
    };

    // Convert build info
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
        content_root,
    })
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
            println!(
                "  Intra-package dedup: {} bytes saved",
                stats.dedup_savings
            );
        }
    }

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
