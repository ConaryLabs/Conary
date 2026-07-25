// conary-core/src/derivation/output.rs

//! Exact package-output authority for derivation build results.

use serde::{Deserialize, Serialize};

use crate::generation::root_manifest::{GenerationRootEntry, validate_payload_tree};
use crate::payload::ResolvedPayloadNode;

/// Current derivation-output manifest contract.
///
/// Versions 1 and 2 described only regular files and symlinks and silently
/// discarded ownership, timestamps, xattrs, directories, hardlinks, and
/// special nodes. They are intentionally unsupported.
pub const OUTPUT_MANIFEST_VERSION: u32 = 3;

/// Errors produced while validating or serializing an output manifest.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("invalid output manifest: {0}")]
    Invalid(String),
    #[error("output manifest TOML serialization failed: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("output manifest TOML parsing failed: {0}")]
    Parse(#[from] toml::de::Error),
}

/// Manifest describing the complete exact output tree of one derivation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputManifest {
    /// Persisted format contract. Only [`OUTPUT_MANIFEST_VERSION`] is accepted.
    pub format_version: u32,
    /// The derivation that produced these outputs.
    pub derivation_id: String,
    /// Package identity used by the remote derivation-cache index.
    pub package_name: String,
    /// Package version used by the remote derivation-cache index.
    pub package_version: String,
    /// Content hash of the exact root and all output entries.
    pub output_hash: String,
    /// Exact numeric metadata of the build output root.
    pub root: ResolvedPayloadNode,
    /// Complete, sorted POSIX node and content authority.
    pub entries: Vec<GenerationRootEntry>,
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

#[derive(Serialize)]
struct OutputHashAuthority<'a> {
    format_version: u32,
    root: &'a ResolvedPayloadNode,
    entries: &'a [GenerationRootEntry],
}

impl OutputManifest {
    /// Build and validate a current-format output manifest.
    pub fn new(
        derivation_id: impl Into<String>,
        package_name: impl Into<String>,
        package_version: impl Into<String>,
        root: ResolvedPayloadNode,
        entries: Vec<GenerationRootEntry>,
        build_duration_secs: u64,
        built_at: String,
    ) -> Result<Self, OutputError> {
        let output_hash = Self::compute_output_hash(&root, &entries)?;
        let manifest = Self {
            format_version: OUTPUT_MANIFEST_VERSION,
            derivation_id: derivation_id.into(),
            package_name: package_name.into(),
            package_version: package_version.into(),
            output_hash,
            root,
            entries,
            build_duration_secs,
            built_at,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Hash every field that defines the exact output filesystem.
    pub fn compute_output_hash(
        root: &ResolvedPayloadNode,
        entries: &[GenerationRootEntry],
    ) -> Result<String, OutputError> {
        let authority = OutputHashAuthority {
            format_version: OUTPUT_MANIFEST_VERSION,
            root,
            entries,
        };
        let canonical = crate::json::canonical_json(&authority).map_err(OutputError::Invalid)?;
        Ok(crate::hash::sha256(&canonical))
    }

    /// Validate the current format, exact payload tree, and content hash.
    pub fn validate(&self) -> Result<(), OutputError> {
        if self.format_version != OUTPUT_MANIFEST_VERSION {
            return Err(OutputError::Invalid(format!(
                "unsupported format version {}; expected {}",
                self.format_version, OUTPUT_MANIFEST_VERSION
            )));
        }
        if self.derivation_id.is_empty() || self.derivation_id.contains('\0') {
            return Err(OutputError::Invalid(
                "derivation ID must be non-empty and NUL-free".to_string(),
            ));
        }
        if self.package_name.is_empty() || self.package_name.contains('\0') {
            return Err(OutputError::Invalid(
                "package name must be non-empty and NUL-free".to_string(),
            ));
        }
        if self.package_version.is_empty() || self.package_version.contains('\0') {
            return Err(OutputError::Invalid(
                "package version must be non-empty and NUL-free".to_string(),
            ));
        }
        validate_payload_tree(&self.root, &self.entries)
            .map_err(|error| OutputError::Invalid(error.to_string()))?;
        let expected = Self::compute_output_hash(&self.root, &self.entries)?;
        if self.output_hash != expected {
            return Err(OutputError::Invalid(format!(
                "output hash mismatch: manifest has {}, exact payload tree hashes to {expected}",
                self.output_hash
            )));
        }
        Ok(())
    }

    /// Parse and validate a current-format TOML manifest.
    pub fn from_toml(input: &str) -> Result<Self, OutputError> {
        let manifest: Self = toml::from_str(input)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Iterate over regular-file content authorities.
    pub fn regular_entries(&self) -> impl Iterator<Item = &GenerationRootEntry> {
        self.entries.iter().filter(|entry| entry.content.is_some())
    }
}

impl PackageOutput {
    /// Validate and serialize a package output manifest to TOML.
    pub fn from_manifest(manifest: OutputManifest) -> Result<Self, OutputError> {
        manifest.validate()?;
        let manifest_bytes = toml::to_string_pretty(&manifest)?.into_bytes();
        let manifest_hash = crate::hash::sha256(&manifest_bytes);
        Ok(Self {
            manifest,
            manifest_bytes,
            manifest_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::generation::root_manifest::GenerationRootEntry;
    use crate::payload::{
        PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
        ResolvedPayloadNode,
    };

    fn node(kind: PayloadNodeKind, permissions: u32) -> ResolvedPayloadNode {
        let mode_type = match kind {
            PayloadNodeKind::Regular { .. } | PayloadNodeKind::Hardlink { .. } => libc::S_IFREG,
            PayloadNodeKind::Directory => libc::S_IFDIR,
            PayloadNodeKind::Symlink { .. } => libc::S_IFLNK,
            PayloadNodeKind::BlockDevice { .. } => libc::S_IFBLK,
            PayloadNodeKind::CharacterDevice { .. } => libc::S_IFCHR,
            PayloadNodeKind::Fifo => libc::S_IFIFO,
            PayloadNodeKind::Socket => libc::S_IFSOCK,
        };
        ResolvedPayloadNode {
            source: PayloadNode {
                kind,
                mode: mode_type | permissions,
                user: PayloadIdentity::Numeric { id: 0 },
                group: PayloadIdentity::Numeric { id: 0 },
                mtime: PayloadTimestamp::UNIX_EPOCH,
                xattrs: BTreeMap::new(),
            },
            uid: 0,
            gid: 0,
        }
    }

    fn root() -> ResolvedPayloadNode {
        node(PayloadNodeKind::Directory, 0o755)
    }

    fn sample_entries() -> Vec<GenerationRootEntry> {
        vec![
            GenerationRootEntry {
                path: "/usr".to_string(),
                node: node(PayloadNodeKind::Directory, 0o755),
                content: None,
            },
            GenerationRootEntry {
                path: "/usr/bin".to_string(),
                node: node(PayloadNodeKind::Directory, 0o755),
                content: None,
            },
            GenerationRootEntry {
                path: "/usr/bin/hello".to_string(),
                node: node(
                    PayloadNodeKind::Regular {
                        hardlink_identity: None,
                    },
                    0o755,
                ),
                content: Some(PayloadContentAuthority {
                    sha256: "a".repeat(64),
                    size: 1024,
                }),
            },
        ]
    }

    fn manifest() -> OutputManifest {
        OutputManifest::new(
            "d".repeat(64),
            "hello",
            "1.0",
            root(),
            sample_entries(),
            42,
            "2026-03-19T00:00:00Z".to_string(),
        )
        .unwrap()
    }

    #[test]
    fn output_hash_is_deterministic_and_covers_exact_metadata() {
        let entries = sample_entries();
        let first = OutputManifest::compute_output_hash(&root(), &entries).unwrap();
        let second = OutputManifest::compute_output_hash(&root(), &entries).unwrap();
        assert_eq!(first, second);

        let mut changed = entries;
        changed[2].node.uid = 42;
        changed[2].node.source.user = PayloadIdentity::Numeric { id: 42 };
        let changed = OutputManifest::compute_output_hash(&root(), &changed).unwrap();
        assert_ne!(first, changed);
    }

    #[test]
    fn output_manifest_round_trips_current_toml() {
        let manifest = manifest();
        let toml = toml::to_string_pretty(&manifest).unwrap();
        assert_eq!(OutputManifest::from_toml(&toml).unwrap(), manifest);
    }

    #[test]
    fn old_lossy_manifests_are_not_readable() {
        let old = r#"
derivation_id = "test"
output_hash = "test"
hash_version = 2
files = []
symlinks = []
build_duration_secs = 0
built_at = ""
"#;
        assert!(OutputManifest::from_toml(old).is_err());
    }

    #[test]
    fn tampered_hash_is_rejected() {
        let mut manifest = manifest();
        manifest.output_hash = "f".repeat(64);
        assert!(manifest.validate().is_err());
        assert!(PackageOutput::from_manifest(manifest).is_err());
    }
}
