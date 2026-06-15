// conary-core/src/repository/static_repo/publish_gate.rs
//! Static artifact-form publish eligibility and signer authority checks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::ccs::attestation::{
    BuildAttestationEnvelope, canonical_json_hash, compute_build_output_identity,
};
use crate::ccs::manifest_provenance::ManifestProvenance;
use crate::ccs::package::CcsPackage;
use crate::ccs::verify::{TrustPolicy, VerificationResult, verify_package};
use crate::packages::traits::PackageFormat;
use crate::recipe::hermetic::PolicyStatus;
use crate::repository::static_repo::{PackageKeyStatus, PackageKeysFile};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedStaticSignerSet {
    active_keys: BTreeMap<String, String>,
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

    pub fn accepts_key_id(&self, key_id: &str) -> bool {
        self.active_keys.contains_key(key_id)
    }

    pub fn public_key_for(&self, key_id: &str) -> Option<&str> {
        self.active_keys.get(key_id).map(String::as_str)
    }

    pub fn trusted_public_keys(&self) -> Vec<String> {
        self.active_keys.values().cloned().collect()
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
    UncleanCommandRiskReport,
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
    let verification = verify_package(
        artifact_path,
        &TrustPolicy::strict(accepted_signers.trusted_public_keys()),
    )?;
    let package = CcsPackage::parse(
        artifact_path
            .to_str()
            .context("artifact path must be valid UTF-8 for CCS parsing")?,
    )
    .map_err(anyhow::Error::from)?;
    verify_verified_static_artifact_publish_eligibility(
        &package,
        &verification,
        accepted_signers,
        accepted_policy_digest,
    )
}

fn verify_verified_static_artifact_publish_eligibility(
    package: &CcsPackage,
    verification: &VerificationResult,
    accepted_signers: &AcceptedStaticSignerSet,
    accepted_policy_digest: &str,
) -> Result<PublishLintReport> {
    let mut failures = Vec::new();
    if !verification.valid {
        failures.push(failure(
            PublishGateFailureCode::PackageSignatureMismatch,
            "artifact package signature is missing, invalid, or untrusted",
        ));
    }
    if !verification.toml_integrity_valid {
        failures.push(failure(
            PublishGateFailureCode::TomlIntegrityMismatch,
            "artifact TOML manifest integrity hash does not match binary manifest",
        ));
    }
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
    verify_command_risk_evidence(provenance, envelope, &mut failures)?;
    verify_foreign_boundary_evidence(envelope, &mut failures);
    if failures.is_empty() {
        Ok(PublishLintReport::passed())
    } else {
        Ok(PublishLintReport::failed(failures))
    }
}

fn verify_command_risk_evidence(
    provenance: &ManifestProvenance,
    envelope: &BuildAttestationEnvelope,
    failures: &mut Vec<PublishGateFailure>,
) -> Result<()> {
    if envelope.payload.command_risk_classifier_version
        != crate::security::command_risk::COMMAND_RISK_CLASSIFIER_VERSION
    {
        failures.push(failure(
            PublishGateFailureCode::StaleOrUnknownPolicy,
            "build attestation command-risk classifier version is not accepted",
        ));
    }

    let Some(evidence) = provenance.hermetic_evidence.as_ref() else {
        failures.push(failure(
            PublishGateFailureCode::UncleanCommandRiskReport,
            "artifact is missing hermetic command-risk evidence",
        ));
        return Ok(());
    };

    let actual_hash = canonical_json_hash(&evidence.command_risk)?;
    if actual_hash != envelope.payload.build_command_risk_report_hash {
        failures.push(failure(
            PublishGateFailureCode::UncleanCommandRiskReport,
            "build command-risk report hash does not match attestation",
        ));
    }
    if !matches!(evidence.command_risk.status, PolicyStatus::Clean) {
        failures.push(failure(
            PublishGateFailureCode::UncleanCommandRiskReport,
            "build command-risk report is not clean",
        ));
    }
    Ok(())
}

fn verify_foreign_boundary_evidence(
    envelope: &BuildAttestationEnvelope,
    failures: &mut Vec<PublishGateFailure>,
) {
    if envelope.payload.origin_class == "foreign-converted"
        && envelope.payload.conversion_boundary_hash.is_none()
    {
        failures.push(failure(
            PublishGateFailureCode::ForeignConversionMissingBoundary,
            "foreign-converted artifact is missing a conversion boundary hash",
        ));
    }
}

fn failure(code: PublishGateFailureCode, message: &str) -> PublishGateFailure {
    PublishGateFailure {
        code,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::static_repo::{PackageKeyEntry, PackageKeyStatus, PackageKeysFile};

    fn package_key(id: &str, public_key: &str, status: PackageKeyStatus) -> PackageKeyEntry {
        PackageKeyEntry {
            algorithm: "ed25519".to_string(),
            public_key: public_key.to_string(),
            key_id: Some(id.to_string()),
            status,
            comment: None,
        }
    }

    #[test]
    fn accepted_signers_include_only_active_package_keys() {
        let keys = PackageKeysFile {
            schema: 1,
            keys: vec![
                package_key("active", "pub-active", PackageKeyStatus::Active),
                package_key("retired", "pub-retired", PackageKeyStatus::Retired),
            ],
        };
        let set = AcceptedStaticSignerSet::from_verified_package_keys(&keys).unwrap();

        assert!(set.accepts_key_id("active"));
        assert!(!set.accepts_key_id("retired"));
    }

    #[test]
    fn retired_signer_cannot_authorize_new_publish() {
        let keys = PackageKeysFile {
            schema: 1,
            keys: vec![package_key(
                "retired",
                "pub-retired",
                PackageKeyStatus::Retired,
            )],
        };
        let err = AcceptedStaticSignerSet::from_verified_package_keys(&keys).unwrap_err();

        assert!(err.to_string().contains("no active package keys"));
    }

    #[test]
    fn duplicate_active_signers_fail_closed() {
        let keys = PackageKeysFile {
            schema: 1,
            keys: vec![
                package_key("dup", "pub-one", PackageKeyStatus::Active),
                package_key("dup", "pub-two", PackageKeyStatus::Active),
            ],
        };
        let err = AcceptedStaticSignerSet::from_verified_package_keys(&keys).unwrap_err();

        assert!(err.to_string().contains("duplicate active package key id"));
    }
}
