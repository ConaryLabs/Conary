// conary-core/src/ccs/builder.rs
//! CCS package builder.
//!
//! The builder owns typed package assembly. Native source discovery lives in
//! `builder/source.rs`, policy owns disk-backed transforms, and
//! `builder/package_writer.rs` owns final streamed archive emission.

use crate::ccs::chunking::{Chunker, MIN_CHUNK_SIZE};
use crate::ccs::manifest::CcsManifest;
use crate::ccs::policy::{PolicyAction, PolicyChain, PolicyError};
use crate::packages::payload::{PackagePayloadFile, ReopenablePayload};
use crate::payload::PayloadContentAuthority;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod package_writer;
mod source;

pub use package_writer::{
    print_build_summary, write_signed_current_ccs_package, write_v2_ccs_package,
    write_v2_ccs_package_from_sources,
};

#[cfg(test)]
pub(crate) mod test_support;

/// Typed errors for the CCS builder pipeline.
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    #[error(transparent)]
    PolicyConfiguration(#[from] PolicyError),

    #[error("file not under source directory: {0}")]
    FileNotUnderSource(PathBuf),

    #[error("policy rejected file {path}: {reason}")]
    PolicyRejected { path: String, reason: String },

    #[error("chunker not initialized even though chunking is enabled")]
    ChunkerNotInitialized,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// One signed file entry in a CCS package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub node: crate::payload::PayloadNode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<PayloadContentAuthority>,
    pub component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<String>>,
}

/// Component data in a built package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentData {
    pub name: String,
    pub files: Vec<FileEntry>,
    pub hash: String,
    pub size: u64,
}

/// Result of building a CCS package.
#[derive(Debug)]
pub struct BuildResult {
    pub manifest: CcsManifest,
    pub components: HashMap<String, ComponentData>,
    pub files: Vec<FileEntry>,
    /// Reopenable content descriptors. No payload object is aggregated here.
    pub payloads: Vec<PackagePayloadFile>,
    pub total_size: u64,
    pub chunked: bool,
    pub chunk_stats: Option<ChunkStats>,
}

impl BuildResult {
    /// Resolve the sole content descriptor for a signed regular-file path.
    pub fn payload(&self, path: &str) -> Result<&PackagePayloadFile> {
        let mut matches = self.payloads.iter().filter(|payload| payload.path == path);
        let payload = matches
            .next()
            .with_context(|| format!("missing payload source for {path}"))?;
        if matches.next().is_some() {
            anyhow::bail!("payload source path {path} appears more than once");
        }
        Ok(payload)
    }

    /// Open a regular file and verify descriptor authority matches build state.
    pub fn open_file(&self, file: &FileEntry) -> Result<Box<dyn Read + Send>> {
        let payload = self.payload(&file.path)?;
        if payload.node != file.node || payload.content_authority != file.content {
            anyhow::bail!(
                "payload descriptor authority disagrees with build entry {}",
                file.path
            );
        }
        payload.open_content().map_err(anyhow::Error::from)
    }
}

/// Convert already-bounded test fixture blobs into the same reopenable source
/// descriptors used by production authoring.
///
/// This is public only because workspace integration tests are separate
/// crates. It rejects aggregate fixture input above the package-writer's fixed
/// small-memory ceiling.
#[doc(hidden)]
pub fn payloads_from_bounded_memory_for_tests(
    files: &[FileEntry],
    blobs: HashMap<String, Vec<u8>>,
) -> Result<Vec<PackagePayloadFile>> {
    let input_total = blobs.values().try_fold(0_u64, |total, bytes| {
        total
            .checked_add(bytes.len() as u64)
            .context("bounded in-memory CCS fixture size overflow")
    })?;
    if input_total > package_writer::BOUNDED_MEMORY_FIXTURE_BYTES {
        anyhow::bail!(
            "bounded in-memory CCS fixture input is {} bytes; limit is {} bytes",
            input_total,
            package_writer::BOUNDED_MEMORY_FIXTURE_BYTES
        );
    }
    let retained_total = files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.content.as_ref().map_or(0, |authority| authority.size))
            .context("bounded in-memory CCS fixture retained size overflow")
    })?;
    if retained_total > package_writer::BOUNDED_MEMORY_FIXTURE_BYTES {
        anyhow::bail!(
            "bounded in-memory CCS fixture would retain {} payload bytes; limit is {} bytes",
            retained_total,
            package_writer::BOUNDED_MEMORY_FIXTURE_BYTES
        );
    }
    let blobs = blobs
        .into_iter()
        .map(|(hash, bytes)| (hash, Arc::<[u8]>::from(bytes)))
        .collect::<HashMap<_, _>>();
    let mut used = HashSet::new();
    let mut payloads = Vec::with_capacity(files.len());
    for file in files {
        let source = if file.node.kind.is_regular() {
            let authority = file.content.as_ref().with_context(|| {
                format!(
                    "regular fixture payload node {} has no content authority",
                    file.path
                )
            })?;
            let bytes = if let Some(chunks) = &file.chunks {
                let capacity = usize::try_from(authority.size)
                    .context("bounded fixture payload size exceeds usize")?;
                let mut assembled = Vec::with_capacity(capacity);
                for chunk in chunks {
                    let bytes = blobs.get(chunk).with_context(|| {
                        format!("missing bounded fixture chunk {chunk} for {}", file.path)
                    })?;
                    used.insert(chunk.as_str());
                    assembled.extend_from_slice(bytes);
                }
                Arc::<[u8]>::from(assembled)
            } else {
                used.insert(authority.sha256.as_str());
                Arc::clone(blobs.get(&authority.sha256).with_context(|| {
                    format!("missing bounded fixture payload for {}", file.path)
                })?)
            };
            if bytes.len() as u64 != authority.size
                || crate::hash::sha256(&bytes) != authority.sha256
            {
                anyhow::bail!(
                    "bounded fixture payload does not match authority for {}",
                    file.path
                );
            }
            Some(ReopenablePayload::from_in_memory_bytes(bytes))
        } else {
            None
        };
        payloads.push(
            PackagePayloadFile::new(
                file.path.clone(),
                file.node.clone(),
                file.content.clone(),
                source,
            )
            .map_err(anyhow::Error::from)?,
        );
    }
    if let Some(extra) = blobs.keys().find(|hash| !used.contains(hash.as_str())) {
        anyhow::bail!("bounded fixture carries unreferenced payload object {extra}");
    }
    Ok(payloads)
}

/// Statistics from bounded FastCDC visits.
#[derive(Debug, Clone, Default)]
pub struct ChunkStats {
    pub chunked_files: usize,
    pub whole_files: usize,
    pub total_chunks: usize,
    pub unique_chunks: usize,
    pub dedup_savings: u64,
}

/// Native CCS authoring builder.
pub struct CcsBuilder {
    manifest: CcsManifest,
    source_dir: PathBuf,
    install_prefix: PathBuf,
    policy_chain: PolicyChain,
    use_chunking: bool,
    chunker: Option<Chunker>,
}

fn is_suspicious_component_executable(component: &str, node: &crate::payload::PayloadNode) -> bool {
    node.kind.is_regular()
        && node.mode & 0o111 != 0
        && matches!(component, "doc" | "config" | "data")
}

impl CcsBuilder {
    pub fn new(
        manifest: CcsManifest,
        source_dir: &Path,
    ) -> std::result::Result<Self, BuilderError> {
        let policy_chain = PolicyChain::from_config(&manifest.policy)?;
        Ok(Self {
            manifest,
            source_dir: source_dir.to_path_buf(),
            install_prefix: PathBuf::from("/"),
            policy_chain,
            use_chunking: false,
            chunker: None,
        })
    }

    #[must_use]
    pub fn with_install_prefix(mut self, prefix: &Path) -> Self {
        self.install_prefix = prefix.to_path_buf();
        self
    }

    #[must_use]
    pub fn with_chunking(mut self) -> Self {
        self.use_chunking = true;
        self.chunker = Some(Chunker::new());
        self
    }

    /// Build source-backed package authority without retaining payload bytes.
    pub fn build(&self) -> Result<BuildResult> {
        self.validate_component_contract()?;
        let source_paths = self.scan_source_files()?;
        let workspace_parent = self
            .source_dir
            .parent()
            .unwrap_or(self.source_dir.as_path());
        let policy_workspace = Arc::new(
            tempfile::Builder::new()
                .prefix(".conary-ccs-authoring-")
                .tempdir_in(workspace_parent)
                .with_context(|| {
                    format!(
                        "create CCS authoring workspace beside {}",
                        self.source_dir.display()
                    )
                })?,
        );

        let mut hardlink_anchors = HashMap::new();
        let mut files = Vec::with_capacity(source_paths.len());
        let mut payloads = Vec::with_capacity(source_paths.len());
        let mut components: HashMap<String, Vec<FileEntry>> = HashMap::new();
        let mut chunk_stats = ChunkStats::default();
        let mut unique_chunks = HashSet::new();

        for (index, source_path) in source_paths.into_iter().enumerate() {
            let mut collected = self.collect_source(&source_path, &mut hardlink_anchors)?;
            let entry_workspace = policy_workspace.path().join(format!("{index:08}"));
            fs::create_dir(&entry_workspace)?;
            let policy_action = self.policy_chain.apply(
                &mut collected.entry,
                &source_path,
                &entry_workspace,
                &self.manifest.policy,
            )?;
            if matches!(policy_action, PolicyAction::Skip) {
                continue;
            }
            if let PolicyAction::Reject(reason) = policy_action {
                return Err(BuilderError::PolicyRejected {
                    path: collected.entry.path,
                    reason,
                }
                .into());
            }

            let source = if collected.entry.node.kind.is_regular() {
                let payload_source = match policy_action {
                    PolicyAction::Replace(path) => {
                        ReopenablePayload::from_spooled_path(path, Arc::clone(&policy_workspace))
                    }
                    PolicyAction::Keep => ReopenablePayload::from_path(source_path),
                    PolicyAction::Skip | PolicyAction::Reject(_) => unreachable!(),
                };
                let authority = stream_content_authority(&payload_source)?;
                collected.entry.content = Some(authority.clone());
                if self.use_chunking && authority.size >= u64::from(MIN_CHUNK_SIZE) {
                    self.visit_chunks(
                        &mut collected.entry,
                        &payload_source,
                        authority.size,
                        &mut chunk_stats,
                        &mut unique_chunks,
                    )?;
                } else {
                    chunk_stats.whole_files += 1;
                }
                Some(payload_source)
            } else {
                None
            };

            collected.entry.node.validate()?;
            collected
                .entry
                .node
                .validate_content(collected.entry.content.as_ref())?;
            if is_suspicious_component_executable(&collected.entry.component, &collected.entry.node)
            {
                tracing::warn!(
                    "Suspicious executable file {} assigned to '{}' component",
                    collected.entry.path,
                    collected.entry.component
                );
            }
            let payload = PackagePayloadFile::new(
                collected.entry.path.clone(),
                collected.entry.node.clone(),
                collected.entry.content.clone(),
                source,
            )
            .map_err(anyhow::Error::from)?;
            components
                .entry(collected.entry.component.clone())
                .or_default()
                .push(collected.entry.clone());
            files.push(collected.entry);
            payloads.push(payload);
        }

        let mut component_data = HashMap::new();
        for (name, component_files) in components {
            let size = component_files
                .iter()
                .filter_map(|file| file.content.as_ref().map(|content| content.size))
                .sum();
            let hash = self.compute_component_hash(&component_files);
            component_data.insert(
                name.clone(),
                ComponentData {
                    name,
                    files: component_files,
                    hash,
                    size,
                },
            );
        }
        let total_size = files
            .iter()
            .filter_map(|file| file.content.as_ref().map(|content| content.size))
            .sum();
        chunk_stats.unique_chunks = unique_chunks.len();

        Ok(BuildResult {
            manifest: self.manifest.clone(),
            components: component_data,
            files,
            payloads,
            total_size,
            chunked: self.use_chunking,
            chunk_stats: self.use_chunking.then_some(chunk_stats),
        })
    }

    fn visit_chunks(
        &self,
        entry: &mut FileEntry,
        source: &ReopenablePayload,
        expected_size: u64,
        stats: &mut ChunkStats,
        unique_chunks: &mut HashSet<String>,
    ) -> Result<()> {
        let chunker = self
            .chunker
            .as_ref()
            .ok_or(BuilderError::ChunkerNotInitialized)?;
        let mut chunk_hashes = Vec::new();
        let processed = chunker.visit_reader_chunks(source.open()?, |chunk| {
            let hash = chunk.hash_hex();
            chunk_hashes.push(hash.clone());
            stats.total_chunks += 1;
            if !unique_chunks.insert(hash) {
                stats.dedup_savings = stats
                    .dedup_savings
                    .checked_add(u64::from(chunk.length))
                    .context("chunk deduplication savings overflow")?;
            }
            Ok(())
        })?;
        if processed != expected_size {
            anyhow::bail!(
                "payload source size changed while chunking {}: expected {}, got {}",
                entry.path,
                expected_size,
                processed
            );
        }
        entry.chunks = Some(chunk_hashes);
        stats.chunked_files += 1;
        Ok(())
    }

    fn component_for_path(&self, path: &str) -> Result<String> {
        if let Some(component) = self.manifest.components.files.get(path) {
            return Ok(component.clone());
        }
        let mut matched_component = None;
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
            matched_component = Some(rule.component.as_str());
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

fn stream_content_authority(source: &ReopenablePayload) -> Result<PayloadContentAuthority> {
    let mut reader = source.open()?;
    let mut hasher = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
    let mut size = 0_u64;
    let mut buffer = [0_u8; crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size = size
            .checked_add(count as u64)
            .context("payload source size overflow")?;
    }
    Ok(PayloadContentAuthority {
        sha256: hasher.finalize().value,
        size,
    })
}

#[cfg(test)]
#[path = "builder/tests.rs"]
mod tests;
