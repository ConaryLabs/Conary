// src/ccs/signing.rs
//! CCS package signing
//!
//! Provides Ed25519 signing for CCS package manifests.
//! Keys can be generated, stored, and loaded for signing operations.

use crate::ccs::verify::PackageSignature;
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// A signing key pair for CCS packages
pub struct SigningKeyPair {
    signing_key: SigningKey,
    key_id: Option<String>,
}

impl SigningKeyPair {
    /// Generate a new random key pair
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self {
            signing_key,
            key_id: None,
        }
    }

    /// Create from an existing signing key
    pub fn from_signing_key(key: SigningKey) -> Self {
        Self {
            signing_key: key,
            key_id: None,
        }
    }

    /// Set a human-readable key identifier
    pub fn with_key_id(mut self, id: &str) -> Self {
        self.key_id = Some(id.to_string());
        self
    }

    /// Get the public key
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// Get the public key as base64
    pub fn public_key_base64(&self) -> String {
        BASE64.encode(self.verifying_key().as_bytes())
    }

    /// Get the key ID
    pub fn key_id(&self) -> Option<&str> {
        self.key_id.as_deref()
    }

    /// Sign content and return a PackageSignature
    pub fn sign(&self, content: &[u8]) -> PackageSignature {
        let signature = self.signing_key.sign(content);
        let timestamp = chrono::Utc::now().to_rfc3339();

        PackageSignature {
            algorithm: "ed25519".to_string(),
            signature: BASE64.encode(signature.to_bytes()),
            public_key: self.public_key_base64(),
            key_id: self.key_id.clone(),
            timestamp: Some(timestamp),
        }
    }

    /// Save the key pair to files (private and public)
    pub fn save_to_files(&self, private_path: &Path, public_path: &Path) -> Result<()> {
        // Save private key (keep secure!)
        let private_data = KeyFile {
            algorithm: "ed25519".to_string(),
            key: BASE64.encode(self.signing_key.to_bytes()),
            key_id: self.key_id.clone(),
        };
        let private_toml = toml::to_string_pretty(&private_data)?;
        fs::write(private_path, private_toml)
            .with_context(|| format!("Failed to write private key: {}", private_path.display()))?;

        // Set restrictive permissions on private key
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(private_path)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(private_path, perms)?;
        }

        // Save public key
        let public_data = KeyFile {
            algorithm: "ed25519".to_string(),
            key: self.public_key_base64(),
            key_id: self.key_id.clone(),
        };
        let public_toml = toml::to_string_pretty(&public_data)?;
        fs::write(public_path, public_toml)
            .with_context(|| format!("Failed to write public key: {}", public_path.display()))?;

        Ok(())
    }

    /// Load a key pair from a private key file
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read key file: {}", path.display()))?;

        let key_file: KeyFile = toml::from_str(&content)
            .with_context(|| format!("Failed to parse key file: {}", path.display()))?;

        if key_file.algorithm != "ed25519" {
            anyhow::bail!("Unsupported key algorithm: {}", key_file.algorithm);
        }

        let key_bytes = BASE64
            .decode(&key_file.key)
            .context("Invalid base64 in key file")?;

        let key_array: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid key length"))?;

        let signing_key = SigningKey::from_bytes(&key_array);

        Ok(Self {
            signing_key,
            key_id: key_file.key_id,
        })
    }
}

/// Key file format
#[derive(Debug, Serialize, Deserialize)]
struct KeyFile {
    algorithm: String,
    key: String,
    #[serde(default)]
    key_id: Option<String>,
}

/// Load a public key from a file (for trust policy)
pub fn load_public_key(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read public key: {}", path.display()))?;

    let key_file: KeyFile = toml::from_str(&content)
        .with_context(|| format!("Failed to parse public key: {}", path.display()))?;

    Ok(key_file.key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_generate_and_sign() {
        let keypair = SigningKeyPair::generate().with_key_id("test-key");

        let content = b"test manifest content";
        let signature = keypair.sign(content);

        assert_eq!(signature.algorithm, "ed25519");
        assert!(signature.timestamp.is_some());
        assert_eq!(signature.key_id, Some("test-key".to_string()));

        // Verify the signature manually
        let sig_bytes = BASE64.decode(&signature.signature).unwrap();
        let sig = ed25519_dalek::Signature::from_slice(&sig_bytes).unwrap();
        keypair.verifying_key().verify_strict(content, &sig).unwrap();
    }

    #[test]
    fn test_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let private_path = temp_dir.path().join("key.private");
        let public_path = temp_dir.path().join("key.public");

        // Generate and save
        let keypair = SigningKeyPair::generate().with_key_id("test-key");
        let original_public = keypair.public_key_base64();
        keypair.save_to_files(&private_path, &public_path).unwrap();

        // Load and verify
        let loaded = SigningKeyPair::load_from_file(&private_path).unwrap();
        assert_eq!(loaded.public_key_base64(), original_public);
        assert_eq!(loaded.key_id(), Some("test-key"));
    }

    #[test]
    fn test_load_public_key() {
        let temp_dir = TempDir::new().unwrap();
        let public_path = temp_dir.path().join("key.public");

        let keypair = SigningKeyPair::generate();
        let private_path = temp_dir.path().join("key.private");
        keypair.save_to_files(&private_path, &public_path).unwrap();

        let public_key = load_public_key(&public_path).unwrap();
        assert_eq!(public_key, keypair.public_key_base64());
    }
}
