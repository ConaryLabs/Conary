// crates/conary-core/src/trust/verify/tests.rs

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
    let (key_id, tuf_key) = signing_keypair_to_tuf_key(keypair).unwrap();

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

    let sig = sign_tuf_metadata(keypair, &root).unwrap();
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

    let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair).unwrap();
    let mut keys = BTreeMap::new();
    keys.insert(key_id, tuf_key);

    assert!(verify_signatures(&signed_root, Role::Root, &keys, 1).is_ok());
}

#[test]
fn test_verify_signatures_threshold_not_met() {
    let keypair = SigningKeyPair::generate();
    let expires = Utc::now() + Duration::days(365);
    let signed_root = make_test_root(&keypair, 1, expires);

    let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair).unwrap();
    let mut keys = BTreeMap::new();
    keys.insert(key_id, tuf_key);

    // Require 2 signatures but only have 1
    let result = verify_signatures(&signed_root, Role::Root, &keys, 2);
    assert!(result.is_err());
    assert!(matches!(result, Err(TrustError::ThresholdNotMet { .. })));
}

#[test]
fn verify_signatures_rejects_zero_threshold() {
    let keypair = SigningKeyPair::generate();
    let expires = Utc::now() + Duration::days(365);
    let signed_root = make_test_root(&keypair, 1, expires);

    let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair).unwrap();
    let mut keys = BTreeMap::new();
    keys.insert(key_id, tuf_key);

    let result = verify_signatures(&signed_root, Role::Root, &keys, 0);
    assert!(matches!(result, Err(TrustError::ConsistencyError(_))));
}

#[test]
fn test_verify_signatures_wrong_key() {
    let keypair1 = SigningKeyPair::generate();
    let keypair2 = SigningKeyPair::generate();
    let expires = Utc::now() + Duration::days(365);

    // Sign with keypair1
    let signed_root = make_test_root(&keypair1, 1, expires);

    // But verify with keypair2's key
    let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair2).unwrap();
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
fn static_snapshot_consistency_requires_root_and_targets_entries() {
    let snapshot = SnapshotMetadata {
        type_field: "snapshot".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: 1,
        expires: chrono::Utc::now() + chrono::Duration::days(1),
        meta: BTreeMap::new(),
    };
    let err = verify_static_snapshot_consistency(&snapshot, 1, 1).unwrap_err();
    assert!(err.to_string().contains("root.json"));
}

#[test]
fn test_verify_metadata_hash_ok() {
    let data = b"test metadata content";
    let hash = hash::sha256(data);
    let mut hashes = BTreeMap::new();
    hashes.insert("sha256".to_string(), hash);

    let meta_ref = MetaFile {
        version: 1,
        length: None,
        hashes: Some(hashes),
    };

    assert!(verify_metadata_hash(&meta_ref, data, false).is_ok());
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

    let result = verify_metadata_hash(&meta_ref, b"test", false);
    assert!(matches!(result, Err(TrustError::ConsistencyError(_))));
}

#[test]
fn test_verify_metadata_hash_no_hash_is_ok() {
    let meta_ref = MetaFile {
        version: 1,
        length: None,
        hashes: None,
    };

    // No hash to check = passes (require_hash = false)
    assert!(verify_metadata_hash(&meta_ref, b"anything", false).is_ok());
}

#[test]
fn test_verify_metadata_hash_require_hash_rejects_missing() {
    let meta_ref = MetaFile {
        version: 1,
        length: None,
        hashes: None,
    };

    // require_hash = true should fail when no hash is present
    let result = verify_metadata_hash(&meta_ref, b"anything", true);
    assert!(matches!(result, Err(TrustError::ConsistencyError(_))));
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
fn extract_role_keys_rejects_mismatched_canonical_key_id() {
    let victim_keypair = SigningKeyPair::generate();
    let attacker_keypair = SigningKeyPair::generate();
    let expires = Utc::now() + Duration::days(365);
    let mut signed_root = make_test_root(&victim_keypair, 1, expires);
    let victim_key_id = signed_root.signed.roles["root"].keyids[0].clone();
    let (_, attacker_tuf_key) = signing_keypair_to_tuf_key(&attacker_keypair).unwrap();
    signed_root
        .signed
        .keys
        .insert(victim_key_id, attacker_tuf_key);

    let result = extract_role_keys(&signed_root.signed, Role::Root);
    assert!(matches!(result, Err(TrustError::KeyError(_))));
}

#[test]
fn extract_role_keys_rejects_zero_threshold() {
    let keypair = SigningKeyPair::generate();
    let expires = Utc::now() + Duration::days(365);
    let mut signed_root = make_test_root(&keypair, 1, expires);
    signed_root.signed.roles.get_mut("root").unwrap().threshold = 0;

    let result = extract_role_keys(&signed_root.signed, Role::Root);
    assert!(matches!(result, Err(TrustError::ConsistencyError(_))));
}

#[test]
fn test_verify_file_missing_path_context() {
    let mut hashes = BTreeMap::new();
    hashes.insert("sha256".to_string(), "abc".to_string());
    let meta_ref = MetaFile {
        version: 1,
        length: None,
        hashes: Some(hashes),
    };

    let result = verify_file(&meta_ref, std::path::Path::new("/nonexistent/file.json"));
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("/nonexistent/file.json"),
        "Error should contain file path, got: {err_msg}"
    );
}

#[test]
fn test_verify_root_self_signed() {
    let keypair = SigningKeyPair::generate();
    let expires = Utc::now() + Duration::days(365);
    let signed_root = make_test_root(&keypair, 1, expires);

    let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair).unwrap();
    let mut trusted_keys = BTreeMap::new();
    trusted_keys.insert(key_id, tuf_key);

    assert!(verify_root(&signed_root, &trusted_keys, 1).is_ok());
}
