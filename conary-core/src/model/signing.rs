// conary-core/src/model/signing.rs

//! Ed25519 signing and verification for model collections
//!
//! Provides cryptographic signatures for CollectionData, allowing
//! verification that remote includes haven't been tampered with.

use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use super::ModelError;
use super::remote::CollectionData;

/// Sign canonical JSON of a CollectionData
pub fn sign_collection(data: &CollectionData, key: &SigningKey) -> Vec<u8> {
    let canonical = canonical_json(data);
    let signature = key.sign(canonical.as_bytes());
    signature.to_bytes().to_vec()
}

/// Verify a collection signature
pub fn verify_collection(
    data: &CollectionData,
    signature_bytes: &[u8],
    public_key_bytes: &[u8],
) -> Result<bool, ModelError> {
    let public_key = VerifyingKey::from_bytes(
        public_key_bytes
            .try_into()
            .map_err(|_| ModelError::RemoteFetchError("Invalid public key length".to_string()))?,
    )
    .map_err(|e| ModelError::RemoteFetchError(format!("Invalid public key: {e}")))?;

    let signature = Signature::from_bytes(
        signature_bytes
            .try_into()
            .map_err(|_| ModelError::RemoteFetchError("Invalid signature length".to_string()))?,
    );

    let canonical = canonical_json(data);
    Ok(public_key.verify(canonical.as_bytes(), &signature).is_ok())
}

/// Load a signing key from a file (32-byte raw seed or 64 hex chars)
pub fn load_signing_key(path: &Path) -> Result<SigningKey, ModelError> {
    let bytes = std::fs::read(path)
        .map_err(|e| ModelError::RemoteFetchError(format!("Cannot read key file: {e}")))?;

    // Support hex-encoded keys (64 chars) or raw 32-byte keys
    let seed_bytes = if bytes.len() == 64 {
        // Hex-encoded
        hex::decode(&bytes)
            .map_err(|e| ModelError::RemoteFetchError(format!("Invalid hex in key file: {e}")))?
    } else if bytes.len() == 32 {
        bytes
    } else {
        return Err(ModelError::RemoteFetchError(
            "Key file must be 32 raw bytes or 64 hex characters".to_string(),
        ));
    };

    let seed: [u8; 32] = seed_bytes
        .try_into()
        .map_err(|_| ModelError::RemoteFetchError("Invalid key length after decode".to_string()))?;

    Ok(SigningKey::from_bytes(&seed))
}

/// Generate a canonical JSON representation for signing
/// (deterministic serialization via serde_json)
fn canonical_json(data: &CollectionData) -> String {
    serde_json::to_string(data).expect("CollectionData should always serialize")
}

/// Get the public key ID (hex-encoded first 8 bytes of public key)
pub fn key_id(verifying_key: &VerifyingKey) -> String {
    hex::encode(&verifying_key.to_bytes()[..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::remote::{CollectionData, CollectionMemberData};
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_test_collection_data() -> CollectionData {
        CollectionData {
            name: "test-collection".to_string(),
            version: "1.0.0".to_string(),
            members: vec![],
            includes: vec![],
            pins: HashMap::new(),
            exclude: vec![],
            content_hash: "sha256:test".to_string(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_sign_and_verify_roundtrip() {
        let key = SigningKey::generate(&mut OsRng);
        let data = create_test_collection_data();

        let signature = sign_collection(&data, &key);
        let public_key = key.verifying_key();

        assert!(verify_collection(&data, &signature, &public_key.to_bytes()).unwrap());
    }

    #[test]
    fn test_verify_fails_with_wrong_key() {
        let key1 = SigningKey::generate(&mut OsRng);
        let key2 = SigningKey::generate(&mut OsRng);
        let data = create_test_collection_data();

        let signature = sign_collection(&data, &key1);
        let wrong_key = key2.verifying_key();

        assert!(!verify_collection(&data, &signature, &wrong_key.to_bytes()).unwrap());
    }

    #[test]
    fn test_verify_fails_with_tampered_data() {
        let key = SigningKey::generate(&mut OsRng);
        let mut data = create_test_collection_data();

        let signature = sign_collection(&data, &key);

        // Tamper with data
        data.version = "2.0.0".to_string();

        let public_key = key.verifying_key();
        assert!(!verify_collection(&data, &signature, &public_key.to_bytes()).unwrap());
    }

    #[test]
    fn test_key_id() {
        let key = SigningKey::generate(&mut OsRng);
        let id = key_id(&key.verifying_key());
        assert_eq!(id.len(), 16); // 8 bytes = 16 hex chars
    }

    #[test]
    fn test_verify_invalid_signature_length() {
        let key = SigningKey::generate(&mut OsRng);
        let data = create_test_collection_data();
        let public_key = key.verifying_key();

        let result = verify_collection(&data, &[0u8; 10], &public_key.to_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_invalid_public_key_length() {
        let key = SigningKey::generate(&mut OsRng);
        let data = create_test_collection_data();
        let signature = sign_collection(&data, &key);

        let result = verify_collection(&data, &signature, &[0u8; 10]);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_signing_key_hex_encoded() {
        let key = SigningKey::generate(&mut OsRng);
        let hex_key = hex::encode(key.to_bytes());

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(hex_key.as_bytes()).unwrap();

        let loaded = load_signing_key(file.path()).unwrap();
        assert_eq!(loaded.to_bytes(), key.to_bytes());
    }

    #[test]
    fn test_load_signing_key_raw_bytes() {
        let key = SigningKey::generate(&mut OsRng);

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&key.to_bytes()).unwrap();

        let loaded = load_signing_key(file.path()).unwrap();
        assert_eq!(loaded.to_bytes(), key.to_bytes());
    }

    #[test]
    fn test_load_signing_key_invalid_length() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&[0u8; 48]).unwrap();

        let result = load_signing_key(file.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_sign_collection_with_members() {
        let key = SigningKey::generate(&mut OsRng);
        let data = CollectionData {
            name: "group-full".to_string(),
            version: "2.0.0".to_string(),
            members: vec![
                CollectionMemberData {
                    name: "nginx".to_string(),
                    version_constraint: Some("1.24.*".to_string()),
                    is_optional: false,
                },
                CollectionMemberData {
                    name: "redis".to_string(),
                    version_constraint: None,
                    is_optional: true,
                },
            ],
            includes: vec!["group-core@upstream:stable".to_string()],
            pins: HashMap::from([("openssl".to_string(), "3.0.*".to_string())]),
            exclude: vec!["sendmail".to_string()],
            content_hash: "sha256:abc123".to_string(),
            published_at: "2026-01-15T12:00:00Z".to_string(),
        };

        let signature = sign_collection(&data, &key);
        let public_key = key.verifying_key();

        assert!(verify_collection(&data, &signature, &public_key.to_bytes()).unwrap());
    }
}
