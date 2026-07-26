// conary-core/src/repository/static_repo/publish_gate.rs
//! Static artifact-form publish eligibility and signer authority checks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::ccs::attestation::{
    BuildAttestationEnvelope, BuildOutputIdentity, canonical_json_hash,
    compute_build_output_identity,
};
use crate::ccs::manifest_provenance::ManifestProvenance;
use crate::ccs::package::CcsPackage;
use crate::ccs::verify::{TrustPolicy, VerifiedCcsArchive, verify_package};
use crate::repository::static_repo::{PackageKeyStatus, PackageKeysFile};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedStaticSignerSet {
    active_keys: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedArtifactSigner {
    pub key_id: String,
    pub public_key: String,
}

impl AcceptedStaticSignerSet {
    pub fn from_verified_package_keys(keys: &PackageKeysFile) -> Result<Self> {
        let mut active_keys = BTreeMap::new();
        let mut public_keys = BTreeSet::new();
        for key in keys
            .keys
            .iter()
            .filter(|key| matches!(key.status, PackageKeyStatus::Active))
        {
            let key_id = key.key_id.clone().unwrap_or_else(|| key.public_key.clone());
            if active_keys.contains_key(&key_id) {
                bail!("duplicate active package key id {key_id}");
            }
            if !public_keys.insert(key.public_key.clone()) {
                bail!("duplicate active package public key");
            }
            active_keys.insert(key_id, key.public_key.clone());
        }
        if active_keys.is_empty() {
            bail!("no active package keys can authorize new artifact publish");
        }
        Ok(Self { active_keys })
    }

    pub fn from_initial_key(key_id: impl Into<String>, public_key: impl Into<String>) -> Self {
        Self {
            active_keys: BTreeMap::from([(key_id.into(), public_key.into())]),
        }
    }

    pub fn from_trusted_artifact_signers(signers: &[TrustedArtifactSigner]) -> Result<Self> {
        if signers.is_empty() {
            bail!("no trusted release signers configured");
        }
        let mut active_keys = BTreeMap::new();
        let mut public_keys = BTreeSet::new();
        for signer in signers {
            if active_keys.contains_key(&signer.key_id) {
                bail!("duplicate trusted release signer id {}", signer.key_id);
            }
            if !public_keys.insert(signer.public_key.clone()) {
                bail!("duplicate trusted release signer public key");
            }
            active_keys.insert(signer.key_id.clone(), signer.public_key.clone());
        }
        Ok(Self { active_keys })
    }

    pub fn accepts_key_id(&self, key_id: &str) -> bool {
        self.active_keys.contains_key(key_id)
    }

    pub fn public_key_for(&self, key_id: &str) -> Option<&str> {
        self.active_keys.get(key_id).map(String::as_str)
    }

    pub fn trusted_public_keys(&self) -> Vec<String> {
        self.active_keys.values().cloned().collect()
    }

    pub fn canonical_hash(&self) -> Result<String> {
        canonical_json_hash(&self.active_keys)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PublishGateStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishLintReport {
    pub status: PublishGateStatus,
    pub failures: Vec<PublishGateFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishGateFailure {
    pub code: PublishGateFailureCode,
    pub message: String,
}

#[derive(Debug)]
pub struct StaticArtifactPublishCandidate {
    pub package: CcsPackage,
    pub lint: PublishLintReport,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PublishGateFailureCode {
    MissingAttestation,
    BuildAttestationSignatureMismatch,
    PackageSignatureMismatch,
    TomlIntegrityMismatch,
    OutputIdentityMismatch,
    UnacceptedSignerKey,
    RetiredSignerKey,
    AbsentOrUnknownProvenanceClass,
    NonHermeticHardeningLevel,
    StaleOrUnknownPolicy,
    ForeignConversionMissingBoundary,
    ForeignConversionBoundaryHashMismatch,
    RecordedDraftArtifact,
}

impl PublishLintReport {
    pub fn passed() -> Self {
        Self {
            status: PublishGateStatus::Passed,
            failures: Vec::new(),
        }
    }

    pub fn failed(failures: Vec<PublishGateFailure>) -> Self {
        Self {
            status: PublishGateStatus::Failed,
            failures,
        }
    }

    pub fn is_passed(&self) -> bool {
        self.status == PublishGateStatus::Passed
    }
}

pub fn format_publish_gate_failures(report: &PublishLintReport) -> String {
    if report.failures.is_empty() {
        return "static artifact publish gate failed".to_string();
    }
    let failures = report
        .failures
        .iter()
        .map(|failure| format!("{:?}: {}", failure.code, failure.message))
        .collect::<Vec<_>>()
        .join("; ");
    format!("static artifact publish gate failed: {failures}")
}

pub fn verify_static_artifact_publish_eligibility(
    artifact_path: &Path,
    accepted_signers: &AcceptedStaticSignerSet,
    accepted_policy_digest: &str,
) -> Result<PublishLintReport> {
    verify_static_artifact_publish_candidate(
        artifact_path,
        accepted_signers,
        accepted_policy_digest,
    )
    .map(|candidate| candidate.lint)
}

pub fn verify_static_artifact_publish_candidate(
    artifact_path: &Path,
    accepted_signers: &AcceptedStaticSignerSet,
    accepted_policy_digest: &str,
) -> Result<StaticArtifactPublishCandidate> {
    let verification = verify_package_for_static_gate(artifact_path, accepted_signers)?;
    let artifact_path_str = artifact_path
        .to_str()
        .context("artifact path must be valid UTF-8 for CCS parsing")?;
    let package = CcsPackage::from_verified_archive(artifact_path_str, &verification)
        .map_err(anyhow::Error::from)?;
    let lint = verify_verified_static_artifact_publish_eligibility(
        &package,
        &verification,
        accepted_signers,
        accepted_policy_digest,
    )?;
    Ok(StaticArtifactPublishCandidate { package, lint })
}

fn verify_package_for_static_gate(
    artifact_path: &Path,
    accepted_signers: &AcceptedStaticSignerSet,
) -> Result<VerifiedCcsArchive> {
    verify_package(
        artifact_path,
        &TrustPolicy::strict(accepted_signers.trusted_public_keys()),
    )
    .context("verify current CCS authority for static publication")
}

fn verify_verified_static_artifact_publish_eligibility(
    package: &CcsPackage,
    _verification: &VerifiedCcsArchive,
    accepted_signers: &AcceptedStaticSignerSet,
    accepted_policy_digest: &str,
) -> Result<PublishLintReport> {
    let mut failures = Vec::new();
    let Some(provenance) = package.manifest().provenance.as_ref() else {
        failures.push(failure(
            PublishGateFailureCode::MissingAttestation,
            "artifact is missing provenance and build attestation",
        ));
        return Ok(PublishLintReport::failed(failures));
    };
    let Some(envelope) = provenance.build_attestation.as_ref() else {
        failures.push(failure(
            PublishGateFailureCode::MissingAttestation,
            "artifact is missing a build attestation",
        ));
        return Ok(PublishLintReport::failed(failures));
    };
    let mut attestation_report = verify_static_attestation(
        package,
        provenance,
        envelope,
        accepted_signers,
        accepted_policy_digest,
    )?;
    failures.append(&mut attestation_report.failures);
    if failures.is_empty() {
        Ok(PublishLintReport::passed())
    } else {
        Ok(PublishLintReport::failed(failures))
    }
}

fn verify_static_attestation(
    package: &CcsPackage,
    provenance: &ManifestProvenance,
    envelope: &BuildAttestationEnvelope,
    accepted_signers: &AcceptedStaticSignerSet,
    accepted_policy_digest: &str,
) -> Result<PublishLintReport> {
    let mut failures = Vec::new();
    let actual_identity =
        compute_build_output_identity(package).context("compute artifact output identity")?;
    if actual_identity.hardening_level != "hermetic"
        || envelope.payload.hardening_level != "hermetic"
        || envelope.payload.output_identity.hardening_level != "hermetic"
    {
        failures.push(failure(
            PublishGateFailureCode::NonHermeticHardeningLevel,
            "artifact is not hermetic",
        ));
    }
    if envelope.payload.origin_class == "recorded-draft" {
        failures.push(failure(
            PublishGateFailureCode::RecordedDraftArtifact,
            "recorded-draft artifacts are not publishable",
        ));
    }
    if envelope.payload.publish_policy_digest != accepted_policy_digest {
        failures.push(failure(
            PublishGateFailureCode::StaleOrUnknownPolicy,
            "build attestation policy digest is not accepted",
        ));
    }
    let Some(public_key) = accepted_signers.public_key_for(&envelope.signer_key_id) else {
        failures.push(failure(
            PublishGateFailureCode::UnacceptedSignerKey,
            "build attestation signer is not accepted for this static target",
        ));
        return Ok(PublishLintReport::failed(failures));
    };
    if crate::ccs::attestation::verify_build_attestation_envelope(envelope, public_key).is_err() {
        failures.push(failure(
            PublishGateFailureCode::BuildAttestationSignatureMismatch,
            "build attestation signature mismatch",
        ));
    }
    if actual_identity != envelope.payload.output_identity
        || actual_identity.origin_class != envelope.payload.origin_class
        || actual_identity.hardening_level != envelope.payload.hardening_level
        || provenance.origin_class.as_deref() != Some(envelope.payload.origin_class.as_str())
        || provenance.hardening_level.as_deref() != Some(envelope.payload.hardening_level.as_str())
    {
        failures.push(failure(
            PublishGateFailureCode::OutputIdentityMismatch,
            "build attestation identity fields do not match artifact provenance",
        ));
    }
    // Command-risk reports are signed diagnostics. Their presence, classifier
    // output, and internal report hashes must never decide publish eligibility.
    verify_foreign_boundary_evidence(provenance, envelope, &actual_identity, &mut failures)?;
    if failures.is_empty() {
        Ok(PublishLintReport::passed())
    } else {
        Ok(PublishLintReport::failed(failures))
    }
}

fn verify_foreign_boundary_evidence(
    provenance: &ManifestProvenance,
    envelope: &BuildAttestationEnvelope,
    actual_identity: &BuildOutputIdentity,
    failures: &mut Vec<PublishGateFailure>,
) -> Result<()> {
    let is_foreign = envelope.payload.origin_class == "foreign-converted"
        || envelope.payload.output_identity.origin_class == "foreign-converted"
        || provenance.origin_class.as_deref() == Some("foreign-converted");
    if !is_foreign {
        return Ok(());
    }

    let expected_boundary_hash = envelope.payload.conversion_boundary_hash.as_ref();
    if expected_boundary_hash.is_none() {
        failures.push(failure(
            PublishGateFailureCode::ForeignConversionMissingBoundary,
            "foreign-converted artifact is missing a conversion boundary hash",
        ));
    }
    let Some(boundary) = provenance.foreign_conversion_boundary.as_ref() else {
        failures.push(failure(
            PublishGateFailureCode::ForeignConversionMissingBoundary,
            "foreign-converted artifact is missing conversion boundary metadata",
        ));
        return Ok(());
    };
    if let Some(expected_boundary_hash) = expected_boundary_hash {
        let actual_boundary_hash = canonical_json_hash(boundary)?;
        if &actual_boundary_hash != expected_boundary_hash {
            failures.push(failure(
                PublishGateFailureCode::ForeignConversionBoundaryHashMismatch,
                "foreign conversion boundary hash mismatch",
            ));
        }
    }
    if &boundary.output_identity != actual_identity {
        failures.push(failure(
            PublishGateFailureCode::ForeignConversionBoundaryHashMismatch,
            "foreign conversion boundary output identity does not match artifact",
        ));
    }
    Ok(())
}

fn failure(code: PublishGateFailureCode, message: &str) -> PublishGateFailure {
    PublishGateFailure {
        code,
        message: message.to_string(),
    }
}

#[cfg(test)]
#[path = "publish_gate/tests.rs"]
mod tests;
