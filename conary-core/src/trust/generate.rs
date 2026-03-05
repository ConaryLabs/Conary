// conary-core/src/trust/generate.rs

//! Server-side TUF metadata generation
//!
//! Generates signed TUF metadata for repositories managed by the Remi server.
//! Feature-gated behind `server` to avoid pulling in unnecessary code on clients.

use crate::ccs::signing::SigningKeyPair;
use crate::trust::TrustResult;
use crate::trust::keys::sign_tuf_metadata;
use crate::trust::metadata::{
    MetaFile, Signed, SnapshotMetadata, TUF_SPEC_VERSION, TargetDescription, TargetsMetadata,
    TimestampMetadata,
};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Generate signed targets metadata from a list of packages
pub fn generate_targets(
    packages: &[(String, u64, String)], // (path, size, sha256)
    key: &SigningKeyPair,
    version: u64,
    expires_days: i64,
) -> TrustResult<Signed<TargetsMetadata>> {
    let expires = Utc::now() + Duration::days(expires_days);

    let mut targets = BTreeMap::new();
    for (path, size, sha256) in packages {
        let mut hashes = BTreeMap::new();
        hashes.insert("sha256".to_string(), sha256.clone());
        targets.insert(
            path.clone(),
            TargetDescription {
                length: *size,
                hashes,
            },
        );
    }

    let metadata = TargetsMetadata {
        type_field: "targets".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version,
        expires,
        targets,
    };

    let sig = sign_tuf_metadata(key, &metadata);
    Ok(Signed {
        signed: metadata,
        signatures: vec![sig],
    })
}

/// Generate signed snapshot metadata pinning root and targets versions
pub fn generate_snapshot(
    root_version: u64,
    targets: &Signed<TargetsMetadata>,
    key: &SigningKeyPair,
    version: u64,
    expires_days: i64,
) -> TrustResult<Signed<SnapshotMetadata>> {
    let expires = Utc::now() + Duration::days(expires_days);

    // Hash the targets metadata
    let targets_json = serde_json::to_vec(targets)?;
    let targets_hash = hex::encode(Sha256::digest(&targets_json));

    let mut meta = BTreeMap::new();
    meta.insert(
        "root.json".to_string(),
        MetaFile {
            version: root_version,
            length: None,
            hashes: None,
        },
    );
    meta.insert(
        "targets.json".to_string(),
        MetaFile {
            version: targets.signed.version,
            length: Some(targets_json.len() as u64),
            hashes: Some({
                let mut h = BTreeMap::new();
                h.insert("sha256".to_string(), targets_hash);
                h
            }),
        },
    );

    let metadata = SnapshotMetadata {
        type_field: "snapshot".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version,
        expires,
        meta,
    };

    let sig = sign_tuf_metadata(key, &metadata);
    Ok(Signed {
        signed: metadata,
        signatures: vec![sig],
    })
}

/// Generate signed timestamp metadata pinning the current snapshot
pub fn generate_timestamp(
    snapshot: &Signed<SnapshotMetadata>,
    key: &SigningKeyPair,
    version: u64,
    expires_hours: i64,
) -> TrustResult<Signed<TimestampMetadata>> {
    let expires = Utc::now() + Duration::hours(expires_hours);

    // Hash the snapshot metadata
    let snapshot_json = serde_json::to_vec(snapshot)?;
    let snapshot_hash = hex::encode(Sha256::digest(&snapshot_json));

    let mut meta = BTreeMap::new();
    meta.insert(
        "snapshot.json".to_string(),
        MetaFile {
            version: snapshot.signed.version,
            length: Some(snapshot_json.len() as u64),
            hashes: Some({
                let mut h = BTreeMap::new();
                h.insert("sha256".to_string(), snapshot_hash);
                h
            }),
        },
    );

    let metadata = TimestampMetadata {
        type_field: "timestamp".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version,
        expires,
        meta,
    };

    let sig = sign_tuf_metadata(key, &metadata);
    Ok(Signed {
        signed: metadata,
        signatures: vec![sig],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_targets() {
        let key = SigningKeyPair::generate();
        let packages = vec![
            (
                "packages/nginx-1.24.0.ccs".to_string(),
                4096,
                "abc123".to_string(),
            ),
            (
                "packages/curl-8.5.0.ccs".to_string(),
                2048,
                "def456".to_string(),
            ),
        ];

        let targets = generate_targets(&packages, &key, 1, 30).unwrap();

        assert_eq!(targets.signed.version, 1);
        assert_eq!(targets.signed.targets.len(), 2);
        assert!(
            targets
                .signed
                .targets
                .contains_key("packages/nginx-1.24.0.ccs")
        );
        assert_eq!(targets.signatures.len(), 1);
    }

    #[test]
    fn test_generate_snapshot() {
        let key = SigningKeyPair::generate();
        let packages = vec![("pkg.ccs".to_string(), 100, "hash".to_string())];
        let targets = generate_targets(&packages, &key, 3, 30).unwrap();

        let snapshot = generate_snapshot(1, &targets, &key, 5, 7).unwrap();

        assert_eq!(snapshot.signed.version, 5);
        assert!(snapshot.signed.meta.contains_key("root.json"));
        assert!(snapshot.signed.meta.contains_key("targets.json"));
        assert_eq!(snapshot.signed.meta["root.json"].version, 1);
        assert_eq!(snapshot.signed.meta["targets.json"].version, 3);
    }

    #[test]
    fn test_generate_timestamp() {
        let key = SigningKeyPair::generate();
        let packages = vec![("pkg.ccs".to_string(), 100, "hash".to_string())];
        let targets = generate_targets(&packages, &key, 1, 30).unwrap();
        let snapshot = generate_snapshot(1, &targets, &key, 1, 7).unwrap();

        let timestamp = generate_timestamp(&snapshot, &key, 42, 6).unwrap();

        assert_eq!(timestamp.signed.version, 42);
        assert!(timestamp.signed.meta.contains_key("snapshot.json"));
        assert_eq!(timestamp.signed.meta["snapshot.json"].version, 1);
        // Timestamp should have hashes for snapshot
        assert!(timestamp.signed.meta["snapshot.json"].hashes.is_some());
    }

    #[test]
    fn test_full_metadata_chain() {
        let key = SigningKeyPair::generate();

        // Generate a full chain: targets -> snapshot -> timestamp
        let packages = vec![
            ("nginx.ccs".to_string(), 4096, "aaa".to_string()),
            ("curl.ccs".to_string(), 2048, "bbb".to_string()),
        ];

        let targets = generate_targets(&packages, &key, 1, 30).unwrap();
        let snapshot = generate_snapshot(1, &targets, &key, 1, 7).unwrap();
        let timestamp = generate_timestamp(&snapshot, &key, 1, 6).unwrap();

        // Verify the chain is consistent
        assert_eq!(snapshot.signed.meta["targets.json"].version, 1);
        assert_eq!(timestamp.signed.meta["snapshot.json"].version, 1);
    }
}
