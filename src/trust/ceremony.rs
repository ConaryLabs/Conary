// src/trust/ceremony.rs

//! TUF key generation and rotation ceremony helpers
//!
//! Provides utilities for generating TUF key pairs, creating initial
//! root metadata, and performing key rotation.

use crate::ccs::signing::SigningKeyPair;
use crate::trust::keys::{sign_tuf_metadata, signing_keypair_to_tuf_key};
use crate::trust::metadata::{
    RoleDefinition, RootMetadata, Signed, TUF_SPEC_VERSION,
};
use crate::trust::TrustResult;
use chrono::{Duration, Utc};
use std::collections::BTreeMap;
use std::path::Path;

/// Generate a new Ed25519 key pair for a TUF role
///
/// Saves the private and public keys to the specified directory.
pub fn generate_role_key(
    role: &str,
    output_dir: &Path,
) -> anyhow::Result<SigningKeyPair> {
    let keypair = SigningKeyPair::generate().with_key_id(role);

    let private_path = output_dir.join(format!("{role}.private"));
    let public_path = output_dir.join(format!("{role}.public"));

    keypair.save_to_files(&private_path, &public_path)?;

    Ok(keypair)
}

/// Create initial root metadata for a repository
///
/// Uses separate keys for each role (recommended) or a single key for all roles.
pub fn create_initial_root(
    root_key: &SigningKeyPair,
    targets_key: &SigningKeyPair,
    snapshot_key: &SigningKeyPair,
    timestamp_key: &SigningKeyPair,
    expires_days: i64,
) -> TrustResult<Signed<RootMetadata>> {
    let expires = Utc::now() + Duration::days(expires_days);

    let (root_key_id, root_tuf_key) = signing_keypair_to_tuf_key(root_key);
    let (targets_key_id, targets_tuf_key) = signing_keypair_to_tuf_key(targets_key);
    let (snapshot_key_id, snapshot_tuf_key) = signing_keypair_to_tuf_key(snapshot_key);
    let (timestamp_key_id, timestamp_tuf_key) = signing_keypair_to_tuf_key(timestamp_key);

    let mut keys = BTreeMap::new();
    keys.insert(root_key_id.clone(), root_tuf_key);
    keys.insert(targets_key_id.clone(), targets_tuf_key);
    keys.insert(snapshot_key_id.clone(), snapshot_tuf_key);
    keys.insert(timestamp_key_id.clone(), timestamp_tuf_key);

    let mut roles = BTreeMap::new();
    roles.insert(
        "root".to_string(),
        RoleDefinition {
            keyids: vec![root_key_id],
            threshold: 1,
        },
    );
    roles.insert(
        "targets".to_string(),
        RoleDefinition {
            keyids: vec![targets_key_id],
            threshold: 1,
        },
    );
    roles.insert(
        "snapshot".to_string(),
        RoleDefinition {
            keyids: vec![snapshot_key_id],
            threshold: 1,
        },
    );
    roles.insert(
        "timestamp".to_string(),
        RoleDefinition {
            keyids: vec![timestamp_key_id],
            threshold: 1,
        },
    );

    let root = RootMetadata {
        type_field: "root".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: 1,
        expires,
        consistent_snapshot: false,
        keys,
        roles,
    };

    let sig = sign_tuf_metadata(root_key, &root);
    Ok(Signed {
        signed: root,
        signatures: vec![sig],
    })
}

/// Create initial root metadata using a single key for all roles
///
/// Simpler setup for development and small repositories.
pub fn create_initial_root_single_key(
    key: &SigningKeyPair,
    expires_days: i64,
) -> TrustResult<Signed<RootMetadata>> {
    create_initial_root(key, key, key, key, expires_days)
}

/// Rotate a key in root metadata
///
/// Creates a new root version with the old key replaced by the new key.
/// The new root must be signed by both the old root key and the new root key.
pub fn rotate_key(
    current_root: &Signed<RootMetadata>,
    role_name: &str,
    old_key: &SigningKeyPair,
    new_key: &SigningKeyPair,
    root_key: &SigningKeyPair,
    expires_days: i64,
) -> TrustResult<Signed<RootMetadata>> {
    let expires = Utc::now() + Duration::days(expires_days);
    let (old_key_id, _) = signing_keypair_to_tuf_key(old_key);
    let (new_key_id, new_tuf_key) = signing_keypair_to_tuf_key(new_key);

    let mut new_root = current_root.signed.clone();
    new_root.version += 1;
    new_root.expires = expires;

    // Remove old key, add new key
    new_root.keys.remove(&old_key_id);
    new_root.keys.insert(new_key_id.clone(), new_tuf_key);

    // Update role definition
    if let Some(role_def) = new_root.roles.get_mut(role_name) {
        role_def.keyids.retain(|id| *id != old_key_id);
        if !role_def.keyids.contains(&new_key_id) {
            role_def.keyids.push(new_key_id);
        }
    }

    // Sign with both old root key and new root key (if rotating root)
    let mut sigs = vec![sign_tuf_metadata(root_key, &new_root)];
    if role_name == "root" {
        sigs.push(sign_tuf_metadata(new_key, &new_root));
    }

    Ok(Signed {
        signed: new_root,
        signatures: sigs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::keys::signing_keypair_to_tuf_key;

    #[test]
    fn test_create_initial_root_single_key() {
        let key = SigningKeyPair::generate();
        let signed = create_initial_root_single_key(&key, 365).unwrap();

        assert_eq!(signed.signed.version, 1);
        assert_eq!(signed.signed.type_field, "root");
        assert_eq!(signed.signed.roles.len(), 4);
        assert!(signed.signed.roles.contains_key("root"));
        assert!(signed.signed.roles.contains_key("targets"));
        assert!(signed.signed.roles.contains_key("snapshot"));
        assert!(signed.signed.roles.contains_key("timestamp"));
        assert_eq!(signed.signatures.len(), 1);

        // All roles should use the same key
        let (key_id, _) = signing_keypair_to_tuf_key(&key);
        for role_def in signed.signed.roles.values() {
            assert_eq!(role_def.keyids, vec![key_id.clone()]);
            assert_eq!(role_def.threshold, 1);
        }
    }

    #[test]
    fn test_create_initial_root_separate_keys() {
        let root_key = SigningKeyPair::generate();
        let targets_key = SigningKeyPair::generate();
        let snapshot_key = SigningKeyPair::generate();
        let timestamp_key = SigningKeyPair::generate();

        let signed = create_initial_root(
            &root_key,
            &targets_key,
            &snapshot_key,
            &timestamp_key,
            365,
        )
        .unwrap();

        assert_eq!(signed.signed.keys.len(), 4);
        assert_eq!(signed.signed.roles.len(), 4);

        // Each role should have a different key
        let (root_id, _) = signing_keypair_to_tuf_key(&root_key);
        let (targets_id, _) = signing_keypair_to_tuf_key(&targets_key);
        assert_ne!(root_id, targets_id);
        assert_eq!(signed.signed.roles["root"].keyids, vec![root_id]);
        assert_eq!(signed.signed.roles["targets"].keyids, vec![targets_id]);
    }

    #[test]
    fn test_rotate_key() {
        let key = SigningKeyPair::generate();
        let initial = create_initial_root_single_key(&key, 365).unwrap();

        let new_targets_key = SigningKeyPair::generate();
        let rotated = rotate_key(&initial, "targets", &key, &new_targets_key, &key, 365).unwrap();

        assert_eq!(rotated.signed.version, 2);

        // Targets role should have the new key
        let (new_id, _) = signing_keypair_to_tuf_key(&new_targets_key);
        assert_eq!(rotated.signed.roles["targets"].keyids, vec![new_id.clone()]);
        assert!(rotated.signed.keys.contains_key(&new_id));

        // Root role should still have the original key
        let (root_id, _) = signing_keypair_to_tuf_key(&key);
        assert_eq!(rotated.signed.roles["root"].keyids, vec![root_id]);
    }

    #[test]
    fn test_generate_role_key() {
        let temp_dir = tempfile::tempdir().unwrap();
        let keypair = generate_role_key("root", temp_dir.path()).unwrap();

        assert!(temp_dir.path().join("root.private").exists());
        assert!(temp_dir.path().join("root.public").exists());

        // Key should produce valid signatures
        let (_, tuf_key) = signing_keypair_to_tuf_key(&keypair);
        assert_eq!(tuf_key.keytype, "ed25519");
    }
}
