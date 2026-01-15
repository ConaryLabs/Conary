// src/ccs/binary_manifest.rs
//! Binary CCS Manifest (CBOR-encoded)
//!
//! This module defines the compact binary representation of a CCS package manifest.
//! The binary manifest is CBOR-encoded for efficient storage and parsing.
//! A human-readable MANIFEST.toml is also included in packages for debugging.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Current format version
pub const FORMAT_VERSION: u8 = 1;

/// Binary manifest structure (CBOR-encoded)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryManifest {
    /// Format version (currently 1)
    pub format_version: u8,

    /// Package name
    pub name: String,

    /// Package version (semver)
    pub version: String,

    /// Short description
    pub description: String,

    /// SPDX license identifier
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Target platform
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<BinaryPlatform>,

    /// What this package provides
    #[serde(default)]
    pub provides: Vec<BinaryCapability>,

    /// What this package requires
    #[serde(default)]
    pub requires: Vec<BinaryRequirement>,

    /// Component references (name -> hash of component JSON)
    #[serde(default)]
    pub components: BTreeMap<String, ComponentRef>,

    /// Declarative hooks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<BinaryHooks>,

    /// Build provenance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BinaryBuildInfo>,

    /// Merkle root of all content (SHA-256)
    pub content_root: Hash,
}

impl BinaryManifest {
    /// Encode to CBOR bytes
    pub fn to_cbor(&self) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
        let mut buf = Vec::new();
        ciborium::into_writer(self, &mut buf)?;
        Ok(buf)
    }

    /// Decode from CBOR bytes
    pub fn from_cbor(data: &[u8]) -> Result<Self, ciborium::de::Error<std::io::Error>> {
        ciborium::from_reader(data)
    }

    /// Calculate SHA-256 hash of the encoded manifest
    pub fn hash(&self) -> Result<Hash, ciborium::ser::Error<std::io::Error>> {
        let cbor = self.to_cbor()?;
        Ok(Hash::sha256(&cbor))
    }
}

/// Platform specification
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BinaryPlatform {
    #[serde(default)]
    pub os: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,

    #[serde(default)]
    pub libc: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<String>,
}

/// A capability this package provides
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryCapability {
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A requirement (capability or package)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryRequirement {
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// "capability" or "package"
    #[serde(default)]
    pub kind: String,
}

/// Reference to a component's file list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentRef {
    /// SHA-256 hash of the component JSON file
    pub hash: Hash,

    /// Number of files in this component
    pub file_count: u32,

    /// Total size of files in this component
    pub total_size: u64,

    /// Whether this component installs by default
    #[serde(default)]
    pub default: bool,
}

/// Cryptographic hash
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hash {
    /// Algorithm name (e.g., "sha256")
    pub algorithm: String,

    /// Hash value as hex string
    pub value: String,
}

impl Hash {
    /// Create a new SHA-256 hash from data
    pub fn sha256(data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        Hash {
            algorithm: "sha256".to_string(),
            value: hex::encode(result),
        }
    }

    /// Create a Hash from a hex string (assumes SHA-256)
    pub fn from_hex(hex_str: &str) -> Self {
        Hash {
            algorithm: "sha256".to_string(),
            value: hex_str.to_string(),
        }
    }

    /// Get the hash value as bytes
    pub fn as_bytes(&self) -> Option<Vec<u8>> {
        hex::decode(&self.value).ok()
    }
}

/// Simplified hooks for binary manifest
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BinaryHooks {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<BinaryUserHook>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<BinaryGroupHook>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directories: Vec<BinaryDirectoryHook>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub systemd: Vec<BinarySystemdHook>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tmpfiles: Vec<BinaryTmpfilesHook>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sysctl: Vec<BinarySysctlHook>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<BinaryAlternativeHook>,
}

impl BinaryHooks {
    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
            && self.groups.is_empty()
            && self.directories.is_empty()
            && self.systemd.is_empty()
            && self.tmpfiles.is_empty()
            && self.sysctl.is_empty()
            && self.alternatives.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryUserHook {
    pub name: String,
    #[serde(default)]
    pub system: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryGroupHook {
    pub name: String,
    #[serde(default)]
    pub system: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryDirectoryHook {
    pub path: String,
    pub mode: u32,
    pub owner: String,
    pub group: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinarySystemdHook {
    pub unit: String,
    #[serde(default)]
    pub enable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryTmpfilesHook {
    pub entry_type: String,
    pub path: String,
    pub mode: u32,
    pub owner: String,
    pub group: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinarySysctlHook {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub only_if_lower: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryAlternativeHook {
    pub name: String,
    pub path: String,
    pub priority: i32,
}

/// Build provenance for binary manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryBuildInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,

    #[serde(default)]
    pub reproducible: bool,
}

/// Merkle tree construction for content verification
pub struct MerkleTree;

impl MerkleTree {
    /// Calculate the Merkle root from component hashes
    ///
    /// ```text
    /// content_root = SHA256(
    ///     sorted([
    ///         SHA256(component_name || component_hash)
    ///         for each component
    ///     ])
    /// )
    /// ```
    pub fn calculate_root(components: &BTreeMap<String, ComponentRef>) -> Hash {
        if components.is_empty() {
            // Empty tree has a well-known hash
            return Hash::sha256(b"empty");
        }

        // Calculate leaf hashes: SHA256(name || hash)
        let mut leaf_hashes: Vec<Vec<u8>> = Vec::new();

        for (name, comp_ref) in components {
            let mut hasher = Sha256::new();
            hasher.update(name.as_bytes());
            hasher.update(comp_ref.hash.value.as_bytes());
            leaf_hashes.push(hasher.finalize().to_vec());
        }

        // Sort the leaf hashes (BTreeMap already sorted by key, but we sort hashes for consistency)
        leaf_hashes.sort();

        // Calculate root: SHA256(concatenated sorted leaf hashes)
        let mut root_hasher = Sha256::new();
        for leaf in &leaf_hashes {
            root_hasher.update(leaf);
        }

        Hash {
            algorithm: "sha256".to_string(),
            value: hex::encode(root_hasher.finalize()),
        }
    }

    /// Verify that the content matches the Merkle root
    pub fn verify_root(components: &BTreeMap<String, ComponentRef>, expected_root: &Hash) -> bool {
        let calculated = Self::calculate_root(components);
        calculated == *expected_root
    }
}

/// Helper module for hex encoding (avoiding extra dependency)
mod hex {
    pub fn encode(data: impl AsRef<[u8]>) -> String {
        data.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, ()> {
        if !s.len().is_multiple_of(2) {
            return Err(());
        }

        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_sha256() {
        let hash = Hash::sha256(b"hello world");
        assert_eq!(hash.algorithm, "sha256");
        assert_eq!(
            hash.value,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_binary_manifest_cbor_roundtrip() {
        let manifest = BinaryManifest {
            format_version: FORMAT_VERSION,
            name: "test-package".to_string(),
            version: "1.0.0".to_string(),
            description: "A test package".to_string(),
            license: Some("MIT".to_string()),
            platform: None,
            provides: vec![BinaryCapability {
                name: "test".to_string(),
                version: None,
            }],
            requires: vec![],
            components: BTreeMap::new(),
            hooks: None,
            build: None,
            content_root: Hash::sha256(b"empty"),
        };

        let cbor = manifest.to_cbor().unwrap();
        let decoded = BinaryManifest::from_cbor(&cbor).unwrap();

        assert_eq!(decoded.name, "test-package");
        assert_eq!(decoded.version, "1.0.0");
        assert_eq!(decoded.format_version, FORMAT_VERSION);
    }

    #[test]
    fn test_merkle_root_empty() {
        let components = BTreeMap::new();
        let root = MerkleTree::calculate_root(&components);
        assert_eq!(root.algorithm, "sha256");
        // Empty tree should have consistent hash
        assert!(!root.value.is_empty());
    }

    #[test]
    fn test_merkle_root_single_component() {
        let mut components = BTreeMap::new();
        components.insert(
            "runtime".to_string(),
            ComponentRef {
                hash: Hash::sha256(b"runtime content"),
                file_count: 5,
                total_size: 1000,
                default: true,
            },
        );

        let root = MerkleTree::calculate_root(&components);
        assert!(MerkleTree::verify_root(&components, &root));
    }

    #[test]
    fn test_merkle_root_multiple_components() {
        let mut components = BTreeMap::new();
        components.insert(
            "runtime".to_string(),
            ComponentRef {
                hash: Hash::sha256(b"runtime"),
                file_count: 5,
                total_size: 1000,
                default: true,
            },
        );
        components.insert(
            "devel".to_string(),
            ComponentRef {
                hash: Hash::sha256(b"devel"),
                file_count: 10,
                total_size: 2000,
                default: false,
            },
        );
        components.insert(
            "doc".to_string(),
            ComponentRef {
                hash: Hash::sha256(b"doc"),
                file_count: 3,
                total_size: 500,
                default: false,
            },
        );

        let root = MerkleTree::calculate_root(&components);
        assert!(MerkleTree::verify_root(&components, &root));

        // Modifying a component should change the root
        let mut modified = components.clone();
        modified.get_mut("runtime").unwrap().file_count = 6;
        // Note: file_count doesn't affect hash, only the component hash does
        // So we need to change the hash
        modified.get_mut("runtime").unwrap().hash = Hash::sha256(b"modified runtime");
        assert!(!MerkleTree::verify_root(&modified, &root));
    }

    #[test]
    fn test_merkle_root_order_independent() {
        // BTreeMap ensures consistent ordering
        let mut comp1 = BTreeMap::new();
        comp1.insert("a".to_string(), ComponentRef {
            hash: Hash::sha256(b"a"),
            file_count: 1,
            total_size: 100,
            default: true,
        });
        comp1.insert("b".to_string(), ComponentRef {
            hash: Hash::sha256(b"b"),
            file_count: 1,
            total_size: 100,
            default: true,
        });

        let mut comp2 = BTreeMap::new();
        comp2.insert("b".to_string(), ComponentRef {
            hash: Hash::sha256(b"b"),
            file_count: 1,
            total_size: 100,
            default: true,
        });
        comp2.insert("a".to_string(), ComponentRef {
            hash: Hash::sha256(b"a"),
            file_count: 1,
            total_size: 100,
            default: true,
        });

        let root1 = MerkleTree::calculate_root(&comp1);
        let root2 = MerkleTree::calculate_root(&comp2);

        assert_eq!(root1, root2);
    }
}
