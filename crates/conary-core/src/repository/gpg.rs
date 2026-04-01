// conary-core/src/repository/gpg.rs

//! GPG signature verification for packages
//!
//! This module provides functionality for verifying GPG signatures on downloaded packages
//! using the sequoia-openpgp library (pure Rust implementation).

use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use openpgp::parse::Parse;
use openpgp::policy::StandardPolicy;
use sequoia_openpgp as openpgp;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

fn detached_signature_urls(url: &str) -> Vec<String> {
    vec![format!("{url}.sig"), format!("{url}.asc")]
}

#[derive(Debug, Clone)]
pub struct MetadataSignatureVerifier {
    keyring_dir: PathBuf,
    repository_name: String,
    enabled: bool,
}

impl MetadataSignatureVerifier {
    pub fn new(keyring_dir: PathBuf, repository_name: String, enabled: bool) -> Self {
        Self {
            keyring_dir,
            repository_name,
            enabled,
        }
    }

    pub async fn verify_metadata_bytes(
        &self,
        metadata_url: &str,
        metadata_bytes: &[u8],
        metadata_label: &str,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let verifier = GpgVerifier::new(self.keyring_dir.clone())?;
        if !verifier.has_key(&self.repository_name) {
            warn!(
                repository = self.repository_name,
                metadata = metadata_label,
                url = metadata_url,
                "Repository metadata GPG verification enabled but no key is imported; skipping verification"
            );
            return Ok(());
        }

        let client = RepositoryClient::new()?;
        let mut last_download_error = None;

        for signature_url in detached_signature_urls(metadata_url) {
            match client.download_to_bytes(&signature_url).await {
                Ok(signature_bytes) => {
                    let metadata_file = tempfile::NamedTempFile::new().map_err(|error| {
                        Error::IoError(format!(
                            "Failed to create temporary file for {}: {}",
                            metadata_label, error
                        ))
                    })?;
                    fs::write(metadata_file.path(), metadata_bytes).map_err(|error| {
                        Error::IoError(format!(
                            "Failed to write temporary metadata file for {}: {}",
                            metadata_label, error
                        ))
                    })?;

                    let signature_file = tempfile::NamedTempFile::new().map_err(|error| {
                        Error::IoError(format!(
                            "Failed to create temporary signature file for {}: {}",
                            metadata_label, error
                        ))
                    })?;
                    fs::write(signature_file.path(), &signature_bytes).map_err(|error| {
                        Error::IoError(format!(
                            "Failed to write temporary signature file for {}: {}",
                            metadata_label, error
                        ))
                    })?;

                    verifier.verify_signature(
                        metadata_file.path(),
                        signature_file.path(),
                        &self.repository_name,
                    )?;
                    info!(
                        repository = self.repository_name,
                        metadata = metadata_label,
                        url = metadata_url,
                        signature_url = signature_url,
                        "Verified detached GPG signature for repository metadata"
                    );
                    return Ok(());
                }
                Err(Error::DownloadError(message))
                    if message.starts_with("HTTP 404") || message.starts_with("HTTP 403") =>
                {
                    continue;
                }
                Err(error) => {
                    last_download_error = Some((signature_url, error));
                    break;
                }
            }
        }

        if let Some((signature_url, error)) = last_download_error {
            return Err(Error::DownloadError(format!(
                "Failed to download metadata signature for {} from {}: {}",
                metadata_label, signature_url, error
            )));
        }

        warn!(
            repository = self.repository_name,
            metadata = metadata_label,
            url = metadata_url,
            "Repository metadata GPG verification enabled but no detached signature was found"
        );
        Ok(())
    }
}

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
            fs::create_dir_all(&keyring_dir).map_err(|e| {
                Error::IoError(format!("Failed to create keyring directory: {}", e))
            })?;
        }

        Ok(Self {
            keyring_dir,
            policy: StandardPolicy::new(),
        })
    }

    /// Sanitize a repository name for safe use as a filesystem path component.
    ///
    /// Replaces any character that is not alphanumeric, dash, underscore, or
    /// dot with an underscore.  Also rejects names containing path traversal
    /// sequences (`..`) or forward slashes (`/`).
    fn sanitize_repo_name(name: &str) -> Result<String> {
        if name.contains("..") || name.contains('/') {
            return Err(Error::ConfigError(format!(
                "Repository name contains unsafe path characters: {}",
                name
            )));
        }
        let sanitized: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Ok(sanitized)
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

        // Store key in keyring directory (sanitized to prevent path traversal)
        let safe_name = Self::sanitize_repo_name(repository_name)?;
        let key_path = self.keyring_dir.join(format!("{}.asc", safe_name));
        fs::write(&key_path, key_data)
            .map_err(|e| Error::IoError(format!("Failed to write GPG key: {}", e)))?;

        info!(
            "Imported GPG key for repository '{}' (fingerprint: {})",
            repository_name, fingerprint
        );
        Ok(fingerprint)
    }

    /// Import a GPG public key from a file
    pub fn import_key_from_file(&self, key_path: &Path, repository_name: &str) -> Result<String> {
        let key_data = fs::read(key_path)
            .map_err(|e| Error::IoError(format!("Failed to read GPG key file: {}", e)))?;
        self.import_key(&key_data, repository_name)
    }

    /// Get the path to a repository's GPG key (sanitized).
    fn get_key_path(&self, repository_name: &str) -> Result<PathBuf> {
        let safe_name = Self::sanitize_repo_name(repository_name)?;
        Ok(self.keyring_dir.join(format!("{}.asc", safe_name)))
    }

    /// Check if a GPG key exists for a repository
    pub fn has_key(&self, repository_name: &str) -> bool {
        self.get_key_path(repository_name)
            .map(|p| p.exists())
            .unwrap_or(false)
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
        debug!(
            "Verifying signature for {:?} using repository '{}'",
            file_path, repository_name
        );

        // Load the repository's GPG key
        let key_path = self.get_key_path(repository_name)?;
        if !key_path.exists() {
            return Err(Error::NotFound(format!(
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
                    if key.for_signing() && sig.verify_message(key.key(), &message_data).is_ok() {
                        found_valid_signature = true;
                        break;
                    }
                }
                if found_valid_signature {
                    break;
                }
            }
        }

        if !found_valid_signature {
            return Err(Error::GpgVerificationFailed(
                "No valid signatures found or verification failed".to_string(),
            ));
        }

        info!("Successfully verified signature for {:?}", file_path);
        Ok(())
    }

    /// Remove a GPG key for a repository
    pub fn remove_key(&self, repository_name: &str) -> Result<()> {
        let key_path = self.get_key_path(repository_name)?;
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
            .map_err(|e| Error::IoError(format!("Failed to read keyring directory: {}", e)))?
        {
            let entry = entry
                .map_err(|e| Error::IoError(format!("Failed to read directory entry: {}", e)))?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("asc")
                && let Some(repo_name) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(key_data) = fs::read(&path)
                && let Ok(cert) = openpgp::Cert::from_bytes(&key_data)
            {
                let fingerprint = cert.fingerprint().to_string();
                keys.push((repo_name.to_string(), fingerprint));
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

    #[test]
    fn test_detached_signature_urls_try_sig_then_asc() {
        let urls = detached_signature_urls("https://example.com/repodata/repomd.xml");
        assert_eq!(
            urls,
            vec![
                "https://example.com/repodata/repomd.xml.sig".to_string(),
                "https://example.com/repodata/repomd.xml.asc".to_string(),
            ]
        );
    }
}
