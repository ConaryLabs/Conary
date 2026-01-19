// src/provenance/source.rs

//! Source layer provenance - where the code came from

use super::CanonicalBytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Source layer provenance information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceProvenance {
    /// URL where the source was fetched from
    #[serde(default)]
    pub upstream_url: Option<String>,

    /// Hash of the upstream source archive
    #[serde(default)]
    pub upstream_hash: Option<String>,

    /// Git commit hash if built from git
    #[serde(default)]
    pub git_commit: Option<String>,

    /// Git repository URL
    #[serde(default)]
    pub git_repo: Option<String>,

    /// Git tag if applicable
    #[serde(default)]
    pub git_tag: Option<String>,

    /// Patches applied to the source
    #[serde(default)]
    pub patches: Vec<PatchInfo>,

    /// When the source was fetched
    #[serde(default)]
    pub fetch_timestamp: Option<DateTime<Utc>>,

    /// Mirror URLs where source was verified
    #[serde(default)]
    pub verified_mirrors: Vec<String>,
}

impl SourceProvenance {
    /// Create source provenance from a tarball URL
    pub fn from_tarball(url: &str, hash: &str) -> Self {
        Self {
            upstream_url: Some(url.to_string()),
            upstream_hash: Some(hash.to_string()),
            fetch_timestamp: Some(Utc::now()),
            ..Default::default()
        }
    }

    /// Create source provenance from a git repository
    pub fn from_git(repo: &str, commit: &str, tag: Option<&str>) -> Self {
        Self {
            git_repo: Some(repo.to_string()),
            git_commit: Some(commit.to_string()),
            git_tag: tag.map(|t| t.to_string()),
            fetch_timestamp: Some(Utc::now()),
            ..Default::default()
        }
    }

    /// Add a patch to the source
    pub fn add_patch(&mut self, patch: PatchInfo) {
        self.patches.push(patch);
    }

    /// Check if source has verified upstream hash
    pub fn has_verified_hash(&self) -> bool {
        self.upstream_hash.is_some() || self.git_commit.is_some()
    }
}

impl CanonicalBytes for SourceProvenance {
    fn canonical_bytes(&self) -> Vec<u8> {
        // Create deterministic representation for hashing
        let mut bytes = Vec::new();

        if let Some(ref url) = self.upstream_url {
            bytes.extend_from_slice(b"url:");
            bytes.extend_from_slice(url.as_bytes());
            bytes.push(0);
        }

        if let Some(ref hash) = self.upstream_hash {
            bytes.extend_from_slice(b"hash:");
            bytes.extend_from_slice(hash.as_bytes());
            bytes.push(0);
        }

        if let Some(ref commit) = self.git_commit {
            bytes.extend_from_slice(b"commit:");
            bytes.extend_from_slice(commit.as_bytes());
            bytes.push(0);
        }

        // Sort patches by hash for determinism
        let mut patches: Vec<_> = self.patches.iter().collect();
        patches.sort_by(|a, b| a.hash.cmp(&b.hash));

        for patch in patches {
            bytes.extend_from_slice(b"patch:");
            bytes.extend_from_slice(patch.hash.as_bytes());
            bytes.push(0);
        }

        bytes
    }
}

/// Information about a patch applied to source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchInfo {
    /// URL or local path where patch was sourced
    pub url: Option<String>,

    /// Hash of the patch file
    pub hash: String,

    /// Who authored the patch
    #[serde(default)]
    pub author: Option<String>,

    /// Reason for the patch
    #[serde(default)]
    pub reason: Option<String>,

    /// CVE this patch addresses (if security-related)
    #[serde(default)]
    pub cve: Option<String>,

    /// Patch level (-p argument)
    #[serde(default = "default_patch_level")]
    pub level: i32,
}

fn default_patch_level() -> i32 {
    1
}

impl PatchInfo {
    /// Create a new patch info
    pub fn new(hash: &str) -> Self {
        Self {
            url: None,
            hash: hash.to_string(),
            author: None,
            reason: None,
            cve: None,
            level: 1,
        }
    }

    /// Create patch info with URL
    pub fn with_url(url: &str, hash: &str) -> Self {
        Self {
            url: Some(url.to_string()),
            hash: hash.to_string(),
            author: None,
            reason: None,
            cve: None,
            level: 1,
        }
    }

    /// Set the reason for this patch
    pub fn with_reason(mut self, reason: &str) -> Self {
        self.reason = Some(reason.to_string());
        self
    }

    /// Set the CVE this patch addresses
    pub fn with_cve(mut self, cve: &str) -> Self {
        self.cve = Some(cve.to_string());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_tarball() {
        let source = SourceProvenance::from_tarball(
            "https://example.com/source.tar.gz",
            "sha256:abc123",
        );
        assert_eq!(source.upstream_url.as_deref(), Some("https://example.com/source.tar.gz"));
        assert!(source.has_verified_hash());
    }

    #[test]
    fn test_from_git() {
        let source = SourceProvenance::from_git(
            "https://github.com/example/repo",
            "abc123def456",
            Some("v1.0.0"),
        );
        assert_eq!(source.git_commit.as_deref(), Some("abc123def456"));
        assert_eq!(source.git_tag.as_deref(), Some("v1.0.0"));
    }

    #[test]
    fn test_patch_info() {
        let patch = PatchInfo::with_url(
            "https://example.com/fix.patch",
            "sha256:def456",
        )
        .with_reason("Fix segfault")
        .with_cve("CVE-2026-1234");

        assert_eq!(patch.cve.as_deref(), Some("CVE-2026-1234"));
    }

    #[test]
    fn test_canonical_bytes_deterministic() {
        let source1 = SourceProvenance::from_tarball("https://example.com/a.tar.gz", "sha256:abc");
        let source2 = SourceProvenance::from_tarball("https://example.com/a.tar.gz", "sha256:abc");

        assert_eq!(source1.canonical_bytes(), source2.canonical_bytes());
    }
}
