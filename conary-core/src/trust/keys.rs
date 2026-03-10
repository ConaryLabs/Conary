// conary-core/src/trust/keys.rs

//! TUF key utilities
//!
//! Bridges the existing `SigningKeyPair` from CCS signing to TUF key formats.
//! Provides canonical JSON serialization for deterministic key IDs and signatures.

use crate::ccs::signing::SigningKeyPair;
use crate::trust::metadata::{KeyVal, TufKey, TufSignature};
use crate::trust::{TrustError, TrustResult};
use ed25519_dalek::Signer;
use sha2::{Digest, Sha256};

/// Compute the TUF key ID for a key
///
/// Per the TUF spec, the key ID is the hex-encoded SHA-256 hash of the
/// OLPC canonical JSON representation of the key.
pub fn compute_key_id(key: &TufKey) -> TrustResult<String> {
    let canonical = canonical_json(key)?;
    let hash = Sha256::digest(&canonical);
    Ok(hex::encode(hash))
}

/// Produce deterministic (canonical) JSON for a serializable value
///
/// Canonical JSON as defined by OLPC:
/// - Object keys sorted lexicographically
/// - No unnecessary whitespace
/// - No trailing commas
///
/// This is used for computing key IDs and for signing metadata.
pub fn canonical_json<T: serde::Serialize>(value: &T) -> TrustResult<Vec<u8>> {
    let json_value = serde_json::to_value(value)
        .map_err(|e| TrustError::SerializationError(format!("Failed to serialize to Value: {e}")))?;
    let sorted = sort_json_value(&json_value);
    serde_json::to_vec(&sorted)
        .map_err(|e| TrustError::SerializationError(format!("Failed to serialize to Vec: {e}")))
}

/// Recursively sort JSON object keys for canonical representation
fn sort_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            // BTreeMap sorts keys lexicographically
            let sorted: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), sort_json_value(v)))
                .collect();
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_json_value).collect())
        }
        other => other.clone(),
    }
}

/// Convert a `SigningKeyPair` to a TUF key with its computed key ID
///
/// Returns `(key_id, tuf_key)` where the public key is hex-encoded.
pub fn signing_keypair_to_tuf_key(keypair: &SigningKeyPair) -> TrustResult<(String, TufKey)> {
    let public_hex = hex::encode(keypair.verifying_key().as_bytes());
    let tuf_key = TufKey {
        keytype: "ed25519".to_string(),
        scheme: "ed25519".to_string(),
        keyval: KeyVal { public: public_hex },
    };
    let key_id = compute_key_id(&tuf_key)?;
    Ok((key_id, tuf_key))
}

/// Sign TUF metadata using a `SigningKeyPair`
///
/// Computes the canonical JSON of the metadata, signs it with Ed25519,
/// and returns a `TufSignature` with the key ID and hex-encoded signature.
pub fn sign_tuf_metadata<T: serde::Serialize>(
    keypair: &SigningKeyPair,
    metadata: &T,
) -> TrustResult<TufSignature> {
    let canonical = canonical_json(metadata)?;
    let signature = keypair.signing_key().sign(&canonical);
    let (key_id, _) = signing_keypair_to_tuf_key(keypair)?;

    Ok(TufSignature {
        keyid: key_id,
        sig: hex::encode(signature.to_bytes()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::metadata::{RootMetadata, Signed, TUF_SPEC_VERSION, TimestampMetadata};
    use chrono::{TimeZone, Utc};
    use std::collections::BTreeMap;

    #[test]
    fn test_canonical_json_deterministic() {
        // Two maps with different insertion orders should produce identical canonical JSON
        let mut map1 = serde_json::Map::new();
        map1.insert("zebra".to_string(), serde_json::Value::from(1));
        map1.insert("apple".to_string(), serde_json::Value::from(2));

        let mut map2 = serde_json::Map::new();
        map2.insert("apple".to_string(), serde_json::Value::from(2));
        map2.insert("zebra".to_string(), serde_json::Value::from(1));

        let val1 = serde_json::Value::Object(map1);
        let val2 = serde_json::Value::Object(map2);

        let c1 = canonical_json(&val1).unwrap();
        let c2 = canonical_json(&val2).unwrap();
        assert_eq!(c1, c2);

        // Keys should be sorted: apple before zebra
        let s = String::from_utf8(c1).unwrap();
        let apple_pos = s.find("apple").unwrap();
        let zebra_pos = s.find("zebra").unwrap();
        assert!(apple_pos < zebra_pos);
    }

    #[test]
    fn test_canonical_json_nested_sorting() {
        let mut inner = serde_json::Map::new();
        inner.insert("z".to_string(), serde_json::Value::from("last"));
        inner.insert("a".to_string(), serde_json::Value::from("first"));

        let mut outer = serde_json::Map::new();
        outer.insert("nested".to_string(), serde_json::Value::Object(inner));
        outer.insert("top".to_string(), serde_json::Value::from(42));

        let val = serde_json::Value::Object(outer);
        let c = canonical_json(&val).unwrap();
        let s = String::from_utf8(c).unwrap();

        // Outer keys sorted: nested before top
        let nested_pos = s.find("nested").unwrap();
        let top_pos = s.find("top").unwrap();
        assert!(nested_pos < top_pos);

        // Inner keys sorted: a before z
        let a_pos = s.find("\"a\"").unwrap();
        let z_pos = s.find("\"z\"").unwrap();
        assert!(a_pos < z_pos);
    }

    #[test]
    fn test_canonical_json_no_whitespace() {
        let mut map = serde_json::Map::new();
        map.insert("key".to_string(), serde_json::Value::from("value"));
        let val = serde_json::Value::Object(map);

        let c = canonical_json(&val).unwrap();
        let s = String::from_utf8(c).unwrap();
        assert_eq!(s, "{\"key\":\"value\"}");
    }

    #[test]
    fn test_compute_key_id_consistent() {
        let key = TufKey {
            keytype: "ed25519".to_string(),
            scheme: "ed25519".to_string(),
            keyval: KeyVal {
                public: "abcdef0123456789".to_string(),
            },
        };

        let id1 = compute_key_id(&key).unwrap();
        let id2 = compute_key_id(&key).unwrap();
        assert_eq!(id1, id2);
        // Should be a hex-encoded SHA-256 (64 chars)
        assert_eq!(id1.len(), 64);
        assert!(id1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_compute_key_id_different_for_different_keys() {
        let key1 = TufKey {
            keytype: "ed25519".to_string(),
            scheme: "ed25519".to_string(),
            keyval: KeyVal {
                public: "aaaa".to_string(),
            },
        };
        let key2 = TufKey {
            keytype: "ed25519".to_string(),
            scheme: "ed25519".to_string(),
            keyval: KeyVal {
                public: "bbbb".to_string(),
            },
        };

        assert_ne!(compute_key_id(&key1).unwrap(), compute_key_id(&key2).unwrap());
    }

    #[test]
    fn test_signing_keypair_to_tuf_key() {
        let keypair = SigningKeyPair::generate();
        let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair).unwrap();

        assert_eq!(tuf_key.keytype, "ed25519");
        assert_eq!(tuf_key.scheme, "ed25519");
        // Public key should be hex-encoded (64 chars for 32 bytes)
        assert_eq!(tuf_key.keyval.public.len(), 64);
        assert!(tuf_key.keyval.public.chars().all(|c| c.is_ascii_hexdigit()));
        // Key ID should be consistent
        assert_eq!(key_id, compute_key_id(&tuf_key).unwrap());
    }

    #[test]
    fn test_sign_tuf_metadata_produces_valid_signature() {
        let keypair = SigningKeyPair::generate();
        let expires = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let timestamp = TimestampMetadata {
            type_field: "timestamp".to_string(),
            spec_version: TUF_SPEC_VERSION.to_string(),
            version: 1,
            expires,
            meta: BTreeMap::new(),
        };

        let sig = sign_tuf_metadata(&keypair, &timestamp).unwrap();

        // Key ID should match
        let (expected_key_id, _) = signing_keypair_to_tuf_key(&keypair).unwrap();
        assert_eq!(sig.keyid, expected_key_id);

        // Signature should be hex-encoded (128 chars for 64 bytes)
        assert_eq!(sig.sig.len(), 128);
        assert!(sig.sig.chars().all(|c| c.is_ascii_hexdigit()));

        // Verify the signature manually
        let canonical = canonical_json(&timestamp).unwrap();
        let sig_bytes = hex::decode(&sig.sig).unwrap();
        let signature = ed25519_dalek::Signature::from_slice(&sig_bytes).unwrap();
        keypair
            .verifying_key()
            .verify_strict(&canonical, &signature)
            .unwrap();
    }

    #[test]
    fn test_sign_and_wrap_root_metadata() {
        let keypair = SigningKeyPair::generate();
        let (key_id, tuf_key) = signing_keypair_to_tuf_key(&keypair).unwrap();
        let expires = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();

        let mut keys = BTreeMap::new();
        keys.insert(key_id.clone(), tuf_key);

        let mut roles = BTreeMap::new();
        roles.insert(
            "root".to_string(),
            crate::trust::metadata::RoleDefinition {
                keyids: vec![key_id],
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

        let sig = sign_tuf_metadata(&keypair, &root).unwrap();
        let signed = Signed {
            signed: root,
            signatures: vec![sig],
        };

        // Should round-trip through JSON
        let json = serde_json::to_string_pretty(&signed).unwrap();
        let deserialized: Signed<RootMetadata> = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.signed.version, 1);
        assert_eq!(deserialized.signatures.len(), 1);
    }
}
