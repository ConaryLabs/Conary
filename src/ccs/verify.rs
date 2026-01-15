// src/ccs/verify.rs
//! CCS package verification
//!
//! Provides signature verification and content integrity checking for CCS packages.
//! Uses Ed25519 signatures for manifest authentication.

use crate::ccs::builder::{ComponentData, FileEntry};
use crate::ccs::manifest::CcsManifest;
use crate::hash;
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tar::Archive;
use thiserror::Error;

/// Verification errors
#[derive(Error, Debug)]
pub enum VerifyError {
    #[error("Package is not signed")]
    NotSigned,

    #[error("Invalid signature format: {0}")]
    InvalidSignatureFormat(String),

    #[error("Signature verification failed: {0}")]
    SignatureInvalid(String),

    #[error("Content hash mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("Missing content blob: {0}")]
    MissingBlob(String),

    #[error("Trust policy violation: {0}")]
    TrustViolation(String),

    #[error("Package structure error: {0}")]
    PackageError(String),
}

/// Signature data embedded in a CCS package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageSignature {
    /// Signature algorithm (currently only "ed25519")
    pub algorithm: String,
    /// Base64-encoded signature bytes
    pub signature: String,
    /// Base64-encoded public key
    pub public_key: String,
    /// Optional key identifier (fingerprint or name)
    #[serde(default)]
    pub key_id: Option<String>,
    /// Timestamp when signed (RFC 3339)
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// Trust policy for signature verification
#[derive(Debug, Clone, Default)]
pub struct TrustPolicy {
    /// Trusted public keys (base64-encoded)
    pub trusted_keys: Vec<String>,
    /// Whether to allow unsigned packages
    pub allow_unsigned: bool,
    /// Whether to require timestamp
    pub require_timestamp: bool,
    /// Maximum age of signature in seconds (0 = no limit)
    pub max_signature_age: u64,
}

impl TrustPolicy {
    /// Create a permissive policy that allows unsigned packages
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
            require_timestamp: true,
            max_signature_age: 0,
        }
    }

    /// Load policy from a TOML file
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read trust policy: {}", path.display()))?;
        Self::from_toml(&content)
    }

    /// Parse policy from TOML string
    pub fn from_toml(content: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct PolicyFile {
            #[serde(default)]
            trusted_keys: Vec<String>,
            #[serde(default)]
            allow_unsigned: bool,
            #[serde(default)]
            require_timestamp: bool,
            #[serde(default)]
            max_signature_age: u64,
        }

        let parsed: PolicyFile = toml::from_str(content)?;
        Ok(Self {
            trusted_keys: parsed.trusted_keys,
            allow_unsigned: parsed.allow_unsigned,
            require_timestamp: parsed.require_timestamp,
            max_signature_age: parsed.max_signature_age,
        })
    }
}

/// Result of package verification
#[derive(Debug)]
pub struct VerificationResult {
    /// Whether verification succeeded
    pub valid: bool,
    /// Package name
    pub package_name: String,
    /// Package version
    pub package_version: String,
    /// Signature status
    pub signature_status: SignatureStatus,
    /// Content verification status
    pub content_status: ContentStatus,
    /// Any warnings (non-fatal issues)
    pub warnings: Vec<String>,
}

/// Signature verification status
#[derive(Debug, Clone)]
pub enum SignatureStatus {
    /// Package is properly signed and verified
    Valid {
        key_id: Option<String>,
        timestamp: Option<String>,
    },
    /// Package is unsigned
    Unsigned,
    /// Signature is invalid
    Invalid(String),
    /// Signature is valid but not trusted
    Untrusted { key_id: Option<String> },
}

/// Content verification status
#[derive(Debug, Clone)]
pub enum ContentStatus {
    /// All content hashes verified
    Valid { files_checked: usize },
    /// Some content failed verification
    Invalid { errors: Vec<String> },
    /// Content verification was skipped
    Skipped,
}

/// Verify a CCS package
pub fn verify_package(path: &Path, policy: &TrustPolicy) -> Result<VerificationResult> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open package: {}", path.display()))?;

    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    let mut manifest: Option<CcsManifest> = None;
    let mut manifest_raw: Option<Vec<u8>> = None;
    let mut signature: Option<PackageSignature> = None;
    let mut blobs: HashMap<String, Vec<u8>> = HashMap::new();
    let mut components: HashMap<String, ComponentData> = HashMap::new();

    // First pass: extract metadata and signature
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?;
        let entry_path_str = entry_path.to_string_lossy().to_string();

        // Prefer CBOR MANIFEST over TOML
        if entry_path_str == "MANIFEST" || entry_path_str == "./MANIFEST" {
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            manifest_raw = Some(content.clone());
            // Convert CBOR to CcsManifest for compatibility
            if let Ok(bin_manifest) = crate::ccs::binary_manifest::BinaryManifest::from_cbor(&content) {
                manifest = Some(crate::ccs::package::convert_binary_to_ccs_manifest(&bin_manifest));
            }
        } else if manifest.is_none() && (entry_path_str == "MANIFEST.toml" || entry_path_str == "./MANIFEST.toml") {
            let mut content = String::new();
            entry.read_to_string(&mut content)?;
            manifest_raw = Some(content.as_bytes().to_vec());
            manifest = Some(CcsManifest::parse(&content)?);
        } else if entry_path_str == "MANIFEST.sig" || entry_path_str == "./MANIFEST.sig" {
            let mut content = String::new();
            entry.read_to_string(&mut content)?;
            signature = Some(serde_json::from_str(&content)?);
        } else if (entry_path_str.starts_with("components/") || entry_path_str.starts_with("./components/"))
            && entry_path_str.ends_with(".json")
        {
            let mut content = String::new();
            entry.read_to_string(&mut content)?;
            if let Ok(comp) = serde_json::from_str::<ComponentData>(&content) {
                components.insert(comp.name.clone(), comp);
            }
        } else if entry_path_str.starts_with("objects/") || entry_path_str.starts_with("./objects/")
        {
            // Extract blob hash from path: objects/{prefix}/{suffix} -> {prefix}{suffix}
            let parts: Vec<&str> = entry_path_str.split('/').collect();
            if parts.len() >= 3 {
                let hash_str = format!(
                    "{}{}",
                    parts[parts.len() - 2],
                    parts[parts.len() - 1]
                );
                let mut content = Vec::new();
                entry.read_to_end(&mut content)?;
                blobs.insert(hash_str, content);
            }
        }
    }

    let manifest = manifest.ok_or_else(|| VerifyError::PackageError("Missing MANIFEST".into()))?;
    let manifest_raw =
        manifest_raw.ok_or_else(|| VerifyError::PackageError("Missing MANIFEST".into()))?;

    // Collect files from components
    let files: Vec<FileEntry> = components
        .values()
        .flat_map(|c| c.files.clone())
        .collect();

    let mut warnings = Vec::new();

    // Verify signature (over raw manifest bytes - CBOR or TOML)
    let signature_status = verify_signature(&manifest_raw, signature.as_ref(), policy, &mut warnings)?;

    // Verify content hashes
    let content_status = verify_content_hashes(&files, &blobs)?;

    let valid = matches!(
        (&signature_status, &content_status),
        (SignatureStatus::Valid { .. } | SignatureStatus::Unsigned, ContentStatus::Valid { .. })
    ) && (policy.allow_unsigned || matches!(signature_status, SignatureStatus::Valid { .. }));

    Ok(VerificationResult {
        valid,
        package_name: manifest.package.name.clone(),
        package_version: manifest.package.version.clone(),
        signature_status,
        content_status,
        warnings,
    })
}

/// Verify the package signature
fn verify_signature(
    manifest_raw: &[u8],
    signature: Option<&PackageSignature>,
    policy: &TrustPolicy,
    warnings: &mut Vec<String>,
) -> Result<SignatureStatus> {
    let Some(sig) = signature else {
        if policy.allow_unsigned {
            return Ok(SignatureStatus::Unsigned);
        }
        return Err(VerifyError::NotSigned.into());
    };

    // Validate algorithm
    if sig.algorithm != "ed25519" {
        return Err(VerifyError::InvalidSignatureFormat(format!(
            "Unsupported algorithm: {}",
            sig.algorithm
        ))
        .into());
    }

    // Decode signature
    let sig_bytes = BASE64
        .decode(&sig.signature)
        .map_err(|e| VerifyError::InvalidSignatureFormat(format!("Invalid signature base64: {}", e)))?;

    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| VerifyError::InvalidSignatureFormat(format!("Invalid signature: {}", e)))?;

    // Decode public key
    let key_bytes = BASE64
        .decode(&sig.public_key)
        .map_err(|e| VerifyError::InvalidSignatureFormat(format!("Invalid public key base64: {}", e)))?;

    let verifying_key = VerifyingKey::from_bytes(&key_bytes.try_into().map_err(|_| {
        VerifyError::InvalidSignatureFormat("Public key must be 32 bytes".into())
    })?)
    .map_err(|e| VerifyError::InvalidSignatureFormat(format!("Invalid public key: {}", e)))?;

    // Verify signature
    verifying_key
        .verify(manifest_raw, &signature)
        .map_err(|e| VerifyError::SignatureInvalid(format!("Signature verification failed: {}", e)))?;

    // Check if key is trusted
    if !policy.trusted_keys.is_empty() && !policy.trusted_keys.contains(&sig.public_key) {
        if policy.allow_unsigned {
            warnings.push(format!(
                "Signature valid but key not in trusted list: {:?}",
                sig.key_id
            ));
            return Ok(SignatureStatus::Untrusted {
                key_id: sig.key_id.clone(),
            });
        }
        return Err(VerifyError::TrustViolation(format!(
            "Key not trusted: {:?}",
            sig.key_id
        ))
        .into());
    }

    // Check timestamp if required
    if policy.require_timestamp && sig.timestamp.is_none() {
        warnings.push("Signature has no timestamp".to_string());
    }

    // Check signature age if configured
    if policy.max_signature_age > 0
        && let Some(ts) = &sig.timestamp
        && let Ok(signed_time) = chrono::DateTime::parse_from_rfc3339(ts)
    {
        let age = chrono::Utc::now().signed_duration_since(signed_time);
        if age.num_seconds() > policy.max_signature_age as i64 {
            warnings.push(format!(
                "Signature is {} seconds old (max: {})",
                age.num_seconds(),
                policy.max_signature_age
            ));
        }
    }

    Ok(SignatureStatus::Valid {
        key_id: sig.key_id.clone(),
        timestamp: sig.timestamp.clone(),
    })
}

/// Verify content hashes match
fn verify_content_hashes(
    files: &[FileEntry],
    blobs: &HashMap<String, Vec<u8>>,
) -> Result<ContentStatus> {
    let mut errors = Vec::new();
    let mut checked = 0;

    for file in files {
        // Skip symlinks (they don't have content in blobs)
        if file.target.is_some() {
            continue;
        }

        // Skip directories
        if file.size == 0 && file.hash.is_empty() {
            continue;
        }

        match blobs.get(&file.hash) {
            Some(content) => {
                let actual_hash = hash::sha256(content);
                if actual_hash != file.hash {
                    errors.push(format!(
                        "{}: expected {}, got {}",
                        file.path, file.hash, actual_hash
                    ));
                } else {
                    checked += 1;
                }
            }
            None => {
                // Check if it might be a symlink stored differently
                if file.size > 0 {
                    errors.push(format!("{}: missing blob {}", file.path, file.hash));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(ContentStatus::Valid {
            files_checked: checked,
        })
    } else {
        Ok(ContentStatus::Invalid { errors })
    }
}

/// Print verification result in human-readable format
pub fn print_result(result: &VerificationResult) {
    let status_icon = if result.valid { "[OK]" } else { "[FAILED]" };

    println!(
        "{} {} v{}",
        status_icon, result.package_name, result.package_version
    );
    println!();

    // Signature status
    print!("Signature: ");
    match &result.signature_status {
        SignatureStatus::Valid { key_id, timestamp } => {
            println!("[VALID]");
            if let Some(id) = key_id {
                println!("  Key ID: {}", id);
            }
            if let Some(ts) = timestamp {
                println!("  Signed: {}", ts);
            }
        }
        SignatureStatus::Unsigned => {
            println!("[UNSIGNED]");
        }
        SignatureStatus::Invalid(reason) => {
            println!("[INVALID] {}", reason);
        }
        SignatureStatus::Untrusted { key_id } => {
            println!("[UNTRUSTED]");
            if let Some(id) = key_id {
                println!("  Key ID: {}", id);
            }
        }
    }

    // Content status
    println!();
    print!("Content: ");
    match &result.content_status {
        ContentStatus::Valid { files_checked } => {
            println!("[VALID] {} files verified", files_checked);
        }
        ContentStatus::Invalid { errors } => {
            println!("[INVALID]");
            for err in errors {
                println!("  - {}", err);
            }
        }
        ContentStatus::Skipped => {
            println!("[SKIPPED]");
        }
    }

    // Warnings
    if !result.warnings.is_empty() {
        println!();
        println!("Warnings:");
        for warning in &result.warnings {
            println!("  - {}", warning);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trust_policy_permissive() {
        let policy = TrustPolicy::permissive();
        assert!(policy.allow_unsigned);
        assert!(policy.trusted_keys.is_empty());
    }

    #[test]
    fn test_trust_policy_strict() {
        let keys = vec!["key1".to_string(), "key2".to_string()];
        let policy = TrustPolicy::strict(keys.clone());
        assert!(!policy.allow_unsigned);
        assert_eq!(policy.trusted_keys, keys);
    }

    #[test]
    fn test_trust_policy_from_toml() {
        let toml = r#"
            trusted_keys = ["abc123", "def456"]
            allow_unsigned = false
            require_timestamp = true
            max_signature_age = 86400
        "#;

        let policy = TrustPolicy::from_toml(toml).unwrap();
        assert_eq!(policy.trusted_keys.len(), 2);
        assert!(!policy.allow_unsigned);
        assert!(policy.require_timestamp);
        assert_eq!(policy.max_signature_age, 86400);
    }

    #[test]
    fn test_content_status_valid() {
        let files = vec![];
        let blobs = HashMap::new();

        let status = verify_content_hashes(&files, &blobs).unwrap();
        assert!(matches!(status, ContentStatus::Valid { files_checked: 0 }));
    }
}
