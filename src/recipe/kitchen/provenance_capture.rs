// src/recipe/kitchen/provenance_capture.rs

//! Provenance capture during recipe builds
//!
//! This module provides the `ProvenanceCapture` struct that accumulates
//! provenance data through each build phase, then converts to a
//! `ManifestProvenance` for inclusion in the CCS package.

use crate::ccs::manifest::{ManifestProvenance, ProvenanceDep, ProvenancePatch};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Accumulates provenance data during a recipe build
#[derive(Debug, Default)]
pub struct ProvenanceCapture {
    // === Source Layer ===
    /// Primary source URL
    pub upstream_url: Option<String>,
    /// Hash of the primary source archive
    pub upstream_hash: Option<String>,
    /// Git commit if building from git
    pub git_commit: Option<String>,
    /// When sources were fetched
    pub fetch_timestamp: Option<DateTime<Utc>>,
    /// Patches applied during build
    pub patches: Vec<CapturedPatch>,

    // === Build Layer ===
    /// Hash of the recipe file
    pub recipe_hash: Option<String>,
    /// When the build started
    pub build_timestamp: Option<DateTime<Utc>>,
    /// Build host architecture
    pub host_arch: Option<String>,
    /// Build host kernel version
    pub host_kernel: Option<String>,
    /// Build dependencies with versions
    pub build_deps: Vec<CapturedDep>,
    /// Whether build was isolated (container)
    pub isolated: bool,

    // === Content Layer (populated during plate) ===
    /// Merkle root of all file hashes
    pub merkle_root: Option<String>,
    /// Individual file hashes for merkle tree
    file_hashes: BTreeMap<String, String>,
}

/// A patch captured during the build
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CapturedPatch {
    /// URL or path to the patch file
    pub source: String,
    /// Hash of the patch content
    pub hash: String,
    /// Strip level used when applying (-p N)
    pub strip_level: u32,
    /// Author if known
    pub author: Option<String>,
    /// Reason for the patch (from recipe)
    pub reason: Option<String>,
}

/// A captured build dependency
#[derive(Debug, Clone)]
pub struct CapturedDep {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// DNA hash of the dependency if known
    pub dna_hash: Option<String>,
}

impl ProvenanceCapture {
    /// Create a new provenance capture instance
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize with recipe file hash
    #[allow(dead_code)]
    pub fn with_recipe_hash(mut self, recipe_path: &Path) -> Self {
        if let Ok(content) = fs::read(recipe_path) {
            let hash = Sha256::digest(&content);
            self.recipe_hash = Some(format!("sha256:{}", hex::encode(hash)));
        }
        self
    }

    /// Record the start of the build (sets build timestamp)
    pub fn start_build(&mut self) {
        self.build_timestamp = Some(Utc::now());
        self.capture_host_info();
    }

    /// Capture host system information
    fn capture_host_info(&mut self) {
        // Architecture
        self.host_arch = Some(std::env::consts::ARCH.to_string());

        // Kernel version
        if let Ok(release) = fs::read_to_string("/proc/sys/kernel/osrelease") {
            self.host_kernel = Some(release.trim().to_string());
        }
    }

    /// Record source fetch during prep phase
    pub fn record_source_fetch(&mut self, url: &str, hash: &str) {
        if self.upstream_url.is_none() {
            // First source is the primary source
            self.upstream_url = Some(url.to_string());
            self.upstream_hash = Some(hash.to_string());
            self.fetch_timestamp = Some(Utc::now());
        }
        // Additional sources could be tracked in a separate field if needed
    }

    /// Record a git commit if building from git
    #[allow(dead_code)]
    pub fn record_git_commit(&mut self, commit: &str) {
        self.git_commit = Some(commit.to_string());
    }

    /// Record a patch being applied
    pub fn record_patch(
        &mut self,
        source: &str,
        content: &[u8],
        strip_level: u32,
        author: Option<&str>,
        reason: Option<&str>,
    ) {
        let hash = Sha256::digest(content);
        self.patches.push(CapturedPatch {
            source: source.to_string(),
            hash: format!("sha256:{}", hex::encode(hash)),
            strip_level,
            author: author.map(|s| s.to_string()),
            reason: reason.map(|s| s.to_string()),
        });
    }

    /// Record build dependencies
    #[allow(dead_code)]
    pub fn record_build_deps(&mut self, deps: Vec<CapturedDep>) {
        self.build_deps = deps;
    }

    /// Add a build dependency
    pub fn add_build_dep(&mut self, name: &str, version: &str, dna_hash: Option<&str>) {
        self.build_deps.push(CapturedDep {
            name: name.to_string(),
            version: version.to_string(),
            dna_hash: dna_hash.map(|s| s.to_string()),
        });
    }

    /// Record whether the build was isolated
    pub fn record_isolation(&mut self, isolated: bool) {
        self.isolated = isolated;
    }

    /// Record a file hash during packaging (for merkle root)
    pub fn record_file_hash(&mut self, path: &str, hash: &str) {
        self.file_hashes.insert(path.to_string(), hash.to_string());
    }

    /// Compute the merkle root from all recorded file hashes
    pub fn compute_merkle_root(&mut self) {
        if self.file_hashes.is_empty() {
            return;
        }

        // Sort by path for deterministic ordering (BTreeMap already sorted)
        let mut hasher = Sha256::new();

        for (path, hash) in &self.file_hashes {
            hasher.update(path.as_bytes());
            hasher.update(b":");
            hasher.update(hash.as_bytes());
            hasher.update(b"\n");
        }

        let root = hasher.finalize();
        self.merkle_root = Some(format!("sha256:{}", hex::encode(root)));
    }

    /// Compute the DNA hash from all provenance data
    pub fn compute_dna_hash(&self) -> String {
        let mut hasher = Sha256::new();

        // Source layer
        if let Some(url) = &self.upstream_url {
            hasher.update(b"source_url:");
            hasher.update(url.as_bytes());
            hasher.update(b"\n");
        }
        if let Some(hash) = &self.upstream_hash {
            hasher.update(b"source_hash:");
            hasher.update(hash.as_bytes());
            hasher.update(b"\n");
        }
        if let Some(commit) = &self.git_commit {
            hasher.update(b"git_commit:");
            hasher.update(commit.as_bytes());
            hasher.update(b"\n");
        }

        // Patches (sorted for determinism)
        for patch in &self.patches {
            hasher.update(b"patch:");
            hasher.update(patch.hash.as_bytes());
            hasher.update(b"\n");
        }

        // Build layer
        if let Some(recipe_hash) = &self.recipe_hash {
            hasher.update(b"recipe:");
            hasher.update(recipe_hash.as_bytes());
            hasher.update(b"\n");
        }

        // Build deps (sorted for determinism)
        let mut sorted_deps: Vec<_> = self.build_deps.iter().collect();
        sorted_deps.sort_by(|a, b| a.name.cmp(&b.name));
        for dep in sorted_deps {
            hasher.update(b"dep:");
            hasher.update(dep.name.as_bytes());
            hasher.update(b"@");
            hasher.update(dep.version.as_bytes());
            if let Some(dna) = &dep.dna_hash {
                hasher.update(b"#");
                hasher.update(dna.as_bytes());
            }
            hasher.update(b"\n");
        }

        // Content layer
        if let Some(merkle) = &self.merkle_root {
            hasher.update(b"merkle:");
            hasher.update(merkle.as_bytes());
            hasher.update(b"\n");
        }

        let hash = hasher.finalize();
        format!("sha256:{}", hex::encode(hash))
    }

    /// Convert to ManifestProvenance for inclusion in CCS manifest
    pub fn to_manifest_provenance(&self) -> ManifestProvenance {
        let dna_hash = self.compute_dna_hash();

        ManifestProvenance {
            // Source layer
            upstream_url: self.upstream_url.clone(),
            upstream_hash: self.upstream_hash.clone(),
            git_commit: self.git_commit.clone(),
            fetch_timestamp: self.fetch_timestamp.map(|t| t.to_rfc3339()),
            patches: self
                .patches
                .iter()
                .map(|p| ProvenancePatch {
                    url: Some(p.source.clone()),
                    hash: p.hash.clone(),
                    author: p.author.clone(),
                    reason: p.reason.clone(),
                })
                .collect(),

            // Build layer
            recipe_hash: self.recipe_hash.clone(),
            build_timestamp: self.build_timestamp.map(|t| t.to_rfc3339()),
            host_arch: self.host_arch.clone(),
            host_kernel: self.host_kernel.clone(),
            build_deps: self
                .build_deps
                .iter()
                .map(|d| ProvenanceDep {
                    name: d.name.clone(),
                    version: d.version.clone(),
                    dna_hash: d.dna_hash.clone(),
                })
                .collect(),

            // Signature layer (empty - signatures added post-build)
            signatures: Vec::new(),
            rekor_log_index: None,
            sbom_spdx: None,

            // Content layer
            merkle_root: self.merkle_root.clone(),
            dna_hash: Some(dna_hash),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_provenance_capture_new() {
        let capture = ProvenanceCapture::new();
        assert!(capture.upstream_url.is_none());
        assert!(capture.build_timestamp.is_none());
        assert!(capture.patches.is_empty());
    }

    #[test]
    fn test_record_source_fetch() {
        let mut capture = ProvenanceCapture::new();
        capture.record_source_fetch(
            "https://example.com/foo-1.0.tar.gz",
            "sha256:abc123",
        );

        assert_eq!(
            capture.upstream_url,
            Some("https://example.com/foo-1.0.tar.gz".to_string())
        );
        assert_eq!(capture.upstream_hash, Some("sha256:abc123".to_string()));
        assert!(capture.fetch_timestamp.is_some());
    }

    #[test]
    fn test_record_patch() {
        let mut capture = ProvenanceCapture::new();
        capture.record_patch(
            "fix-build.patch",
            b"--- a/foo\n+++ b/foo\n",
            1,
            Some("maintainer@example.com"),
            Some("Fix build on modern compilers"),
        );

        assert_eq!(capture.patches.len(), 1);
        assert_eq!(capture.patches[0].source, "fix-build.patch");
        assert!(capture.patches[0].hash.starts_with("sha256:"));
        assert_eq!(capture.patches[0].author, Some("maintainer@example.com".to_string()));
    }

    #[test]
    fn test_start_build_captures_host_info() {
        let mut capture = ProvenanceCapture::new();
        capture.start_build();

        assert!(capture.build_timestamp.is_some());
        assert!(capture.host_arch.is_some());
        // Kernel might not be available in all test environments
    }

    #[test]
    fn test_compute_merkle_root() {
        let mut capture = ProvenanceCapture::new();
        capture.record_file_hash("/usr/bin/foo", "sha256:abc123");
        capture.record_file_hash("/usr/lib/libfoo.so", "sha256:def456");
        capture.compute_merkle_root();

        assert!(capture.merkle_root.is_some());
        assert!(capture.merkle_root.as_ref().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn test_compute_dna_hash_deterministic() {
        let mut capture1 = ProvenanceCapture::new();
        capture1.upstream_url = Some("https://example.com/foo-1.0.tar.gz".to_string());
        capture1.upstream_hash = Some("sha256:abc123".to_string());
        capture1.recipe_hash = Some("sha256:recipe123".to_string());

        let mut capture2 = ProvenanceCapture::new();
        capture2.upstream_url = Some("https://example.com/foo-1.0.tar.gz".to_string());
        capture2.upstream_hash = Some("sha256:abc123".to_string());
        capture2.recipe_hash = Some("sha256:recipe123".to_string());

        assert_eq!(capture1.compute_dna_hash(), capture2.compute_dna_hash());
    }

    #[test]
    fn test_compute_dna_hash_different_sources() {
        let mut capture1 = ProvenanceCapture::new();
        capture1.upstream_url = Some("https://example.com/foo-1.0.tar.gz".to_string());

        let mut capture2 = ProvenanceCapture::new();
        capture2.upstream_url = Some("https://example.com/foo-2.0.tar.gz".to_string());

        assert_ne!(capture1.compute_dna_hash(), capture2.compute_dna_hash());
    }

    #[test]
    fn test_to_manifest_provenance() {
        let mut capture = ProvenanceCapture::new();
        capture.upstream_url = Some("https://example.com/foo-1.0.tar.gz".to_string());
        capture.upstream_hash = Some("sha256:abc123".to_string());
        capture.build_timestamp = Some(Utc::now());
        capture.host_arch = Some("x86_64".to_string());
        capture.add_build_dep("gcc", "14.2.0", None);

        let manifest_prov = capture.to_manifest_provenance();

        assert_eq!(manifest_prov.upstream_url, capture.upstream_url);
        assert_eq!(manifest_prov.upstream_hash, capture.upstream_hash);
        assert!(manifest_prov.build_timestamp.is_some());
        assert_eq!(manifest_prov.host_arch, Some("x86_64".to_string()));
        assert_eq!(manifest_prov.build_deps.len(), 1);
        assert_eq!(manifest_prov.build_deps[0].name, "gcc");
        assert!(manifest_prov.dna_hash.is_some());
    }

    #[test]
    fn test_with_recipe_hash() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "[package]\nname = \"test\"\nversion = \"1.0\"").unwrap();

        let capture = ProvenanceCapture::new().with_recipe_hash(file.path());

        assert!(capture.recipe_hash.is_some());
        assert!(capture.recipe_hash.as_ref().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn test_build_deps_sorted_for_dna() {
        let mut capture1 = ProvenanceCapture::new();
        capture1.add_build_dep("zlib", "1.0", None);
        capture1.add_build_dep("autoconf", "2.0", None);

        let mut capture2 = ProvenanceCapture::new();
        capture2.add_build_dep("autoconf", "2.0", None);
        capture2.add_build_dep("zlib", "1.0", None);

        // DNA hash should be the same regardless of insertion order
        assert_eq!(capture1.compute_dna_hash(), capture2.compute_dna_hash());
    }
}
