// conary-core/src/ccs/verify.rs

//! Trusted CCS v3 package verification.
//!
//! `archive_reader` remains an explicitly untrusted diagnostic decoder. This
//! module authenticates metadata before streaming signed objects to a spool.

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

pub(crate) mod content;
mod object_sink;
mod stream;

#[derive(Debug, Clone)]
pub(crate) struct RawControlDocuments {
    pub manifest: Vec<u8>,
    pub signature: String,
    pub debug_toml: Option<Vec<u8>>,
    pub build_attestation: Option<String>,
    pub foreign_conversion_boundary: Option<String>,
}

#[derive(Error, Debug)]
pub enum VerifyError {
    #[error("CCS v3 package is not signed")]
    NotSigned,
    #[error("invalid CCS v3 signature format: {0}")]
    InvalidSignatureFormat(String),
    #[error("CCS v3 signature verification failed: {0}")]
    SignatureInvalid(String),
    #[error("CCS v3 package signer is not trusted: {0}")]
    TrustViolation(String),
    #[error("CCS v3 payload authority failed: {0}")]
    PayloadInvalid(String),
    #[error("CCS v3 package structure failed: {0}")]
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
    authority: crate::ccs::v3::AuthorityDocumentV3,
    signature: PackageSignature,
    build_attestation: Option<crate::ccs::attestation::BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<crate::ccs::attestation::ForeignConversionBoundary>,
    debug_toml: Option<Vec<u8>>,
    components: HashMap<String, crate::ccs::builder::ComponentData>,
    payload: crate::packages::payload::PackagePayload,
    object_sources: HashMap<String, crate::packages::payload::ReopenablePayload>,
    verified_object_metrics: Option<crate::filesystem::VerifiedObjectBatchMetrics>,
    files_checked: usize,
    raw: RawControlDocuments,
}

impl VerifiedCcsArchive {
    pub fn authority(&self) -> &crate::ccs::v3::AuthorityDocumentV3 {
        &self.authority
    }

    pub fn signature(&self) -> &PackageSignature {
        &self.signature
    }

    pub fn package_name(&self) -> &str {
        &self.authority.identity.name
    }

    pub fn package_version(&self) -> &str {
        &self.authority.identity.version
    }

    pub fn files_checked(&self) -> usize {
        self.files_checked
    }

    /// Permanent-CAS work performed by install-oriented verification.
    pub fn verified_object_metrics(&self) -> Option<crate::filesystem::VerifiedObjectBatchMetrics> {
        self.verified_object_metrics
    }

    pub(crate) fn object_sources(
        &self,
    ) -> &HashMap<String, crate::packages::payload::ReopenablePayload> {
        &self.object_sources
    }

    pub(crate) fn raw_control_documents(&self) -> &RawControlDocuments {
        &self.raw
    }

    pub(crate) fn build_attestation(
        &self,
    ) -> Option<&crate::ccs::attestation::BuildAttestationEnvelope> {
        self.build_attestation.as_ref()
    }

    pub(crate) fn foreign_conversion_boundary(
        &self,
    ) -> Option<&crate::ccs::attestation::ForeignConversionBoundary> {
        self.foreign_conversion_boundary.as_ref()
    }

    pub(crate) fn debug_toml(&self) -> Option<&[u8]> {
        self.debug_toml.as_deref()
    }

    pub(crate) fn components(&self) -> &HashMap<String, crate::ccs::builder::ComponentData> {
        &self.components
    }

    pub(crate) fn payload(&self) -> &crate::packages::payload::PackagePayload {
        &self.payload
    }
}

/// Verify current CCS authority, signature trust, diagnostic projections, and
/// every payload object before returning an install/publication capability.
pub fn verify_package(path: &Path, policy: &TrustPolicy) -> Result<VerifiedCcsArchive> {
    verify_package_to(path, policy, object_sink::ObjectDestination::Spool)
}

/// Verify a current CCS archive directly into one permanent SHA-256 CAS.
///
/// The returned payload sources carry the committed batch capability, allowing
/// installation to reference their canonical identities without reopening and
/// re-ingesting the same bytes.
pub fn verify_package_into_cas(
    path: &Path,
    policy: &TrustPolicy,
    cas: &crate::filesystem::CasStore,
) -> Result<VerifiedCcsArchive> {
    verify_package_to(path, policy, object_sink::ObjectDestination::Permanent(cas))
}

fn verify_package_to(
    path: &Path,
    policy: &TrustPolicy,
    destination: object_sink::ObjectDestination<'_>,
) -> Result<VerifiedCcsArchive> {
    policy.validate()?;
    let verified = stream::verify_archive(path, policy, destination)
        .with_context(|| format!("verify streaming CCS v3 archive {}", path.display()))?;
    Ok(VerifiedCcsArchive {
        authority: verified.authority,
        signature: verified.signature,
        build_attestation: verified.build_attestation,
        foreign_conversion_boundary: verified.foreign_conversion_boundary,
        debug_toml: verified.debug_toml,
        components: verified.components,
        payload: verified.payload,
        object_sources: verified.object_sources,
        verified_object_metrics: verified.verified_object_metrics,
        files_checked: verified.files_checked,
        raw: verified.raw,
    })
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
    use crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests;
    use crate::ccs::signing::SigningKeyPair;
    use std::io::Read;

    fn package(signer: &SigningKeyPair) -> (tempfile::TempDir, std::path::PathBuf, TrustPolicy) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("verified.ccs");
        let authority = crate::ccs::v3::test_support::package_authority_with_one_file("verified");
        let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
        write_v3_ccs_package_from_bounded_memory_for_tests(
            &authority, &payloads, &path, signer, None, None, None,
        )
        .unwrap();
        let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);
        (temp, path, policy)
    }

    fn chunked_package(
        signer: &SigningKeyPair,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        TrustPolicy,
        Vec<u8>,
        usize,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("chunked.ccs");
        let bytes = (0..(crate::ccs::chunking::MAX_CHUNK_SIZE as usize * 4))
            .map(|index| ((index * 19 + index / 97) % 256) as u8)
            .collect::<Vec<_>>();
        let chunks = crate::ccs::chunking::Chunker::new()
            .chunk_bytes(&bytes)
            .iter()
            .map(crate::ccs::chunking::Chunk::reference)
            .collect::<Vec<_>>();
        let unique = chunks
            .iter()
            .map(|chunk| chunk.sha256.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let mut authority =
            crate::ccs::v3::test_support::package_authority_with_one_file("chunked");
        let crate::ccs::v3::PackageKindV3::Package(package) = &mut authority.kind else {
            unreachable!()
        };
        let file = &mut package.files[0];
        file.content = Some(crate::payload::PayloadContentAuthority {
            sha256: crate::hash::sha256(&bytes),
            size: bytes.len() as u64,
        });
        file.content_layout = crate::ccs::v3::FileContentLayoutV3::FastCdcV2020 {
            min_size: crate::ccs::chunking::MIN_CHUNK_SIZE,
            average_size: crate::ccs::chunking::AVG_CHUNK_SIZE,
            max_size: crate::ccs::chunking::MAX_CHUNK_SIZE,
            chunks,
        };
        authority.components.get_mut("main").unwrap().total_size = bytes.len() as u64;
        let payloads = std::collections::BTreeMap::from([(file.path.clone(), bytes.clone())]);
        write_v3_ccs_package_from_bounded_memory_for_tests(
            &authority, &payloads, &path, signer, None, None, None,
        )
        .unwrap();
        let policy = TrustPolicy::strict(vec![signer.public_key_base64()]);
        (temp, path, policy, bytes, unique)
    }

    #[test]
    fn verified_value_requires_current_signed_trusted_authority() {
        let signer = SigningKeyPair::generate().with_key_id("release");
        let (_temp, path, policy) = package(&signer);
        let verified = verify_package(&path, &policy).unwrap();

        assert_eq!(verified.package_name(), "verified");
        assert_eq!(verified.files_checked(), 1);
        assert_eq!(verified.signature().key_id.as_deref(), Some("release"));
        assert_eq!(verified.verified_object_metrics(), None);
    }

    #[test]
    fn permanent_verification_writes_cold_objects_once_and_warm_objects_zero_times() {
        let signer = SigningKeyPair::generate().with_key_id("release");
        let (temp, path, policy) = package(&signer);
        let cas = crate::filesystem::CasStore::new(temp.path().join("permanent-objects")).unwrap();

        let cold = verify_package_into_cas(&path, &policy, &cas).unwrap();
        let cold_metrics = cold.verified_object_metrics().unwrap();
        assert_eq!(cold_metrics.misses, 1);
        assert_eq!(cold_metrics.hits, 0);
        assert_eq!(
            cold_metrics.persistent_bytes_written,
            cold_metrics.incoming_bytes_hashed
        );
        assert!(cold_metrics.persistent_bytes_written > 0);
        let cold_file = &cold.payload().files()[0];
        let content = cold_file.content_authority.as_ref().unwrap();
        assert_eq!(
            cold_file
                .source()
                .unwrap()
                .verified_cas_identity_for(&cas, content)
                .unwrap()
                .as_deref(),
            Some(content.sha256.as_str())
        );

        let warm = verify_package_into_cas(&path, &policy, &cas).unwrap();
        let warm_metrics = warm.verified_object_metrics().unwrap();
        assert_eq!(warm_metrics.hits, 1);
        assert_eq!(warm_metrics.misses, 0);
        assert_eq!(warm_metrics.persistent_bytes_written, 0);
        assert_eq!(warm_metrics.canonical_bytes_reread, 0);
        assert!(warm_metrics.incoming_bytes_hashed > 0);
    }

    #[test]
    fn permanent_chunk_verification_reuses_objects_and_defers_whole_file_cas_to_installer() {
        let signer = SigningKeyPair::generate().with_key_id("release");
        let (temp, path, policy, bytes, unique) = chunked_package(&signer);
        let cas = crate::filesystem::CasStore::new(temp.path().join("permanent-objects")).unwrap();

        let cold = verify_package_into_cas(&path, &policy, &cas).unwrap();
        let cold_metrics = cold.verified_object_metrics().unwrap();
        assert_eq!(cold_metrics.misses, unique as u64);
        assert_eq!(cold_metrics.hits, 0);
        let file = &cold.payload().files()[0];
        assert_eq!(
            file.source()
                .unwrap()
                .verified_cas_identity_for(&cas, file.content_authority.as_ref().unwrap())
                .unwrap(),
            None,
            "a chunk set is not the whole-file CAS identity generation needs"
        );
        let mut reconstructed = Vec::new();
        file.open_content()
            .unwrap()
            .read_to_end(&mut reconstructed)
            .unwrap();
        assert_eq!(reconstructed, bytes);

        let warm = verify_package_into_cas(&path, &policy, &cas).unwrap();
        let warm_metrics = warm.verified_object_metrics().unwrap();
        assert_eq!(warm_metrics.hits, unique as u64);
        assert_eq!(warm_metrics.misses, 0);
        assert_eq!(warm_metrics.persistent_bytes_written, 0);
        assert_eq!(warm_metrics.canonical_bytes_reread, 0);
    }

    #[test]
    fn permanent_payload_capability_fails_closed_for_another_cas() {
        let signer = SigningKeyPair::generate();
        let (temp, path, policy) = package(&signer);
        let cas = crate::filesystem::CasStore::new(temp.path().join("objects")).unwrap();
        let other = crate::filesystem::CasStore::new(temp.path().join("other-objects")).unwrap();
        let verified = verify_package_into_cas(&path, &policy, &cas).unwrap();
        let file = &verified.payload().files()[0];
        let error = file
            .source()
            .unwrap()
            .verified_cas_identity_for(&other, file.content_authority.as_ref().unwrap())
            .unwrap_err();
        assert!(error.to_string().contains("destination store"));
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
