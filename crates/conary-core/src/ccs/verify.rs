// conary-core/src/ccs/verify.rs
//! CCS package verification
//!
//! Provides signature verification and content integrity checking for CCS packages.
//! Uses Ed25519 signatures for manifest authentication.

use crate::ccs::archive_reader::read_ccs_archive;
use crate::ccs::binary_manifest::MerkleTree;
use crate::ccs::builder::FileEntry;
use crate::hash;
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
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

    #[error("Merkle root mismatch: expected {expected}, got {actual}")]
    MerkleRootMismatch { expected: String, actual: String },

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
    /// Whether the embedded TOML manifest matches the CBOR manifest integrity hash
    pub toml_integrity_valid: bool,
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
    let file =
        File::open(path).with_context(|| format!("Failed to open package: {}", path.display()))?;

    let contents = read_ccs_archive(file)?;

    let manifest = &contents.manifest;

    // Collect files from components
    let files: Vec<FileEntry> = contents
        .components
        .values()
        .flat_map(|c| c.files.clone())
        .collect();

    // Parse signature JSON if present
    let signature: Option<PackageSignature> = contents
        .signature_raw
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;

    let mut warnings = Vec::new();

    let verified_v2 = if let Some(raw_manifest) = contents.v2_manifest_raw.as_deref() {
        Some(crate::ccs::v2::read_authority_document(
            raw_manifest,
            contents.signature_raw.as_deref(),
            contents.toml_raw.as_deref(),
            contents.v2_build_attestation_raw.as_deref(),
            contents.v2_foreign_conversion_boundary_raw.as_deref(),
            policy,
        )?)
    } else {
        None
    };

    // Verify signature (over raw manifest bytes - CBOR or TOML)
    let signature_status = if let Some(verified) = verified_v2.as_ref() {
        SignatureStatus::Valid {
            key_id: verified.signature.key_id.clone(),
            timestamp: verified.signature.timestamp.clone(),
        }
    } else {
        verify_signature(
            &contents.manifest_raw,
            signature.as_ref(),
            policy,
            &mut warnings,
        )?
    };

    // Verify content hashes
    let mut content_status = if let Some(verified) = verified_v2.as_ref() {
        verify_v2_archive_payload(&verified.authority, &contents.components, &contents.blobs)?
    } else {
        verify_content_hashes(&files, &contents.blobs)?
    };

    // Verify Merkle root against binary manifest's content_root.
    // A Merkle root mismatch is a structural integrity failure: also mark
    // content_status as Invalid so callers see the failure even if every
    // individual file hash looks correct.
    let mut merkle_valid = true;
    if let Some(ref bin_manifest) = contents.binary_manifest
        && !MerkleTree::verify_root(&bin_manifest.components, &bin_manifest.content_root)
    {
        let calculated = MerkleTree::calculate_root(&bin_manifest.components);
        let mismatch_msg = format!(
            "Merkle root mismatch: expected {}, got {}",
            bin_manifest.content_root.value, calculated.value
        );
        warnings.push(mismatch_msg.clone());
        merkle_valid = false;
        // Propagate into content_status so the structural failure is visible.
        content_status = match content_status {
            ContentStatus::Invalid { mut errors } => {
                errors.push(mismatch_msg);
                ContentStatus::Invalid { errors }
            }
            _ => ContentStatus::Invalid {
                errors: vec![mismatch_msg],
            },
        };
    }

    // Verify TOML integrity hash when the binary manifest contains one
    let mut toml_integrity_valid = true;
    if let Some(ref bin_manifest) = contents.binary_manifest
        && bin_manifest.toml_integrity_hash.is_some()
    {
        if let Some(ref toml_raw) = contents.toml_raw {
            if let Err(err) = verify_toml_integrity(toml_raw, bin_manifest) {
                warnings.push(err.to_string());
                toml_integrity_valid = false;
            }
        } else {
            // Binary manifest claims a TOML hash but no TOML was found in the archive
            warnings.push(
                "TOML integrity hash present in CBOR manifest but MANIFEST.toml is missing"
                    .to_string(),
            );
            toml_integrity_valid = false;
        }
    }

    let valid = merkle_valid
        && toml_integrity_valid
        && matches!(
            (&signature_status, &content_status),
            (
                SignatureStatus::Valid { .. } | SignatureStatus::Unsigned,
                ContentStatus::Valid { .. }
            )
        )
        && (policy.allow_unsigned || matches!(signature_status, SignatureStatus::Valid { .. }));

    Ok(VerificationResult {
        valid,
        package_name: manifest.package.name.clone(),
        package_version: manifest.package.version.clone(),
        signature_status,
        content_status,
        toml_integrity_valid,
        warnings,
    })
}

/// Verify that `MANIFEST.toml` matches the CBOR manifest integrity hash.
pub fn verify_toml_integrity(
    toml_raw: &[u8],
    bin_manifest: &crate::ccs::binary_manifest::BinaryManifest,
) -> Result<()> {
    let Some(expected_hash) = bin_manifest.toml_integrity_hash.as_ref() else {
        return Ok(());
    };
    let actual_hash = hash::sha256(toml_raw);
    if actual_hash != *expected_hash {
        bail!("TOML manifest integrity check failed: expected {expected_hash}, got {actual_hash}");
    }
    Ok(())
}

/// Verify the package signature
fn verify_signature(
    manifest_raw: &[u8],
    signature: Option<&PackageSignature>,
    policy: &TrustPolicy,
    warnings: &mut Vec<String>,
) -> Result<SignatureStatus> {
    verify_manifest_signature_with_warnings(manifest_raw, signature, policy, warnings)
}

pub(crate) fn verify_manifest_signature(
    manifest_raw: &[u8],
    signature: Option<&PackageSignature>,
    policy: &TrustPolicy,
) -> Result<SignatureStatus> {
    let mut warnings = Vec::new();
    verify_manifest_signature_with_warnings(manifest_raw, signature, policy, &mut warnings)
}

fn verify_manifest_signature_with_warnings(
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
    let sig_bytes = BASE64.decode(&sig.signature).map_err(|e| {
        VerifyError::InvalidSignatureFormat(format!("Invalid signature base64: {}", e))
    })?;

    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| VerifyError::InvalidSignatureFormat(format!("Invalid signature: {}", e)))?;

    // Decode public key
    let key_bytes = BASE64.decode(&sig.public_key).map_err(|e| {
        VerifyError::InvalidSignatureFormat(format!("Invalid public key base64: {}", e))
    })?;

    let verifying_key =
        VerifyingKey::from_bytes(&key_bytes.try_into().map_err(|_| {
            VerifyError::InvalidSignatureFormat("Public key must be 32 bytes".into())
        })?)
        .map_err(|e| VerifyError::InvalidSignatureFormat(format!("Invalid public key: {}", e)))?;

    // Verify signature
    verifying_key
        .verify_strict(manifest_raw, &signature)
        .map_err(|e| {
            VerifyError::SignatureInvalid(format!("Signature verification failed: {}", e))
        })?;

    // Check if key is trusted -- a valid signature is only meaningful if the
    // signing key is in the trusted set.  When trusted_keys is empty there are
    // no trust anchors, so a self-signed package must NOT be accepted as Valid.
    if !policy.trusted_keys.contains(&sig.public_key) {
        if policy.allow_unsigned {
            if policy.trusted_keys.is_empty() {
                warnings.push(
                    "Signature is cryptographically valid, but no trusted CCS keys are configured; \
                     self-signed packages remain untrusted"
                        .to_string(),
                );
            } else {
                warnings.push(format!(
                    "Signature valid but key not in trusted list: {:?}",
                    sig.key_id
                ));
            }
            return Ok(SignatureStatus::Untrusted {
                key_id: sig.key_id.clone(),
            });
        }
        return Err(
            VerifyError::TrustViolation(format!("Key not trusted: {:?}", sig.key_id)).into(),
        );
    }

    // Check timestamp if required
    if policy.require_timestamp && sig.timestamp.is_none() {
        return Err(VerifyError::TrustViolation(
            "Signature has no timestamp but policy requires one".to_string(),
        )
        .into());
    }

    // Validate timestamp format when policy requires timestamps
    if policy.require_timestamp
        && let Some(ts) = &sig.timestamp
        && chrono::DateTime::parse_from_rfc3339(ts).is_err()
    {
        return Err(VerifyError::TrustViolation(format!(
            "Signature timestamp is malformed: '{ts}'"
        ))
        .into());
    }

    // Check signature age if configured
    if policy.max_signature_age > 0
        && let Some(ts) = &sig.timestamp
    {
        match chrono::DateTime::parse_from_rfc3339(ts) {
            Ok(signed_time) => {
                let age = chrono::Utc::now().signed_duration_since(signed_time);
                if age.num_seconds() > policy.max_signature_age as i64 {
                    return Err(VerifyError::TrustViolation(format!(
                        "Signature expired: {} seconds old (max: {})",
                        age.num_seconds(),
                        policy.max_signature_age
                    ))
                    .into());
                }
            }
            Err(_) => {
                // Malformed timestamp with age policy -- cannot verify age
                return Err(VerifyError::TrustViolation(format!(
                    "Cannot verify signature age: malformed timestamp '{ts}'"
                ))
                .into());
            }
        }
    }

    Ok(SignatureStatus::Valid {
        key_id: sig.key_id.clone(),
        timestamp: sig.timestamp.clone(),
    })
}

fn verify_v2_archive_payload(
    authority: &crate::ccs::v2::AuthorityDocumentV2,
    components: &std::collections::HashMap<String, crate::ccs::builder::ComponentData>,
    blobs: &std::collections::HashMap<String, Vec<u8>>,
) -> Result<ContentStatus> {
    use crate::ccs::builder::FileType as LegacyFileType;
    use crate::ccs::v2::schema::{FileTypeV2, PackageKindV2};
    use std::collections::HashSet;

    let PackageKindV2::Package(data) = &authority.kind else {
        return Ok(ContentStatus::Skipped);
    };

    let mut errors = Vec::new();
    let signed_files: HashSet<(&str, &str)> = data
        .files
        .iter()
        .map(|file| (file.component.as_str(), file.path.as_str()))
        .collect();

    for (component_name, component) in components {
        if !authority.components.contains_key(component_name) {
            errors.push(format!(
                "v2 archive carries unsigned component {component_name}"
            ));
        }
        for component_file in &component.files {
            if !signed_files.contains(&(component_name.as_str(), component_file.path.as_str())) {
                errors.push(format!(
                    "v2 archive carries unsigned file {} in component {}",
                    component_file.path, component_name
                ));
            }
        }
    }

    for file in &data.files {
        let component = match components.get(&file.component) {
            Some(component) => component,
            None => {
                errors.push(format!(
                    "v2 file {} references missing component {}",
                    file.path, file.component
                ));
                continue;
            }
        };
        let Some(component_file) = component.files.iter().find(|item| item.path == file.path)
        else {
            errors.push(format!(
                "v2 signed file {} missing from component {}",
                file.path, file.component
            ));
            continue;
        };
        let expected_type = match file.file_type {
            FileTypeV2::Regular => LegacyFileType::Regular,
            FileTypeV2::Directory => LegacyFileType::Directory,
            FileTypeV2::Symlink => LegacyFileType::Symlink,
        };
        if component_file.hash != file.sha256
            || component_file.size != file.size
            || component_file.mode != file.mode
            || component_file.file_type != expected_type
            || component_file.component != file.component
            || component_file.target != file.symlink_target
        {
            errors.push(format!(
                "v2 file authority mismatch for {} in component {}",
                file.path, file.component
            ));
        }
        if matches!(file.file_type, FileTypeV2::Regular) {
            match blobs.get(&file.sha256) {
                Some(content)
                    if crate::hash::sha256(content) == file.sha256
                        && content.len() as u64 == file.size => {}
                Some(content) if content.len() as u64 != file.size => {
                    errors.push(format!("v2 blob size mismatch for {}", file.path));
                }
                Some(_) => errors.push(format!("v2 blob hash mismatch for {}", file.path)),
                None => errors.push(format!("v2 blob missing for {}", file.path)),
            }
        } else if matches!(file.file_type, FileTypeV2::Symlink) && file.symlink_target.is_none() {
            errors.push(format!("v2 symlink target missing for {}", file.path));
        }
    }

    if errors.is_empty() {
        Ok(ContentStatus::Valid {
            files_checked: data.files.len(),
        })
    } else {
        Ok(ContentStatus::Invalid { errors })
    }
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

        // Skip directories (use file_type instead of size/hash heuristic)
        if file.file_type == crate::ccs::builder::FileType::Directory {
            continue;
        }

        // Chunked files: verify each chunk hash instead of the whole-file hash
        if let Some(ref chunks) = file.chunks {
            let mut chunk_errors = false;
            for chunk_hash in chunks {
                match blobs.get(chunk_hash) {
                    Some(content) => {
                        let actual_hash = hash::sha256(content);
                        if actual_hash != *chunk_hash {
                            errors.push(format!(
                                "{}: chunk hash mismatch: expected {}, got {}",
                                file.path, chunk_hash, actual_hash
                            ));
                            chunk_errors = true;
                        }
                    }
                    None => {
                        errors.push(format!("{}: missing chunk blob {}", file.path, chunk_hash));
                        chunk_errors = true;
                    }
                }
            }
            if !chunk_errors {
                checked += 1;
            }
            continue;
        }

        // Non-chunked files: verify whole-file hash
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

    #[test]
    fn test_chunked_file_verification_valid() {
        use crate::ccs::builder::{FileEntry, FileType};

        let chunk1_data = b"chunk one data";
        let chunk2_data = b"chunk two data";
        let chunk1_hash = hash::sha256(chunk1_data);
        let chunk2_hash = hash::sha256(chunk2_data);

        let files = vec![FileEntry {
            path: "/usr/bin/app".to_string(),
            hash: "whole-file-hash-unused-for-chunked".to_string(),
            size: (chunk1_data.len() + chunk2_data.len()) as u64,
            mode: 0o755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: Some(vec![chunk1_hash.clone(), chunk2_hash.clone()]),
        }];

        let mut blobs = HashMap::new();
        blobs.insert(chunk1_hash, chunk1_data.to_vec());
        blobs.insert(chunk2_hash, chunk2_data.to_vec());

        let status = verify_content_hashes(&files, &blobs).unwrap();
        assert!(matches!(status, ContentStatus::Valid { files_checked: 1 }));
    }

    #[test]
    fn test_chunked_file_verification_missing_chunk() {
        use crate::ccs::builder::{FileEntry, FileType};

        let chunk1_data = b"chunk one data";
        let chunk1_hash = hash::sha256(chunk1_data);
        let missing_hash = "deadbeef".repeat(8);

        let files = vec![FileEntry {
            path: "/usr/bin/app".to_string(),
            hash: "unused".to_string(),
            size: 100,
            mode: 0o755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: Some(vec![chunk1_hash.clone(), missing_hash]),
        }];

        let mut blobs = HashMap::new();
        blobs.insert(chunk1_hash, chunk1_data.to_vec());

        let status = verify_content_hashes(&files, &blobs).unwrap();
        assert!(matches!(status, ContentStatus::Invalid { .. }));
    }

    #[test]
    fn test_chunked_file_verification_hash_mismatch() {
        use crate::ccs::builder::{FileEntry, FileType};

        let chunk_data = b"real data";
        let chunk_hash = hash::sha256(chunk_data);

        let files = vec![FileEntry {
            path: "/usr/bin/app".to_string(),
            hash: "unused".to_string(),
            size: chunk_data.len() as u64,
            mode: 0o755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: Some(vec![chunk_hash.clone()]),
        }];

        let mut blobs = HashMap::new();
        // Insert wrong content for the hash
        blobs.insert(chunk_hash, b"tampered data".to_vec());

        let status = verify_content_hashes(&files, &blobs).unwrap();
        assert!(matches!(status, ContentStatus::Invalid { .. }));
    }

    /// Helper: create a self-signed PackageSignature for test manifest data.
    /// Uses deterministic key bytes to avoid OsRng/rand_core version conflicts
    /// in the test compilation environment.
    fn make_self_signed_signature(manifest: &[u8]) -> (PackageSignature, String) {
        use ed25519_dalek::{Signer, SigningKey};

        // Deterministic 32-byte seed (any fixed bytes work for testing)
        let seed: [u8; 32] = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32,
        ];
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let public_key_b64 = BASE64.encode(verifying_key.as_bytes());

        let sig = signing_key.sign(manifest);
        let sig_b64 = BASE64.encode(sig.to_bytes());

        let pkg_sig = PackageSignature {
            algorithm: "ed25519".to_string(),
            signature: sig_b64,
            public_key: public_key_b64.clone(),
            key_id: Some("attacker-key".to_string()),
            timestamp: Some("2026-01-01T00:00:00Z".to_string()),
        };

        (pkg_sig, public_key_b64)
    }

    #[test]
    fn test_self_signed_package_returns_untrusted_with_empty_trusted_keys() {
        // An attacker embeds their own keypair and self-signs the manifest.
        // With no trusted keys configured (the default), this must NOT return Valid.
        let manifest = b"fake manifest content";
        let (sig, _pub_key) = make_self_signed_signature(manifest);

        // Use permissive policy so verify_signature returns Ok(Untrusted)
        // instead of Err(TrustViolation) when the key is not in the trust list.
        let policy = TrustPolicy::permissive(); // trusted_keys is empty, allow_unsigned = true
        let mut warnings = Vec::new();

        let status = verify_signature(manifest, Some(&sig), &policy, &mut warnings).unwrap();

        assert!(
            matches!(status, SignatureStatus::Untrusted { .. }),
            "Expected Untrusted when trusted_keys is empty, got {:?}",
            status
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("no trusted CCS keys are configured")),
            "Expected explicit trust-anchor warning, got {:?}",
            warnings
        );
    }

    #[test]
    fn test_self_signed_package_returns_valid_when_key_trusted() {
        // When the key IS in trusted_keys, the signature should be Valid.
        let manifest = b"trusted manifest content";
        let (sig, pub_key) = make_self_signed_signature(manifest);

        let policy = TrustPolicy {
            trusted_keys: vec![pub_key],
            ..Default::default()
        };
        let mut warnings = Vec::new();

        let status = verify_signature(manifest, Some(&sig), &policy, &mut warnings).unwrap();

        assert!(
            matches!(status, SignatureStatus::Valid { .. }),
            "Expected Valid when key is in trusted_keys, got {:?}",
            status
        );
    }

    #[test]
    fn test_self_signed_package_returns_untrusted_when_key_not_in_list() {
        // Signature is valid but the key is not in the trusted list.
        let manifest = b"some manifest content";
        let (sig, _pub_key) = make_self_signed_signature(manifest);

        let policy = TrustPolicy {
            trusted_keys: vec!["some-other-key-base64".to_string()],
            allow_unsigned: true, // permissive but with a trust list
            ..Default::default()
        };
        let mut warnings = Vec::new();

        let status = verify_signature(manifest, Some(&sig), &policy, &mut warnings).unwrap();

        assert!(
            matches!(status, SignatureStatus::Untrusted { .. }),
            "Expected Untrusted when key is not in trusted_keys, got {:?}",
            status
        );
    }

    #[test]
    fn verify_package_rejects_tampered_attestation_toml() {
        let temp = tempfile::tempdir().unwrap();
        let key = crate::ccs::signing::SigningKeyPair::generate().with_key_id("test-publisher");
        let package_path = temp.path().join("signed.ccs");
        let tampered_path = temp.path().join("tampered.ccs");

        let mut result = crate::ccs::builder::test_support::minimal_build_result("attested", "1.0");
        result
            .manifest
            .provenance
            .get_or_insert_with(Default::default)
            .build_attestation =
            Some(crate::ccs::attestation::test_support::sample_envelope_for_tests(&key));
        crate::ccs::builder::write_signed_ccs_package(&result, &package_path, &key).unwrap();
        crate::ccs::builder::test_support::rewrite_manifest_toml_for_tests(
            &package_path,
            &tampered_path,
            |toml| toml.replace("m2-policy-v1", "m2-policy-mutated"),
        )
        .unwrap();

        let verification = verify_package(
            &tampered_path,
            &TrustPolicy::strict(vec![key.public_key_base64()]),
        )
        .unwrap();

        assert!(!verification.toml_integrity_valid);
        assert!(
            verification
                .warnings
                .iter()
                .any(|warning| warning.contains("TOML manifest integrity check failed")),
            "{:?}",
            verification.warnings
        );
    }

    fn write_v2_archive_for_verify_test(
        path: &Path,
        mutate_component: impl FnOnce(
            &mut crate::ccs::builder::ComponentData,
            &mut HashMap<String, Vec<u8>>,
        ),
    ) -> String {
        use crate::ccs::builder::{ComponentData, FileEntry, FileType};
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        use tar::Builder;

        fn append_bytes<W: Write>(builder: &mut Builder<W>, path: &str, bytes: &[u8]) {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, bytes).unwrap();
        }

        let authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("verify-v2");
        let raw = authority.to_cbor().unwrap();
        let key = crate::ccs::signing::SigningKeyPair::generate();
        let public_key = key.public_key_base64();
        let signature = serde_json::to_vec(&key.sign(&raw)).unwrap();
        let hello_hash = crate::hash::sha256(b"hello world\n");
        let mut component = ComponentData {
            name: "main".to_string(),
            files: vec![FileEntry {
                path: "/usr/bin/hello".to_string(),
                hash: hello_hash.clone(),
                size: 12,
                mode: 0o755,
                component: "main".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            }],
            hash: "component-hash-unused-by-v2-verify-test".to_string(),
            size: 12,
        };
        let mut blobs = HashMap::from([(hello_hash, b"hello world\n".to_vec())]);
        mutate_component(&mut component, &mut blobs);
        let component_json = serde_json::to_vec(&component).unwrap();

        let file = std::fs::File::create(path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = Builder::new(encoder);
        append_bytes(&mut builder, "MANIFEST", &raw);
        append_bytes(&mut builder, "MANIFEST.sig", &signature);
        append_bytes(&mut builder, "components/main.json", &component_json);
        for (hash, content) in blobs {
            let object_path = format!("objects/{}/{}", &hash[..2], &hash[2..]);
            append_bytes(&mut builder, &object_path, &content);
        }
        builder.into_inner().unwrap().finish().unwrap();
        public_key
    }

    #[test]
    fn verify_package_rejects_v2_component_mode_mismatch_against_signed_authority() {
        let temp = tempfile::tempdir().unwrap();
        let package_path = temp.path().join("mode-mismatch.ccs");
        let public_key = write_v2_archive_for_verify_test(&package_path, |component, _blobs| {
            component.files[0].mode = 0o644;
        });

        let verification =
            verify_package(&package_path, &TrustPolicy::strict(vec![public_key])).unwrap();

        assert!(!verification.valid);
        assert!(matches!(
            verification.content_status,
            ContentStatus::Invalid { .. }
        ));
    }

    #[test]
    fn verify_package_rejects_unsigned_extra_v2_component_file() {
        use crate::ccs::builder::{FileEntry, FileType};

        let temp = tempfile::tempdir().unwrap();
        let package_path = temp.path().join("extra-file.ccs");
        let public_key = write_v2_archive_for_verify_test(&package_path, |component, blobs| {
            let extra_hash = crate::hash::sha256(b"extra\n");
            component.files.push(FileEntry {
                path: "/usr/bin/extra".to_string(),
                hash: extra_hash.clone(),
                size: 6,
                mode: 0o755,
                component: "main".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            });
            blobs.insert(extra_hash, b"extra\n".to_vec());
        });

        let verification =
            verify_package(&package_path, &TrustPolicy::strict(vec![public_key])).unwrap();

        assert!(!verification.valid);
        assert!(matches!(
            verification.content_status,
            ContentStatus::Invalid { .. }
        ));
    }

    #[test]
    fn verify_v2_package_rejects_payload_not_in_signed_authority() {
        let temp = tempfile::tempdir().unwrap();
        let package_path = temp.path().join("tampered-v2.ccs");
        let signer = crate::ccs::signing::SigningKeyPair::generate();
        let authority =
            crate::ccs::v2::test_support::package_authority_with_one_file("tampered-v2");
        let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
        crate::ccs::builder::write_v2_ccs_package(
            &authority,
            &payloads,
            &package_path,
            &signer,
            None,
            None,
            None,
        )
        .unwrap();

        let tampered_path = temp.path().join("tampered-v2-extra.ccs");
        crate::ccs::v2::test_support::rewrite_v2_archive_for_tests(
            &package_path,
            &tampered_path,
            |entries| {
                entries.insert(
                    "components/extra.json".to_string(),
                    br#"{"name":"extra","files":[],"hash":"sha256:extra","size":0}"#.to_vec(),
                );
            },
        )
        .unwrap();

        let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);
        let result = verify_package(&tampered_path, &policy).unwrap();
        assert!(!result.valid);
        assert!(matches!(
            result.content_status,
            ContentStatus::Invalid { .. }
        ));
    }
}
