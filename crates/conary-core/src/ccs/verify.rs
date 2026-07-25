// conary-core/src/ccs/verify.rs

//! Trusted CCS v2 package verification.
//!
//! `archive_reader` is an explicitly untrusted structural decoder. This module
//! is the only path that turns those bytes into a package-trust capability.

use crate::ccs::archive_reader::{UntrustedCcsArchive, inspect_untrusted_ccs_archive};
use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::path::Path;
use thiserror::Error;

mod payload;

use payload::verify_v2_archive_payload;

#[derive(Error, Debug)]
pub enum VerifyError {
    #[error("CCS v2 package is not signed")]
    NotSigned,
    #[error("invalid CCS v2 signature format: {0}")]
    InvalidSignatureFormat(String),
    #[error("CCS v2 signature verification failed: {0}")]
    SignatureInvalid(String),
    #[error("CCS v2 package signer is not trusted: {0}")]
    TrustViolation(String),
    #[error("CCS v2 payload authority failed: {0}")]
    PayloadInvalid(String),
    #[error("CCS v2 package structure failed: {0}")]
    PackageError(String),
}

/// Signature data embedded in a current CCS package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSignature {
    pub algorithm: String,
    pub signature: String,
    pub public_key: String,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// Exact trust anchors and timestamp constraints for CCS verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustPolicy {
    trusted_keys: Vec<String>,
    require_timestamp: bool,
    max_signature_age: u64,
}

impl TrustPolicy {
    /// Require a signature from one of the supplied Ed25519 public keys.
    pub fn strict(trusted_keys: Vec<String>) -> Self {
        Self {
            trusted_keys,
            require_timestamp: true,
            max_signature_age: 0,
        }
    }

    pub fn with_timestamp_required(mut self, required: bool) -> Self {
        self.require_timestamp = required;
        self
    }

    pub fn with_max_signature_age(mut self, seconds: u64) -> Self {
        self.max_signature_age = seconds;
        self
    }

    pub fn trusted_keys(&self) -> &[String] {
        &self.trusted_keys
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("read CCS trust policy {}", path.display()))?;
        Self::from_toml(&content)
    }

    pub fn from_toml(content: &str) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PolicyFile {
            trusted_keys: Vec<String>,
            #[serde(default = "default_true")]
            require_timestamp: bool,
            #[serde(default)]
            max_signature_age: u64,
        }

        fn default_true() -> bool {
            true
        }

        let parsed: PolicyFile = toml::from_str(content).context("parse CCS trust policy")?;
        let policy = Self {
            trusted_keys: parsed.trusted_keys,
            require_timestamp: parsed.require_timestamp,
            max_signature_age: parsed.max_signature_age,
        };
        policy.validate()?;
        Ok(policy)
    }

    fn validate(&self) -> Result<()> {
        if self.trusted_keys.is_empty() {
            return Err(VerifyError::TrustViolation(
                "no trusted CCS package signing keys are configured".to_string(),
            )
            .into());
        }
        let mut unique = BTreeSet::new();
        for key in &self.trusted_keys {
            let bytes = BASE64.decode(key).map_err(|error| {
                VerifyError::InvalidSignatureFormat(format!(
                    "trusted public key is not base64: {error}"
                ))
            })?;
            let key_bytes: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
                VerifyError::InvalidSignatureFormat(format!(
                    "trusted public key decoded to {} bytes; expected 32",
                    bytes.len()
                ))
            })?;
            VerifyingKey::from_bytes(&key_bytes).map_err(|error| {
                VerifyError::InvalidSignatureFormat(format!(
                    "trusted public key is not Ed25519: {error}"
                ))
            })?;
            if !unique.insert(key) {
                return Err(VerifyError::TrustViolation(
                    "trusted CCS package key set contains a duplicate key".to_string(),
                )
                .into());
            }
        }
        Ok(())
    }
}

/// Authenticated current CCS archive.
///
/// Construction is private to `verify_package`; consumers use this value as
/// proof that current authority, package signature, projections, and payload
/// bytes agreed under the supplied trust policy.
#[derive(Debug, Clone)]
pub struct VerifiedCcsArchive {
    archive: UntrustedCcsArchive,
    signature: PackageSignature,
    files_checked: usize,
}

impl VerifiedCcsArchive {
    pub fn authority(&self) -> &crate::ccs::v2::AuthorityDocumentV2 {
        &self.archive.v2_authority
    }

    pub fn signature(&self) -> &PackageSignature {
        &self.signature
    }

    pub fn package_name(&self) -> &str {
        &self.archive.v2_authority.identity.name
    }

    pub fn package_version(&self) -> &str {
        &self.archive.v2_authority.identity.version
    }

    pub fn files_checked(&self) -> usize {
        self.files_checked
    }

    pub(crate) fn archive(&self) -> &UntrustedCcsArchive {
        &self.archive
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContentStatus {
    Valid { files_checked: usize },
    Invalid { errors: Vec<String> },
    Skipped,
}

/// Verify current CCS authority, signature trust, diagnostic projections, and
/// every payload object before returning an install/publication capability.
pub fn verify_package(path: &Path, policy: &TrustPolicy) -> Result<VerifiedCcsArchive> {
    policy.validate()?;
    let file = File::open(path).with_context(|| format!("open CCS package {}", path.display()))?;
    let archive = inspect_untrusted_ccs_archive(file)
        .with_context(|| format!("inspect CCS v2 archive {}", path.display()))?;

    let verified = crate::ccs::v2::read_authority_document(
        &archive.v2_manifest_raw,
        archive.signature_raw.as_deref(),
        archive.toml_raw.as_deref(),
        archive.v2_build_attestation_raw.as_deref(),
        archive.v2_foreign_conversion_boundary_raw.as_deref(),
        policy,
    )?;
    if verified.authority != archive.v2_authority {
        return Err(VerifyError::PackageError(
            "decoded authority changed between archive inspection and verification".to_string(),
        )
        .into());
    }
    if verified.build_attestation != archive.v2_build_attestation
        || verified.foreign_conversion_boundary != archive.v2_foreign_conversion_boundary
    {
        return Err(VerifyError::PackageError(
            "decoded v2 evidence changed between archive inspection and verification".to_string(),
        )
        .into());
    }

    let files_checked = match verify_v2_archive_payload(
        &verified.authority,
        &archive.components,
        &archive.blobs,
    )? {
        ContentStatus::Valid { files_checked } => files_checked,
        ContentStatus::Invalid { errors } => {
            return Err(VerifyError::PayloadInvalid(errors.join("; ")).into());
        }
        ContentStatus::Skipped => 0,
    };
    verify_exact_component_summaries(&archive)?;
    verify_no_unreferenced_objects(&archive)?;

    Ok(VerifiedCcsArchive {
        archive,
        signature: verified.signature,
        files_checked,
    })
}

fn verify_exact_component_summaries(archive: &UntrustedCcsArchive) -> Result<()> {
    let signed_names = archive
        .v2_authority
        .components
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let archived_names = archive
        .components
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if signed_names != archived_names {
        return Err(VerifyError::PayloadInvalid(format!(
            "signed component set {signed_names:?} disagrees with archived set {archived_names:?}"
        ))
        .into());
    }

    for (name, signed) in &archive.v2_authority.components {
        let component = archive
            .components
            .get(name)
            .expect("component sets were proven equal");
        let file_count = u32::try_from(component.files.len()).map_err(|_| {
            VerifyError::PayloadInvalid(format!("component {name:?} file count exceeds u32"))
        })?;
        let total_size = component.files.iter().try_fold(0_u64, |total, file| {
            total.checked_add(file.content.as_ref().map_or(0, |content| content.size))
        });
        let total_size = total_size.ok_or_else(|| {
            VerifyError::PayloadInvalid(format!("component {name:?} size overflows u64"))
        })?;
        if file_count != signed.file_count || total_size != signed.total_size {
            return Err(VerifyError::PayloadInvalid(format!(
                "component {name:?} summary disagrees with signed authority: \
                 files {file_count}/{}, bytes {total_size}/{}",
                signed.file_count, signed.total_size
            ))
            .into());
        }
    }
    Ok(())
}

fn verify_no_unreferenced_objects(archive: &UntrustedCcsArchive) -> Result<()> {
    use crate::ccs::v2::schema::PackageKindV2;

    let expected = match &archive.v2_authority.kind {
        PackageKindV2::Package(package) => package
            .files
            .iter()
            .filter_map(|file| file.content.as_ref().map(|content| content.sha256.as_str()))
            .collect::<BTreeSet<_>>(),
        _ => BTreeSet::new(),
    };
    let archived = archive
        .blobs
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if expected != archived {
        return Err(VerifyError::PayloadInvalid(format!(
            "signed object set {expected:?} disagrees with archived set {archived:?}"
        ))
        .into());
    }
    Ok(())
}

/// Verify one manifest signature against exact archived bytes.
pub(crate) fn verify_manifest_signature(
    manifest_raw: &[u8],
    signature: &PackageSignature,
    policy: &TrustPolicy,
) -> Result<()> {
    policy.validate()?;
    if signature.algorithm != "ed25519" {
        return Err(VerifyError::InvalidSignatureFormat(format!(
            "unsupported algorithm {:?}",
            signature.algorithm
        ))
        .into());
    }
    let sig_bytes = BASE64.decode(&signature.signature).map_err(|error| {
        VerifyError::InvalidSignatureFormat(format!("signature is not base64: {error}"))
    })?;
    let signature_bytes = Signature::from_slice(&sig_bytes).map_err(|error| {
        VerifyError::InvalidSignatureFormat(format!("invalid Ed25519 signature bytes: {error}"))
    })?;
    let key_bytes = BASE64.decode(&signature.public_key).map_err(|error| {
        VerifyError::InvalidSignatureFormat(format!("public key is not base64: {error}"))
    })?;
    let key_bytes: [u8; 32] = key_bytes.try_into().map_err(|bytes: Vec<u8>| {
        VerifyError::InvalidSignatureFormat(format!(
            "public key decoded to {} bytes; expected 32",
            bytes.len()
        ))
    })?;
    let verifying_key = VerifyingKey::from_bytes(&key_bytes).map_err(|error| {
        VerifyError::InvalidSignatureFormat(format!("invalid Ed25519 public key: {error}"))
    })?;
    verifying_key
        .verify_strict(manifest_raw, &signature_bytes)
        .map_err(|error| VerifyError::SignatureInvalid(error.to_string()))?;

    if !policy.trusted_keys.contains(&signature.public_key) {
        return Err(VerifyError::TrustViolation(format!("key_id={:?}", signature.key_id)).into());
    }
    if policy.require_timestamp && signature.timestamp.is_none() {
        return Err(
            VerifyError::TrustViolation("signature timestamp is required".to_string()).into(),
        );
    }
    if let Some(timestamp) = &signature.timestamp {
        let signed_time = chrono::DateTime::parse_from_rfc3339(timestamp).map_err(|_| {
            VerifyError::TrustViolation(format!("signature timestamp is malformed: {timestamp:?}"))
        })?;
        if policy.max_signature_age > 0 {
            let age = chrono::Utc::now().signed_duration_since(signed_time);
            if age.num_seconds() < 0 {
                return Err(VerifyError::TrustViolation(
                    "signature timestamp is in the future".to_string(),
                )
                .into());
            }
            if age.num_seconds() > policy.max_signature_age as i64 {
                return Err(VerifyError::TrustViolation(format!(
                    "signature is {} seconds old; maximum is {}",
                    age.num_seconds(),
                    policy.max_signature_age
                ))
                .into());
            }
        }
    }
    Ok(())
}

pub fn print_result(result: &VerifiedCcsArchive) {
    println!(
        "[OK] {} v{}",
        result.package_name(),
        result.package_version()
    );
    println!(
        "Signature: [VALID]{}",
        result
            .signature()
            .key_id
            .as_deref()
            .map(|key_id| format!(" key={key_id}"))
            .unwrap_or_default()
    );
    println!("Content: [VALID] {} files verified", result.files_checked());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::builder::write_v2_ccs_package;
    use crate::ccs::signing::SigningKeyPair;

    fn package(signer: &SigningKeyPair) -> (tempfile::TempDir, std::path::PathBuf, TrustPolicy) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("verified.ccs");
        let authority = crate::ccs::v2::test_support::package_authority_with_one_file("verified");
        let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
        write_v2_ccs_package(&authority, &payloads, &path, signer, None, None, None).unwrap();
        let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);
        (temp, path, policy)
    }

    #[test]
    fn verified_value_requires_current_signed_trusted_authority() {
        let signer = SigningKeyPair::generate().with_key_id("release");
        let (_temp, path, policy) = package(&signer);
        let verified = verify_package(&path, &policy).unwrap();

        assert_eq!(verified.package_name(), "verified");
        assert_eq!(verified.files_checked(), 1);
        assert_eq!(verified.signature().key_id.as_deref(), Some("release"));
    }

    #[test]
    fn empty_trust_anchor_set_fails_before_archive_acceptance() {
        let signer = SigningKeyPair::generate();
        let (_temp, path, _) = package(&signer);
        let error = verify_package(&path, &TrustPolicy::strict(Vec::new())).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no trusted CCS package signing keys")
        );
    }

    #[test]
    fn policy_rejects_removed_allow_unsigned_field() {
        let error = TrustPolicy::from_toml(
            r#"
trusted_keys = ["AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="]
allow_unsigned = true
"#,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("unknown field `allow_unsigned`"));
    }

    #[test]
    fn untrusted_signer_is_a_typed_failure() {
        let signer = SigningKeyPair::generate();
        let other = SigningKeyPair::generate();
        let (_temp, path, _) = package(&signer);
        let error = verify_package(&path, &TrustPolicy::strict(vec![other.public_key_base64()]))
            .unwrap_err();
        assert!(error.downcast_ref::<VerifyError>().is_some());
        assert!(format!("{error:#}").contains("not trusted"));
    }
}
