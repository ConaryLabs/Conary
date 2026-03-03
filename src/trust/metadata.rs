// src/trust/metadata.rs

//! TUF metadata types
//!
//! Implements the core metadata structures from the TUF specification (v1.0.31).
//! All metadata types use `BTreeMap` for deterministic JSON serialization.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// TUF specification version
pub const TUF_SPEC_VERSION: &str = "1.0.31";

/// Signed TUF metadata wrapper
///
/// Contains the signed payload and its signatures, per TUF spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signed<T> {
    /// The metadata payload
    pub signed: T,
    /// Signatures over the canonical JSON of `signed`
    pub signatures: Vec<TufSignature>,
}

/// A single signature on TUF metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TufSignature {
    /// Key ID that produced this signature (SHA-256 of canonical key JSON)
    pub keyid: String,
    /// Hex-encoded Ed25519 signature
    pub sig: String,
}

/// Root metadata - the trust anchor
///
/// Contains the keys and thresholds for all top-level roles.
/// Root is self-signed and forms the basis of the trust chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootMetadata {
    /// Role type identifier
    #[serde(rename = "_type")]
    pub type_field: String,
    /// TUF specification version
    pub spec_version: String,
    /// Metadata version (monotonically increasing)
    pub version: u64,
    /// Expiration timestamp
    pub expires: DateTime<Utc>,
    /// Whether consistent snapshots are used
    pub consistent_snapshot: bool,
    /// All keys referenced by roles, keyed by key ID
    pub keys: BTreeMap<String, TufKey>,
    /// Role definitions with key IDs and thresholds
    pub roles: BTreeMap<String, RoleDefinition>,
}

/// Targets metadata - describes available packages
///
/// Maps target paths to their hashes and lengths for verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetsMetadata {
    /// Role type identifier
    #[serde(rename = "_type")]
    pub type_field: String,
    /// TUF specification version
    pub spec_version: String,
    /// Metadata version (monotonically increasing)
    pub version: u64,
    /// Expiration timestamp
    pub expires: DateTime<Utc>,
    /// Target files with their hashes and lengths
    pub targets: BTreeMap<String, TargetDescription>,
    // delegations: Option<Delegations>,  // Phase 2
}

/// Snapshot metadata - versions of all other metadata
///
/// Records the version of every metadata file for consistency checking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    /// Role type identifier
    #[serde(rename = "_type")]
    pub type_field: String,
    /// TUF specification version
    pub spec_version: String,
    /// Metadata version (monotonically increasing)
    pub version: u64,
    /// Expiration timestamp
    pub expires: DateTime<Utc>,
    /// Metadata file versions
    pub meta: BTreeMap<String, MetaFile>,
}

/// Timestamp metadata - freshness indicator
///
/// The most frequently updated metadata, pointing to the current snapshot.
/// Short expiry ensures clients detect freeze attacks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimestampMetadata {
    /// Role type identifier
    #[serde(rename = "_type")]
    pub type_field: String,
    /// TUF specification version
    pub spec_version: String,
    /// Metadata version (monotonically increasing)
    pub version: u64,
    /// Expiration timestamp
    pub expires: DateTime<Utc>,
    /// Points to the current snapshot metadata
    pub meta: BTreeMap<String, MetaFile>,
}

/// A TUF public key
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TufKey {
    /// Key type (e.g., "ed25519")
    pub keytype: String,
    /// Signing scheme (e.g., "ed25519")
    pub scheme: String,
    /// Key value container
    pub keyval: KeyVal,
}

/// Key value container holding the public key
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyVal {
    /// Hex-encoded public key bytes
    pub public: String,
}

/// Role definition specifying which keys can sign and the threshold
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDefinition {
    /// Key IDs authorized to sign for this role
    pub keyids: Vec<String>,
    /// Minimum number of valid signatures required
    pub threshold: u64,
}

/// Description of a target file (package)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetDescription {
    /// File length in bytes
    pub length: u64,
    /// Hash algorithm to hex digest mapping
    pub hashes: BTreeMap<String, String>,
}

/// Metadata file reference in snapshot/timestamp
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaFile {
    /// Version of the referenced metadata
    pub version: u64,
    /// Optional file length
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    /// Optional hash digests
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hashes: Option<BTreeMap<String, String>>,
}

/// State returned after a successful TUF update
#[derive(Debug)]
pub struct VerifiedTufState {
    /// Verified root metadata version
    pub root_version: u64,
    /// Verified targets metadata version
    pub targets_version: u64,
    /// Verified snapshot metadata version
    pub snapshot_version: u64,
    /// Verified timestamp metadata version
    pub timestamp_version: u64,
    /// Verified target descriptions
    pub targets: BTreeMap<String, TargetDescription>,
}

/// TUF roles
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Root role - trust anchor
    Root,
    /// Targets role - package metadata
    Targets,
    /// Snapshot role - metadata versioning
    Snapshot,
    /// Timestamp role - freshness
    Timestamp,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => write!(f, "root"),
            Self::Targets => write!(f, "targets"),
            Self::Snapshot => write!(f, "snapshot"),
            Self::Timestamp => write!(f, "timestamp"),
        }
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "root" => Ok(Self::Root),
            "targets" => Ok(Self::Targets),
            "snapshot" => Ok(Self::Snapshot),
            "timestamp" => Ok(Self::Timestamp),
            other => Err(format!("unknown TUF role: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_role_display() {
        assert_eq!(Role::Root.to_string(), "root");
        assert_eq!(Role::Targets.to_string(), "targets");
        assert_eq!(Role::Snapshot.to_string(), "snapshot");
        assert_eq!(Role::Timestamp.to_string(), "timestamp");
    }

    #[test]
    fn test_role_from_str() {
        assert_eq!(Role::from_str("root").unwrap(), Role::Root);
        assert_eq!(Role::from_str("targets").unwrap(), Role::Targets);
        assert_eq!(Role::from_str("snapshot").unwrap(), Role::Snapshot);
        assert_eq!(Role::from_str("timestamp").unwrap(), Role::Timestamp);
        assert!(Role::from_str("invalid").is_err());
    }

    #[test]
    fn test_role_roundtrip() {
        for role in &[Role::Root, Role::Targets, Role::Snapshot, Role::Timestamp] {
            let s = role.to_string();
            let parsed = Role::from_str(&s).unwrap();
            assert_eq!(*role, parsed);
        }
    }

    #[test]
    fn test_root_metadata_serialization_roundtrip() {
        let expires = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let key = TufKey {
            keytype: "ed25519".to_string(),
            scheme: "ed25519".to_string(),
            keyval: KeyVal {
                public: "abcdef0123456789".to_string(),
            },
        };

        let mut keys = BTreeMap::new();
        keys.insert("key1".to_string(), key);

        let mut roles = BTreeMap::new();
        roles.insert(
            "root".to_string(),
            RoleDefinition {
                keyids: vec!["key1".to_string()],
                threshold: 1,
            },
        );

        let root = RootMetadata {
            type_field: "root".to_string(),
            spec_version: TUF_SPEC_VERSION.to_string(),
            version: 1,
            expires,
            consistent_snapshot: true,
            keys,
            roles,
        };

        let json = serde_json::to_string(&root).unwrap();
        let deserialized: RootMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.type_field, "root");
        assert_eq!(deserialized.version, 1);
        assert_eq!(deserialized.spec_version, TUF_SPEC_VERSION);
        assert!(deserialized.consistent_snapshot);
        assert_eq!(deserialized.keys.len(), 1);
        assert_eq!(deserialized.roles.len(), 1);
        assert_eq!(deserialized.roles["root"].threshold, 1);
    }

    #[test]
    fn test_type_field_serializes_as_underscore_type() {
        let expires = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let root = RootMetadata {
            type_field: "root".to_string(),
            spec_version: TUF_SPEC_VERSION.to_string(),
            version: 1,
            expires,
            consistent_snapshot: false,
            keys: BTreeMap::new(),
            roles: BTreeMap::new(),
        };

        let json = serde_json::to_string(&root).unwrap();
        assert!(json.contains("\"_type\":\"root\"") || json.contains("\"_type\": \"root\""));
        assert!(!json.contains("\"type_field\""));
    }

    #[test]
    fn test_targets_metadata_serialization_roundtrip() {
        let expires = Utc.with_ymd_and_hms(2030, 6, 15, 12, 0, 0).unwrap();
        let mut hashes = BTreeMap::new();
        hashes.insert(
            "sha256".to_string(),
            "abc123def456".to_string(),
        );

        let mut targets = BTreeMap::new();
        targets.insert(
            "packages/nginx-1.24.0.ccs".to_string(),
            TargetDescription {
                length: 4096,
                hashes,
            },
        );

        let meta = TargetsMetadata {
            type_field: "targets".to_string(),
            spec_version: TUF_SPEC_VERSION.to_string(),
            version: 5,
            expires,
            targets,
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: TargetsMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.type_field, "targets");
        assert_eq!(deserialized.version, 5);
        assert_eq!(deserialized.targets.len(), 1);
        let target = &deserialized.targets["packages/nginx-1.24.0.ccs"];
        assert_eq!(target.length, 4096);
        assert_eq!(target.hashes["sha256"], "abc123def456");
    }

    #[test]
    fn test_snapshot_metadata_serialization_roundtrip() {
        let expires = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let mut meta_files = BTreeMap::new();
        meta_files.insert(
            "targets.json".to_string(),
            MetaFile {
                version: 3,
                length: Some(1024),
                hashes: None,
            },
        );

        let snapshot = SnapshotMetadata {
            type_field: "snapshot".to_string(),
            spec_version: TUF_SPEC_VERSION.to_string(),
            version: 7,
            expires,
            meta: meta_files,
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        let deserialized: SnapshotMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.version, 7);
        assert_eq!(deserialized.meta["targets.json"].version, 3);
        assert_eq!(deserialized.meta["targets.json"].length, Some(1024));
    }

    #[test]
    fn test_timestamp_metadata_serialization_roundtrip() {
        let expires = Utc.with_ymd_and_hms(2030, 1, 2, 0, 0, 0).unwrap();
        let mut hashes = BTreeMap::new();
        hashes.insert("sha256".to_string(), "deadbeef".to_string());

        let mut meta_files = BTreeMap::new();
        meta_files.insert(
            "snapshot.json".to_string(),
            MetaFile {
                version: 7,
                length: Some(512),
                hashes: Some(hashes),
            },
        );

        let timestamp = TimestampMetadata {
            type_field: "timestamp".to_string(),
            spec_version: TUF_SPEC_VERSION.to_string(),
            version: 42,
            expires,
            meta: meta_files,
        };

        let json = serde_json::to_string(&timestamp).unwrap();
        let deserialized: TimestampMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.version, 42);
        let snap_meta = &deserialized.meta["snapshot.json"];
        assert_eq!(snap_meta.version, 7);
        assert_eq!(
            snap_meta.hashes.as_ref().unwrap()["sha256"],
            "deadbeef"
        );
    }

    #[test]
    fn test_signed_wrapper_serialization() {
        let expires = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let timestamp = TimestampMetadata {
            type_field: "timestamp".to_string(),
            spec_version: TUF_SPEC_VERSION.to_string(),
            version: 1,
            expires,
            meta: BTreeMap::new(),
        };

        let signed = Signed {
            signed: timestamp,
            signatures: vec![TufSignature {
                keyid: "abc123".to_string(),
                sig: "deadbeef".to_string(),
            }],
        };

        let json = serde_json::to_string(&signed).unwrap();
        let deserialized: Signed<TimestampMetadata> = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.signatures.len(), 1);
        assert_eq!(deserialized.signatures[0].keyid, "abc123");
        assert_eq!(deserialized.signatures[0].sig, "deadbeef");
        assert_eq!(deserialized.signed.version, 1);
    }

    #[test]
    fn test_meta_file_optional_fields_omitted() {
        let mf = MetaFile {
            version: 1,
            length: None,
            hashes: None,
        };

        let json = serde_json::to_string(&mf).unwrap();
        assert!(!json.contains("length"));
        assert!(!json.contains("hashes"));
    }

    #[test]
    fn test_tuf_key_equality() {
        let key1 = TufKey {
            keytype: "ed25519".to_string(),
            scheme: "ed25519".to_string(),
            keyval: KeyVal {
                public: "aabbccdd".to_string(),
            },
        };
        let key2 = key1.clone();
        assert_eq!(key1, key2);

        let key3 = TufKey {
            keytype: "ed25519".to_string(),
            scheme: "ed25519".to_string(),
            keyval: KeyVal {
                public: "different".to_string(),
            },
        };
        assert_ne!(key1, key3);
    }
}
