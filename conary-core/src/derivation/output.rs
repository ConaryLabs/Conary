// conary-core/src/derivation/output.rs

//! Package output types for derivation build results.
//!
//! After a derivation builds successfully, the outputs (files, symlinks) are
//! recorded in an `OutputManifest` whose `output_hash` provides a
//! content-addressed key for the build result.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::id::DerivationId;

/// A single file produced by a derivation build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputFile {
    /// Absolute path within the package.
    pub path: String,
    /// SHA-256 hash of the file contents.
    pub hash: String,
    /// File size in bytes.
    pub size: u64,
    /// Unix file mode (e.g. 0o755).
    pub mode: u32,
}

/// A symbolic link produced by a derivation build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSymlink {
    /// Absolute path of the symlink.
    pub path: String,
    /// The symlink target.
    pub target: String,
}

/// Manifest describing all outputs of a derivation build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputManifest {
    /// The derivation that produced these outputs.
    pub derivation_id: String,
    /// Content hash of all outputs (files + symlinks).
    pub output_hash: String,
    /// Files produced by the build.
    pub files: Vec<OutputFile>,
    /// Symlinks produced by the build.
    pub symlinks: Vec<OutputSymlink>,
    /// Wall-clock build duration in seconds.
    pub build_duration_secs: u64,
    /// ISO 8601 timestamp of when the build completed.
    pub built_at: String,
}

/// A complete package output: the manifest plus its serialized bytes and hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageOutput {
    /// The output manifest.
    pub manifest: OutputManifest,
    /// Serialized (TOML) manifest bytes.
    pub manifest_bytes: Vec<u8>,
    /// SHA-256 hash of `manifest_bytes`.
    pub manifest_hash: String,
}

impl OutputManifest {
    /// Compute a deterministic output hash from files and symlinks.
    ///
    /// The hash is the SHA-256 of all file hashes (sorted by path) followed by
    /// all symlink targets (sorted by path), each on its own line.
    #[must_use]
    pub fn compute_output_hash(files: &[OutputFile], symlinks: &[OutputSymlink]) -> String {
        let mut hasher = Sha256::new();

        let mut sorted_files: Vec<&OutputFile> = files.iter().collect();
        sorted_files.sort_by(|a, b| a.path.cmp(&b.path));

        let mut sorted_symlinks: Vec<&OutputSymlink> = symlinks.iter().collect();
        sorted_symlinks.sort_by(|a, b| a.path.cmp(&b.path));

        for file in sorted_files {
            hasher.update(format!("file:{}:{}\n", file.path, file.hash).as_bytes());
        }

        for symlink in sorted_symlinks {
            hasher.update(format!("symlink:{}:{}\n", symlink.path, symlink.target).as_bytes());
        }

        hex::encode(hasher.finalize())
    }

    /// Build a new `OutputManifest`, computing the output hash automatically.
    #[must_use]
    pub fn new(
        derivation_id: &DerivationId,
        files: Vec<OutputFile>,
        symlinks: Vec<OutputSymlink>,
        build_duration_secs: u64,
        built_at: String,
    ) -> Self {
        let output_hash = Self::compute_output_hash(&files, &symlinks);
        Self {
            derivation_id: derivation_id.to_string(),
            output_hash,
            files,
            symlinks,
            build_duration_secs,
            built_at,
        }
    }
}

impl PackageOutput {
    /// Build a `PackageOutput` from a manifest, serializing to TOML and hashing.
    #[must_use]
    pub fn from_manifest(manifest: OutputManifest) -> Self {
        let manifest_bytes = toml::to_string_pretty(&manifest)
            .expect("OutputManifest must serialize to TOML")
            .into_bytes();
        let manifest_hash = hex::encode(Sha256::digest(&manifest_bytes));
        Self {
            manifest,
            manifest_bytes,
            manifest_hash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_files() -> Vec<OutputFile> {
        vec![
            OutputFile {
                path: "/usr/bin/hello".to_owned(),
                hash: "aaa111".to_owned(),
                size: 1024,
                mode: 0o755,
            },
            OutputFile {
                path: "/usr/lib/libhello.so".to_owned(),
                hash: "bbb222".to_owned(),
                size: 4096,
                mode: 0o644,
            },
        ]
    }

    fn sample_symlinks() -> Vec<OutputSymlink> {
        vec![OutputSymlink {
            path: "/usr/lib/libhello.so.1".to_owned(),
            target: "libhello.so".to_owned(),
        }]
    }

    #[test]
    fn output_hash_is_deterministic() {
        let files = sample_files();
        let symlinks = sample_symlinks();

        let hash1 = OutputManifest::compute_output_hash(&files, &symlinks);
        let hash2 = OutputManifest::compute_output_hash(&files, &symlinks);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn output_hash_is_order_independent() {
        let files = sample_files();
        let mut files_reversed = files.clone();
        files_reversed.reverse();

        let symlinks = sample_symlinks();

        let hash1 = OutputManifest::compute_output_hash(&files, &symlinks);
        let hash2 = OutputManifest::compute_output_hash(&files_reversed, &symlinks);
        assert_eq!(hash1, hash2, "output hash must be independent of input order");
    }

    #[test]
    fn different_file_content_produces_different_hash() {
        let files1 = sample_files();
        let symlinks = sample_symlinks();

        let mut files2 = sample_files();
        files2[0].hash = "changed_hash".to_owned();

        let hash1 = OutputManifest::compute_output_hash(&files1, &symlinks);
        let hash2 = OutputManifest::compute_output_hash(&files2, &symlinks);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn output_manifest_serializes_to_toml() {
        let manifest = OutputManifest {
            derivation_id: "d".repeat(64),
            output_hash: "e".repeat(64),
            files: sample_files(),
            symlinks: sample_symlinks(),
            build_duration_secs: 42,
            built_at: "2026-03-19T00:00:00Z".to_owned(),
        };

        let toml_str = toml::to_string_pretty(&manifest).expect("must serialize");
        assert!(toml_str.contains("derivation_id"));
        assert!(toml_str.contains("output_hash"));
        assert!(toml_str.contains("/usr/bin/hello"));

        // Round-trip.
        let deserialized: OutputManifest =
            toml::from_str(&toml_str).expect("must deserialize");
        assert_eq!(manifest, deserialized);
    }

    #[test]
    fn package_output_from_manifest() {
        let manifest = OutputManifest {
            derivation_id: "d".repeat(64),
            output_hash: "e".repeat(64),
            files: sample_files(),
            symlinks: sample_symlinks(),
            build_duration_secs: 10,
            built_at: "2026-03-19T00:00:00Z".to_owned(),
        };

        let output = PackageOutput::from_manifest(manifest.clone());
        assert_eq!(output.manifest, manifest);
        assert!(!output.manifest_bytes.is_empty());
        assert_eq!(output.manifest_hash.len(), 64);
        assert!(output.manifest_hash.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
