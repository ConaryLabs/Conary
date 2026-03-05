// conary-core/src/trust/verify.rs

//! TUF signature and metadata verification
//!
//! Provides the core verification primitives for TUF trust:
//! - Signature threshold verification (enough valid signatures?)
//! - Version monotonicity (rollback protection)
//! - Expiration checking (freeze protection)
//! - Snapshot consistency (mix-and-match protection)

use crate::trust::keys::canonical_json;
use crate::trust::metadata::{MetaFile, Role, RootMetadata, Signed, SnapshotMetadata, TufKey};
use crate::trust::{TrustError, TrustResult};
use chrono::Utc;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::Digest;
use std::collections::BTreeMap;

/// Verify that a signed metadata document has enough valid signatures
///
/// Checks each signature against the provided keys and verifies that
/// the threshold is met.
pub fn verify_signatures<T: serde::Serialize>(
    signed: &Signed<T>,
    role: Role,
    keys: &BTreeMap<String, TufKey>,
    threshold: u64,
) -> TrustResult<()> {
    let canonical = canonical_json(&signed.signed);
    let mut valid_count: u64 = 0;
    let mut seen_keyids = std::collections::HashSet::new();

    for sig in &signed.signatures {
        // Skip duplicate key IDs
        if !seen_keyids.insert(&sig.keyid) {
            continue;
        }

        // Look up the key
        let Some(tuf_key) = keys.get(&sig.keyid) else {
            continue;
        };

        // Only support ed25519
        if tuf_key.keytype != "ed25519" {
            continue;
        }

        // Verify the signature
        if verify_ed25519_signature(&canonical, &sig.sig, &tuf_key.keyval.public).is_ok() {
            valid_count += 1;
        }
    }

    if valid_count >= threshold {
        Ok(())
    } else {
        Err(TrustError::ThresholdNotMet {
            role: role.to_string(),
            threshold,
            got: valid_count,
        })
    }
}

/// Verify a single Ed25519 signature
fn verify_ed25519_signature(
    message: &[u8],
    sig_hex: &str,
    public_key_hex: &str,
) -> TrustResult<()> {
    let public_bytes = hex::decode(public_key_hex)
        .map_err(|e| TrustError::KeyError(format!("Invalid public key hex: {e}")))?;

    let public_key_bytes: [u8; 32] = public_bytes
        .try_into()
        .map_err(|_| TrustError::KeyError("Public key must be 32 bytes".to_string()))?;

    let verifying_key = VerifyingKey::from_bytes(&public_key_bytes)
        .map_err(|e| TrustError::KeyError(format!("Invalid Ed25519 public key: {e}")))?;

    let sig_bytes = hex::decode(sig_hex)
        .map_err(|e| TrustError::VerificationFailed(format!("Invalid signature hex: {e}")))?;

    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| TrustError::VerificationFailed(format!("Invalid signature: {e}")))?;

    verifying_key
        .verify(message, &signature)
        .map_err(|_| TrustError::VerificationFailed("Signature verification failed".to_string()))
}

/// Verify that a metadata version is strictly increasing (rollback protection)
pub fn verify_version_increase(
    role: Role,
    new_version: u64,
    stored_version: u64,
) -> TrustResult<()> {
    if new_version > stored_version {
        Ok(())
    } else {
        Err(TrustError::RollbackAttack {
            role: role.to_string(),
            new: new_version,
            stored: stored_version,
        })
    }
}

/// Verify that metadata has not expired (freeze protection)
pub fn verify_not_expired(role: Role, expires: &chrono::DateTime<Utc>) -> TrustResult<()> {
    if Utc::now() < *expires {
        Ok(())
    } else {
        Err(TrustError::MetadataExpired {
            role: role.to_string(),
            expires: expires.to_rfc3339(),
        })
    }
}

/// Verify snapshot consistency with root and targets versions
///
/// Ensures the snapshot pins the expected versions of other metadata,
/// preventing mix-and-match attacks.
pub fn verify_snapshot_consistency(
    snapshot: &SnapshotMetadata,
    expected_root_version: u64,
    expected_targets_version: Option<u64>,
) -> TrustResult<()> {
    // Check root version in snapshot
    if let Some(root_meta) = snapshot.meta.get("root.json")
        && root_meta.version != expected_root_version
    {
        return Err(TrustError::ConsistencyError(format!(
            "Snapshot pins root.json v{} but expected v{}",
            root_meta.version, expected_root_version
        )));
    }

    // Check targets version if provided
    if let Some(expected_tv) = expected_targets_version
        && let Some(targets_meta) = snapshot.meta.get("targets.json")
        && targets_meta.version != expected_tv
    {
        return Err(TrustError::ConsistencyError(format!(
            "Snapshot pins targets.json v{} but expected v{}",
            targets_meta.version, expected_tv
        )));
    }

    Ok(())
}

/// Verify that a hash matches the expected value from a MetaFile reference
pub fn verify_metadata_hash(meta_ref: &MetaFile, actual_bytes: &[u8]) -> TrustResult<()> {
    if let Some(ref hashes) = meta_ref.hashes
        && let Some(expected_sha256) = hashes.get("sha256")
    {
        let actual_hash = hex::encode(sha2::Sha256::digest(actual_bytes));
        if actual_hash != *expected_sha256 {
            return Err(TrustError::ConsistencyError(format!(
                "Hash mismatch: expected {expected_sha256}, got {actual_hash}"
            )));
        }
    }
    Ok(())
}

/// Extract keys and threshold for a specific role from root metadata
pub fn extract_role_keys(
    root: &RootMetadata,
    role: Role,
) -> TrustResult<(BTreeMap<String, TufKey>, u64)> {
    let role_name = role.to_string();
    let role_def = root.roles.get(&role_name).ok_or_else(|| {
        TrustError::ConsistencyError(format!(
            "Root metadata missing role definition: {role_name}"
        ))
    })?;

    let mut role_keys = BTreeMap::new();
    for keyid in &role_def.keyids {
        if let Some(key) = root.keys.get(keyid) {
            role_keys.insert(keyid.clone(), key.clone());
        }
    }

    if role_keys.is_empty() {
        return Err(TrustError::KeyError(format!(
            "No keys found for role {role_name}"
        )));
    }

    Ok((role_keys, role_def.threshold))
}

/// Verify root metadata self-signatures using its own keys
///
/// Root is special: it's verified against both the old root keys
/// (from the previously trusted root) and its own new keys.
pub fn verify_root(
    signed_root: &Signed<RootMetadata>,
    trusted_keys: &BTreeMap<String, TufKey>,
    trusted_threshold: u64,
) -> TrustResult<()> {
    // Verify signatures against the trusted keys
    verify_signatures(signed_root, Role::Root, trusted_keys, trusted_threshold)?;

    // Also verify the root is self-signed (new keys must also meet threshold)
    let (new_keys, new_threshold) = extract_role_keys(&signed_root.signed, Role::Root)?;
    verify_signatures(signed_root, Role::Root, &new_keys, new_threshold)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::signing::SigningKeyPair;
    use crate::trust::keys::{sign_tuf_metadata, signing_keypair_to_tuf_key};
    use crate::trust::metadata::*;
    use chrono::Duration;

    fn make_test_root(
        keypair: &SigningKeyPair,
        version: u64,
        expires: chrono::DateTime<Utc>,
    ) -> Signed<RootMetadata> {
        let (key_id, tuf_key) = signing_keypair_to_tuf_key(keypair);

        let mut keys = BTreeMap::new();
        keys.insert(key_id.clone(), tuf_key);

        let mut roles = BTreeMap::new();
        for role_name in &["root", "targets", "snapshot", "timestamp"] {
            roles.insert(
                role_name.to_string(),
                RoleDefinition {
                    keyids: vec![key_id.clone()],
                    threshold: 1,
                },
            );
        }

        let root = RootMetadata {
            type_field: "root".to_string(),
            spec_version: TUF_SPEC_VERSION.to_string(),
            version,
            expires,
            consistent_snapshot: false,
            keys,
            roles,
        };

        let sig = sign_tuf_metadata(keypair, &root);
        Signed {
            signed: root,
            signatures: vec![sig],
        }
    }

    #[test]
    fn test_verify_signatures_valid() {
        let keypair = SigningKeyPair::generate();
        let expires = Utc::now() + Duration::days(365);
        let signed_root = make_test_root(&keypair, 1, expires);

        let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair);
        let mut keys = BTreeMap::new();
        keys.insert(key_id, tuf_key);

        assert!(verify_signatures(&signed_root, Role::Root, &keys, 1).is_ok());
    }

    #[test]
    fn test_verify_signatures_threshold_not_met() {
        let keypair = SigningKeyPair::generate();
        let expires = Utc::now() + Duration::days(365);
        let signed_root = make_test_root(&keypair, 1, expires);

        let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair);
        let mut keys = BTreeMap::new();
        keys.insert(key_id, tuf_key);

        // Require 2 signatures but only have 1
        let result = verify_signatures(&signed_root, Role::Root, &keys, 2);
        assert!(result.is_err());
        assert!(matches!(result, Err(TrustError::ThresholdNotMet { .. })));
    }

    #[test]
    fn test_verify_signatures_wrong_key() {
        let keypair1 = SigningKeyPair::generate();
        let keypair2 = SigningKeyPair::generate();
        let expires = Utc::now() + Duration::days(365);

        // Sign with keypair1
        let signed_root = make_test_root(&keypair1, 1, expires);

        // But verify with keypair2's key
        let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair2);
        let mut keys = BTreeMap::new();
        keys.insert(key_id, tuf_key);

        let result = verify_signatures(&signed_root, Role::Root, &keys, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_version_increase_ok() {
        assert!(verify_version_increase(Role::Timestamp, 2, 1).is_ok());
        assert!(verify_version_increase(Role::Timestamp, 100, 99).is_ok());
    }

    #[test]
    fn test_verify_version_increase_rollback() {
        let result = verify_version_increase(Role::Timestamp, 1, 2);
        assert!(matches!(result, Err(TrustError::RollbackAttack { .. })));

        // Equal version is also a rollback
        let result = verify_version_increase(Role::Timestamp, 5, 5);
        assert!(matches!(result, Err(TrustError::RollbackAttack { .. })));
    }

    #[test]
    fn test_verify_not_expired_ok() {
        let future = Utc::now() + Duration::hours(1);
        assert!(verify_not_expired(Role::Timestamp, &future).is_ok());
    }

    #[test]
    fn test_verify_not_expired_expired() {
        let past = Utc::now() - Duration::hours(1);
        let result = verify_not_expired(Role::Timestamp, &past);
        assert!(matches!(result, Err(TrustError::MetadataExpired { .. })));
    }

    #[test]
    fn test_verify_snapshot_consistency_ok() {
        let expires = Utc::now() + Duration::days(7);
        let mut snapshot = SnapshotMetadata {
            type_field: "snapshot".to_string(),
            spec_version: TUF_SPEC_VERSION.to_string(),
            version: 1,
            expires,
            meta: BTreeMap::new(),
        };

        snapshot.meta.insert(
            "root.json".to_string(),
            MetaFile {
                version: 3,
                length: None,
                hashes: None,
            },
        );
        snapshot.meta.insert(
            "targets.json".to_string(),
            MetaFile {
                version: 5,
                length: None,
                hashes: None,
            },
        );

        assert!(verify_snapshot_consistency(&snapshot, 3, Some(5)).is_ok());
    }

    #[test]
    fn test_verify_snapshot_consistency_root_mismatch() {
        let expires = Utc::now() + Duration::days(7);
        let mut snapshot = SnapshotMetadata {
            type_field: "snapshot".to_string(),
            spec_version: TUF_SPEC_VERSION.to_string(),
            version: 1,
            expires,
            meta: BTreeMap::new(),
        };

        snapshot.meta.insert(
            "root.json".to_string(),
            MetaFile {
                version: 2,
                length: None,
                hashes: None,
            },
        );

        let result = verify_snapshot_consistency(&snapshot, 3, None);
        assert!(matches!(result, Err(TrustError::ConsistencyError(_))));
    }

    #[test]
    fn test_verify_metadata_hash_ok() {
        let data = b"test metadata content";
        let hash = hex::encode(sha2::Sha256::digest(data));
        let mut hashes = BTreeMap::new();
        hashes.insert("sha256".to_string(), hash);

        let meta_ref = MetaFile {
            version: 1,
            length: None,
            hashes: Some(hashes),
        };

        assert!(verify_metadata_hash(&meta_ref, data).is_ok());
    }

    #[test]
    fn test_verify_metadata_hash_mismatch() {
        let mut hashes = BTreeMap::new();
        hashes.insert("sha256".to_string(), "wrong_hash".to_string());

        let meta_ref = MetaFile {
            version: 1,
            length: None,
            hashes: Some(hashes),
        };

        let result = verify_metadata_hash(&meta_ref, b"test");
        assert!(matches!(result, Err(TrustError::ConsistencyError(_))));
    }

    #[test]
    fn test_verify_metadata_hash_no_hash_is_ok() {
        let meta_ref = MetaFile {
            version: 1,
            length: None,
            hashes: None,
        };

        // No hash to check = passes
        assert!(verify_metadata_hash(&meta_ref, b"anything").is_ok());
    }

    #[test]
    fn test_extract_role_keys() {
        let keypair = SigningKeyPair::generate();
        let expires = Utc::now() + Duration::days(365);
        let signed_root = make_test_root(&keypair, 1, expires);

        let (keys, threshold) = extract_role_keys(&signed_root.signed, Role::Targets).unwrap();
        assert_eq!(threshold, 1);
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn test_verify_root_self_signed() {
        let keypair = SigningKeyPair::generate();
        let expires = Utc::now() + Duration::days(365);
        let signed_root = make_test_root(&keypair, 1, expires);

        let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair);
        let mut trusted_keys = BTreeMap::new();
        trusted_keys.insert(key_id, tuf_key);

        assert!(verify_root(&signed_root, &trusted_keys, 1).is_ok());
    }
}
