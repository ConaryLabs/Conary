// src/repository/gpg.rs

//! GPG signature verification for packages
//!
//! This module provides functionality for verifying GPG signatures on downloaded packages
//! using the sequoia-openpgp library (pure Rust implementation).

use crate::error::{Error, Result};
use sequoia_openpgp as openpgp;
use openpgp::parse::Parse;
use openpgp::policy::StandardPolicy;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// GPG key and signature verifier
pub struct GpgVerifier {
    /// Directory where GPG keys are stored (per-repository)
    keyring_dir: PathBuf,
    /// OpenPGP policy for signature verification
    policy: StandardPolicy<'static>,
}

impl GpgVerifier {
    /// Create a new GPG verifier with the specified keyring directory
    pub fn new(keyring_dir: PathBuf) -> Result<Self> {
        // Create keyring directory if it doesn't exist
        if !keyring_dir.exists() {
            fs::create_dir_all(&keyring_dir)
                .map_err(|e| Error::IoError(format!("Failed to create keyring directory: {}", e)))?;
        }

        Ok(Self {
            keyring_dir,
            policy: StandardPolicy::new(),
        })
    }

    /// Import a GPG public key from bytes
    ///
    /// The key is stored in the keyring directory with a filename based on the key ID.
    pub fn import_key(&self, key_data: &[u8], repository_name: &str) -> Result<String> {
        // Parse the certificate (public key)
        let cert = openpgp::Cert::from_bytes(key_data)
            .map_err(|e| Error::ParseError(format!("Failed to parse GPG key: {}", e)))?;

        // Get key fingerprint for identification
        let fingerprint = cert.fingerprint().to_string();
        debug!("Importing GPG key with fingerprint: {}", fingerprint);

        // Store key in keyring directory
        let key_path = self.keyring_dir.join(format!("{}.asc", repository_name));
        fs::write(&key_path, key_data)
            .map_err(|e| Error::IoError(format!("Failed to write GPG key: {}", e)))?;

        info!("Imported GPG key for repository '{}' (fingerprint: {})", repository_name, fingerprint);
        Ok(fingerprint)
    }

    /// Import a GPG public key from a file
    pub fn import_key_from_file(&self, key_path: &Path, repository_name: &str) -> Result<String> {
        let key_data = fs::read(key_path)
            .map_err(|e| Error::IoError(format!("Failed to read GPG key file: {}", e)))?;
        self.import_key(&key_data, repository_name)
    }

    /// Get the path to a repository's GPG key
    fn get_key_path(&self, repository_name: &str) -> PathBuf {
        self.keyring_dir.join(format!("{}.asc", repository_name))
    }

    /// Check if a GPG key exists for a repository
    pub fn has_key(&self, repository_name: &str) -> bool {
        self.get_key_path(repository_name).exists()
    }

    /// Verify a detached GPG signature for a file
    ///
    /// # Arguments
    /// * `file_path` - Path to the file to verify
    /// * `signature_path` - Path to the detached signature (.asc file)
    /// * `repository_name` - Name of the repository (used to locate the GPG key)
    ///
    /// # Returns
    /// Ok(()) if the signature is valid, Err otherwise
    pub fn verify_signature(
        &self,
        file_path: &Path,
        signature_path: &Path,
        repository_name: &str,
    ) -> Result<()> {
        debug!("Verifying signature for {:?} using repository '{}'", file_path, repository_name);

        // Load the repository's GPG key
        let key_path = self.get_key_path(repository_name);
        if !key_path.exists() {
            return Err(Error::NotFoundError(format!(
                "GPG key not found for repository '{}'. Run repo-sync to import keys.",
                repository_name
            )));
        }

        let key_data = fs::read(&key_path)
            .map_err(|e| Error::IoError(format!("Failed to read GPG key: {}", e)))?;

        let cert = openpgp::Cert::from_bytes(&key_data)
            .map_err(|e| Error::ParseError(format!("Failed to parse GPG key: {}", e)))?;

        // Read the message data (file to verify)
        let message_data = fs::read(file_path)
            .map_err(|e| Error::IoError(format!("Failed to read file to verify: {}", e)))?;

        // Read the detached signature
        let signature_data = fs::read(signature_path)
            .map_err(|e| Error::IoError(format!("Failed to read signature file: {}", e)))?;

        // Parse signature packets
        use openpgp::PacketPile;
        let signature_pile = PacketPile::from_bytes(&signature_data)
            .map_err(|e| Error::ParseError(format!("Failed to parse signature: {}", e)))?;

        // Extract signature packets
        let mut found_valid_signature = false;
        for packet in signature_pile.descendants() {
            if let openpgp::Packet::Signature(sig) = packet {
                // Try to verify with each valid key
                for key in cert.keys().with_policy(&self.policy, None) {
                    if key.for_signing() {
                        if sig.verify_message(key.key(), &message_data).is_ok() {
                            found_valid_signature = true;
                            break;
                        }
                    }
                }
                if found_valid_signature {
                    break;
                }
            }
        }

        if !found_valid_signature {
            return Err(Error::GpgVerificationFailed(
                "No valid signatures found or verification failed".to_string()
            ));
        }

        info!("Successfully verified signature for {:?}", file_path);
        Ok(())
    }

    /// Remove a GPG key for a repository
    pub fn remove_key(&self, repository_name: &str) -> Result<()> {
        let key_path = self.get_key_path(repository_name);
        if key_path.exists() {
            fs::remove_file(&key_path)
                .map_err(|e| Error::IoError(format!("Failed to remove GPG key: {}", e)))?;
            info!("Removed GPG key for repository '{}'", repository_name);
        }
        Ok(())
    }

    /// List all imported GPG keys
    pub fn list_keys(&self) -> Result<Vec<(String, String)>> {
        let mut keys = Vec::new();

        if !self.keyring_dir.exists() {
            return Ok(keys);
        }

        for entry in fs::read_dir(&self.keyring_dir)
            .map_err(|e| Error::IoError(format!("Failed to read keyring directory: {}", e)))? {
            let entry = entry
                .map_err(|e| Error::IoError(format!("Failed to read directory entry: {}", e)))?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("asc") {
                if let Some(repo_name) = path.file_stem().and_then(|s| s.to_str()) {
                    // Try to read the key and get its fingerprint
                    if let Ok(key_data) = fs::read(&path) {
                        if let Ok(cert) = openpgp::Cert::from_bytes(&key_data) {
                            let fingerprint = cert.fingerprint().to_string();
                            keys.push((repo_name.to_string(), fingerprint));
                        }
                    }
                }
            }
        }

        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_verifier_creation() {
        let temp_dir = TempDir::new().unwrap();
        let _verifier = GpgVerifier::new(temp_dir.path().to_path_buf()).unwrap();
        assert!(temp_dir.path().exists());
    }

    #[test]
    fn test_has_key() {
        let temp_dir = TempDir::new().unwrap();
        let verifier = GpgVerifier::new(temp_dir.path().to_path_buf()).unwrap();

        assert!(!verifier.has_key("test-repo"));
    }

    #[test]
    fn test_list_keys_empty() {
        let temp_dir = TempDir::new().unwrap();
        let verifier = GpgVerifier::new(temp_dir.path().to_path_buf()).unwrap();

        let keys = verifier.list_keys().unwrap();
        assert_eq!(keys.len(), 0);
    }
}
