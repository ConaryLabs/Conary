// conary-core/src/ccs/signing.rs
//! CCS package signing
//!
//! Provides Ed25519 signing for CCS package manifests.
//! Keys can be generated, stored, and loaded for signing operations.

use crate::ccs::verify::PackageSignature;
use crate::error::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rand_core_06::OsRng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static PRIVATE_KEY_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const PRIVATE_KEY_TEMP_ATTEMPTS: usize = 1024;

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

    /// Get a reference to the underlying signing key
    ///
    /// Used by TUF key utilities for raw Ed25519 signing operations.
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
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
        let private_toml = toml::to_string_pretty(&private_data)
            .map_err(|e| Error::ParseError(format!("Failed to serialize private key: {}", e)))?;
        write_private_key_atomic(private_path, &private_toml)?;

        // Save public key
        let public_data = KeyFile {
            algorithm: "ed25519".to_string(),
            key: self.public_key_base64(),
            key_id: self.key_id.clone(),
        };
        let public_toml = toml::to_string_pretty(&public_data)
            .map_err(|e| Error::ParseError(format!("Failed to serialize public key: {}", e)))?;
        ensure_parent_dir(public_path)?;
        fs::write(public_path, &public_toml).map_err(|e| {
            Error::IoError(format!(
                "Failed to write public key {}: {}",
                public_path.display(),
                e
            ))
        })?;

        Ok(())
    }

    /// Load a key pair from a private key file
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path).map_err(|e| {
            Error::IoError(format!("Failed to read key file {}: {}", path.display(), e))
        })?;

        let key_file: KeyFile = toml::from_str(&content).map_err(|e| {
            Error::ParseError(format!(
                "Failed to parse key file {}: {}",
                path.display(),
                e
            ))
        })?;

        if key_file.algorithm != "ed25519" {
            return Err(Error::ParseError(format!(
                "Unsupported key algorithm: {}",
                key_file.algorithm
            )));
        }

        let key_bytes = BASE64
            .decode(&key_file.key)
            .map_err(|e| Error::ParseError(format!("Invalid base64 in key file: {}", e)))?;

        let key_array: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| Error::ParseError("Invalid key length (expected 32 bytes)".to_string()))?;

        let signing_key = SigningKey::from_bytes(&key_array);

        Ok(Self {
            signing_key,
            key_id: key_file.key_id,
        })
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| {
            Error::IoError(format!(
                "Failed to create key directory {}: {}",
                parent.display(),
                e
            ))
        })?;
    }

    Ok(())
}

fn write_private_key_atomic(private_path: &Path, private_toml: &str) -> Result<()> {
    ensure_parent_dir(private_path)?;

    let (tmp_private_path, mut file) = create_private_key_temp_file(private_path)?;

    if let Err(error) = write_private_key_temp_file(&tmp_private_path, &mut file, private_toml) {
        drop(file);
        let _ = fs::remove_file(&tmp_private_path);
        return Err(error);
    }
    drop(file);

    let result = rename_private_key(&tmp_private_path, private_path);
    if result.is_err() {
        let _ = fs::remove_file(&tmp_private_path);
    }
    result
}

fn create_private_key_temp_file(private_path: &Path) -> Result<(PathBuf, fs::File)> {
    for _ in 0..PRIVATE_KEY_TEMP_ATTEMPTS {
        let tmp_private_path = unique_private_key_temp_path(private_path);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        match options.open(&tmp_private_path) {
            Ok(file) => return Ok((tmp_private_path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(Error::IoError(format!(
                    "Failed to write private key temp file {}: {}",
                    tmp_private_path.display(),
                    error
                )));
            }
        }
    }

    Err(Error::IoError(format!(
        "Failed to create unique private key temp file next to {} after {} attempts",
        private_path.display(),
        PRIVATE_KEY_TEMP_ATTEMPTS
    )))
}

fn unique_private_key_temp_path(private_path: &Path) -> PathBuf {
    let suffix = PRIVATE_KEY_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    private_path.with_extension(format!("private.tmp.{}.{}", std::process::id(), suffix))
}

fn write_private_key_temp_file(
    tmp_private_path: &Path,
    file: &mut fs::File,
    private_toml: &str,
) -> Result<()> {
    file.write_all(private_toml.as_bytes()).map_err(|e| {
        Error::IoError(format!(
            "Failed to write private key temp file {}: {}",
            tmp_private_path.display(),
            e
        ))
    })?;
    file.sync_all().map_err(|e| {
        Error::IoError(format!(
            "Failed to sync private key temp file {}: {}",
            tmp_private_path.display(),
            e
        ))
    })?;

    Ok(())
}

fn rename_private_key(tmp_private_path: &Path, private_path: &Path) -> Result<()> {
    fs::rename(tmp_private_path, private_path).map_err(|e| {
        Error::IoError(format!(
            "Failed to replace private key {} with temp file {}: {}",
            private_path.display(),
            tmp_private_path.display(),
            e
        ))
    })
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
    let content = fs::read_to_string(path).map_err(|e| {
        Error::IoError(format!(
            "Failed to read public key {}: {}",
            path.display(),
            e
        ))
    })?;

    let key_file: KeyFile = toml::from_str(&content).map_err(|e| {
        Error::ParseError(format!(
            "Failed to parse public key {}: {}",
            path.display(),
            e
        ))
    })?;

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
        keypair
            .verifying_key()
            .verify_strict(content, &sig)
            .unwrap();
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
    #[cfg(unix)]
    fn save_to_files_creates_private_key_0600() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let private_path = temp_dir.path().join("key.private");
        let public_path = temp_dir.path().join("key.public");

        SigningKeyPair::generate()
            .save_to_files(&private_path, &public_path)
            .unwrap();

        let mode = fs::metadata(&private_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn save_to_files_ignores_stale_fixed_temp_name_and_overwrites_private_key() {
        let temp_dir = TempDir::new().unwrap();
        let private_path = temp_dir.path().join("key.private");
        let public_path = temp_dir.path().join("key.public");
        let stale_temp_path =
            private_path.with_extension(format!("private.tmp.{}", std::process::id()));
        fs::write(&stale_temp_path, b"stale temp from earlier implementation").unwrap();

        let first_keypair = SigningKeyPair::generate().with_key_id("first-key");
        first_keypair
            .save_to_files(&private_path, &public_path)
            .unwrap();

        let replacement_keypair = SigningKeyPair::generate().with_key_id("replacement-key");
        let replacement_public = replacement_keypair.public_key_base64();
        replacement_keypair
            .save_to_files(&private_path, &public_path)
            .unwrap();

        let loaded = SigningKeyPair::load_from_file(&private_path).unwrap();
        assert_eq!(loaded.public_key_base64(), replacement_public);
        assert_eq!(loaded.key_id(), Some("replacement-key"));
        assert_eq!(
            fs::read(&stale_temp_path).unwrap(),
            b"stale temp from earlier implementation"
        );
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
