// src/federation/manifest.rs
//! Signed federation manifests
//!
//! A federation manifest describes a resource (package, file, etc.) as an
//! ordered list of content-addressed chunks. Signing the manifest ensures:
//! - The chunk list hasn't been tampered with
//! - The chunks belong together (integrity)
//! - The manifest comes from a trusted source
//!
//! # Example
//!
//! ```ignore
//! use conary::federation::manifest::{FederationManifest, ManifestBuilder};
//! use conary::ccs::signing::SigningKeyPair;
//!
//! // Create a manifest for a package
//! let manifest = ManifestBuilder::new("mypackage-1.0.0")
//!     .add_chunk("abc123...", 65536)
//!     .add_chunk("def456...", 65536)
//!     .add_chunk("ghi789...", 32000)
//!     .build();
//!
//! // Sign it
//! let keypair = SigningKeyPair::load_from_file(&key_path)?;
//! let signed = manifest.sign(&keypair);
//!
//! // Verify it
//! let trust_policy = ManifestTrustPolicy::strict(vec![public_key]);
//! signed.verify(&trust_policy)?;
//! ```

use crate::ccs::signing::SigningKeyPair;
use crate::ccs::verify::PackageSignature;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors during manifest verification
#[derive(Error, Debug)]
pub enum ManifestError {
    #[error("Manifest is not signed")]
    NotSigned,

    #[error("Invalid signature format: {0}")]
    InvalidSignature(String),

    #[error("Signature verification failed")]
    SignatureFailed,

    #[error("Untrusted public key: {0}")]
    UntrustedKey(String),

    #[error("Manifest expired or invalid timestamp")]
    ExpiredOrInvalidTimestamp,

    #[error("Invalid manifest data: {0}")]
    InvalidData(String),
}

/// A chunk reference in a manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChunkRef {
    /// SHA-256 hash of the chunk content
    pub hash: String,
    /// Size of the chunk in bytes
    pub size: u64,
    /// Offset within the reconstructed resource
    pub offset: u64,
}

/// Federation manifest describing a resource as a list of chunks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationManifest {
    /// Manifest format version
    pub version: u32,
    /// Resource identifier (e.g., package name-version, file path)
    pub resource_id: String,
    /// Ordered list of chunks that make up this resource
    pub chunks: Vec<ChunkRef>,
    /// Total size of the reconstructed resource
    pub total_size: u64,
    /// MIME type of the resource (optional)
    #[serde(default)]
    pub content_type: Option<String>,
    /// Additional metadata
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    /// Signature (if signed)
    #[serde(default)]
    pub signature: Option<PackageSignature>,
}

impl FederationManifest {
    /// Create a new unsigned manifest
    pub fn new(resource_id: impl Into<String>) -> Self {
        Self {
            version: 1,
            resource_id: resource_id.into(),
            chunks: Vec::new(),
            total_size: 0,
            content_type: None,
            metadata: std::collections::HashMap::new(),
            signature: None,
        }
    }

    /// Add a chunk to the manifest
    pub fn add_chunk(&mut self, hash: impl Into<String>, size: u64) {
        let offset = self.total_size;
        self.chunks.push(ChunkRef {
            hash: hash.into(),
            size,
            offset,
        });
        self.total_size += size;
    }

    /// Get the canonical bytes for signing
    ///
    /// This serializes the manifest without the signature field
    /// to create a deterministic byte representation.
    fn canonical_bytes(&self) -> Vec<u8> {
        // Create a copy without signature for canonical representation
        let canonical = CanonicalManifest {
            version: self.version,
            resource_id: &self.resource_id,
            chunks: &self.chunks,
            total_size: self.total_size,
            content_type: self.content_type.as_deref(),
            metadata: &self.metadata,
        };

        // Use JSON for deterministic serialization
        serde_json::to_vec(&canonical).expect("manifest serialization should not fail")
    }

    /// Sign the manifest with the given key pair
    pub fn sign(&self, keypair: &SigningKeyPair) -> Self {
        let canonical = self.canonical_bytes();
        let signature = keypair.sign(&canonical);

        Self {
            signature: Some(signature),
            ..self.clone()
        }
    }

    /// Check if the manifest is signed
    pub fn is_signed(&self) -> bool {
        self.signature.is_some()
    }

    /// Verify the manifest signature against a trust policy
    pub fn verify(&self, policy: &ManifestTrustPolicy) -> Result<(), ManifestError> {
        let signature = match &self.signature {
            Some(sig) => sig,
            None => {
                if policy.allow_unsigned {
                    return Ok(());
                }
                return Err(ManifestError::NotSigned);
            }
        };

        // Check algorithm
        if signature.algorithm != "ed25519" {
            return Err(ManifestError::InvalidSignature(format!(
                "unsupported algorithm: {}",
                signature.algorithm
            )));
        }

        // Check if public key is trusted
        if !policy.trusted_keys.is_empty()
            && !policy.trusted_keys.contains(&signature.public_key)
        {
            return Err(ManifestError::UntrustedKey(
                signature.key_id.clone().unwrap_or_else(|| signature.public_key.clone()),
            ));
        }

        // Check timestamp if required
        if policy.require_timestamp && signature.timestamp.is_none() {
            return Err(ManifestError::ExpiredOrInvalidTimestamp);
        }

        // Verify signature
        let public_key_bytes = BASE64
            .decode(&signature.public_key)
            .map_err(|e| ManifestError::InvalidSignature(format!("invalid public key: {e}")))?;

        let verifying_key = VerifyingKey::from_bytes(
            public_key_bytes
                .as_slice()
                .try_into()
                .map_err(|_| ManifestError::InvalidSignature("invalid key length".into()))?,
        )
        .map_err(|e| ManifestError::InvalidSignature(format!("invalid key: {e}")))?;

        let sig_bytes = BASE64
            .decode(&signature.signature)
            .map_err(|e| ManifestError::InvalidSignature(format!("invalid signature: {e}")))?;

        let sig = Signature::from_slice(&sig_bytes)
            .map_err(|e| ManifestError::InvalidSignature(format!("invalid signature: {e}")))?;

        let canonical = self.canonical_bytes();
        verifying_key
            .verify(&canonical, &sig)
            .map_err(|_| ManifestError::SignatureFailed)?;

        Ok(())
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> Result<String, ManifestError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| ManifestError::InvalidData(e.to_string()))
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> Result<Self, ManifestError> {
        serde_json::from_str(json).map_err(|e| ManifestError::InvalidData(e.to_string()))
    }

    /// Get chunk hashes in order
    pub fn chunk_hashes(&self) -> Vec<&str> {
        self.chunks.iter().map(|c| c.hash.as_str()).collect()
    }
}

/// Canonical manifest for signing (without signature field)
#[derive(Serialize)]
struct CanonicalManifest<'a> {
    version: u32,
    resource_id: &'a str,
    chunks: &'a [ChunkRef],
    total_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<&'a str>,
    metadata: &'a std::collections::HashMap<String, String>,
}

/// Trust policy for manifest verification
#[derive(Debug, Clone, Default)]
pub struct ManifestTrustPolicy {
    /// Trusted public keys (base64-encoded)
    pub trusted_keys: Vec<String>,
    /// Whether to allow unsigned manifests
    pub allow_unsigned: bool,
    /// Whether to require timestamp
    pub require_timestamp: bool,
}

impl ManifestTrustPolicy {
    /// Create a permissive policy that allows unsigned manifests
    pub fn permissive() -> Self {
        Self {
            allow_unsigned: true,
            ..Default::default()
        }
    }

    /// Create a strict policy requiring signatures from trusted keys
    pub fn strict(trusted_keys: Vec<String>) -> Self {
        Self {
            trusted_keys,
            allow_unsigned: false,
            require_timestamp: false,
        }
    }

    /// Create a trust policy from federation config settings
    pub fn from_config(config: &super::config::FederationConfig) -> Self {
        Self {
            trusted_keys: config.manifest_trusted_keys.clone(),
            allow_unsigned: config.manifest_allow_unsigned,
            require_timestamp: false,
        }
    }

    /// Add a trusted key
    pub fn trust_key(&mut self, public_key: String) {
        if !self.trusted_keys.contains(&public_key) {
            self.trusted_keys.push(public_key);
        }
    }
}

/// Builder for creating manifests
pub struct ManifestBuilder {
    manifest: FederationManifest,
}

impl ManifestBuilder {
    /// Create a new manifest builder
    pub fn new(resource_id: impl Into<String>) -> Self {
        Self {
            manifest: FederationManifest::new(resource_id),
        }
    }

    /// Add a chunk
    pub fn add_chunk(mut self, hash: impl Into<String>, size: u64) -> Self {
        self.manifest.add_chunk(hash, size);
        self
    }

    /// Set content type
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.manifest.content_type = Some(content_type.into());
        self
    }

    /// Add metadata
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.manifest.metadata.insert(key.into(), value.into());
        self
    }

    /// Build the manifest
    pub fn build(self) -> FederationManifest {
        self.manifest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_creation() {
        let manifest = ManifestBuilder::new("test-package-1.0.0")
            .add_chunk("abc123", 1024)
            .add_chunk("def456", 2048)
            .content_type("application/octet-stream")
            .metadata("author", "test")
            .build();

        assert_eq!(manifest.resource_id, "test-package-1.0.0");
        assert_eq!(manifest.chunks.len(), 2);
        assert_eq!(manifest.total_size, 3072);
        assert_eq!(manifest.chunks[0].offset, 0);
        assert_eq!(manifest.chunks[1].offset, 1024);
        assert!(!manifest.is_signed());
    }

    #[test]
    fn test_manifest_signing_and_verification() {
        let keypair = SigningKeyPair::generate().with_key_id("test-key");
        let public_key = keypair.public_key_base64();

        let manifest = ManifestBuilder::new("signed-package-1.0.0")
            .add_chunk("chunk1hash", 512)
            .add_chunk("chunk2hash", 512)
            .build();

        // Sign
        let signed = manifest.sign(&keypair);
        assert!(signed.is_signed());

        // Verify with trusted key
        let policy = ManifestTrustPolicy::strict(vec![public_key.clone()]);
        assert!(signed.verify(&policy).is_ok());

        // Verify with permissive policy
        let permissive = ManifestTrustPolicy::permissive();
        assert!(signed.verify(&permissive).is_ok());
    }

    #[test]
    fn test_unsigned_manifest_strict_policy() {
        let manifest = ManifestBuilder::new("unsigned-package")
            .add_chunk("chunk1", 100)
            .build();

        let strict = ManifestTrustPolicy::strict(vec!["some-key".to_string()]);
        let result = manifest.verify(&strict);
        assert!(matches!(result, Err(ManifestError::NotSigned)));
    }

    #[test]
    fn test_unsigned_manifest_permissive_policy() {
        let manifest = ManifestBuilder::new("unsigned-package")
            .add_chunk("chunk1", 100)
            .build();

        let permissive = ManifestTrustPolicy::permissive();
        assert!(manifest.verify(&permissive).is_ok());
    }

    #[test]
    fn test_untrusted_key() {
        let keypair = SigningKeyPair::generate();
        let other_keypair = SigningKeyPair::generate();

        let manifest = ManifestBuilder::new("package")
            .add_chunk("chunk1", 100)
            .build();

        let signed = manifest.sign(&keypair);

        // Verify with different key
        let policy = ManifestTrustPolicy::strict(vec![other_keypair.public_key_base64()]);
        let result = signed.verify(&policy);
        assert!(matches!(result, Err(ManifestError::UntrustedKey(_))));
    }

    #[test]
    fn test_tampered_manifest() {
        let keypair = SigningKeyPair::generate();

        let manifest = ManifestBuilder::new("package")
            .add_chunk("chunk1", 100)
            .build();

        let mut signed = manifest.sign(&keypair);

        // Tamper with the manifest
        signed.chunks[0].hash = "tampered_hash".to_string();

        // Verification should fail
        let policy = ManifestTrustPolicy::strict(vec![keypair.public_key_base64()]);
        let result = signed.verify(&policy);
        assert!(matches!(result, Err(ManifestError::SignatureFailed)));
    }

    #[test]
    fn test_manifest_json_roundtrip() {
        let keypair = SigningKeyPair::generate();

        let manifest = ManifestBuilder::new("package-1.0.0")
            .add_chunk("hash1", 1000)
            .add_chunk("hash2", 2000)
            .metadata("version", "1.0.0")
            .build();

        let signed = manifest.sign(&keypair);

        // Serialize to JSON
        let json = signed.to_json().unwrap();

        // Deserialize back
        let loaded = FederationManifest::from_json(&json).unwrap();

        assert_eq!(loaded.resource_id, signed.resource_id);
        assert_eq!(loaded.chunks.len(), signed.chunks.len());
        assert!(loaded.is_signed());

        // Signature should still verify
        let policy = ManifestTrustPolicy::strict(vec![keypair.public_key_base64()]);
        assert!(loaded.verify(&policy).is_ok());
    }

    #[test]
    fn test_chunk_hashes() {
        let manifest = ManifestBuilder::new("test")
            .add_chunk("hash1", 100)
            .add_chunk("hash2", 200)
            .add_chunk("hash3", 300)
            .build();

        let hashes = manifest.chunk_hashes();
        assert_eq!(hashes, vec!["hash1", "hash2", "hash3"]);
    }
}
