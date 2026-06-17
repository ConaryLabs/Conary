// conary-core/src/repository/static_repo/publish_gate.rs
//! Static artifact-form publish eligibility and signer authority checks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::ccs::archive_reader::read_ccs_archive;
use crate::ccs::attestation::{
    BuildAttestationEnvelope, BuildOutputIdentity, canonical_json_hash,
    compute_build_output_identity,
};
use crate::ccs::manifest_provenance::ManifestProvenance;
use crate::ccs::package::CcsPackage;
use crate::ccs::verify::{TrustPolicy, VerificationResult, verify_package};
use crate::packages::traits::PackageFormat;
use crate::recipe::hermetic::PolicyStatus;
use crate::repository::static_repo::{PackageKeyStatus, PackageKeysFile};
use crate::security::command_risk::{
    COMMAND_RISK_CLASSIFIER_VERSION, CommandRiskReport, CommandRiskStatus,
};

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
    let verification = verify_package_for_static_gate(artifact_path, accepted_signers)?;
    let artifact_path_str = artifact_path
        .to_str()
        .context("artifact path must be valid UTF-8 for CCS parsing")?;
    let has_v2_authority = read_ccs_archive(std::fs::File::open(artifact_path)?)
        .map(|contents| contents.v2_authority.is_some())
        .map_err(anyhow::Error::from)?;
    let package = if has_v2_authority {
        CcsPackage::parse_verified_v2(artifact_path_str, &verification)
            .map_err(anyhow::Error::from)?
    } else {
        CcsPackage::parse(artifact_path_str).map_err(anyhow::Error::from)?
    };
    verify_verified_static_artifact_publish_eligibility(
        &package,
        &verification,
        accepted_signers,
        accepted_policy_digest,
    )
}

fn verify_package_for_static_gate(
    artifact_path: &Path,
    accepted_signers: &AcceptedStaticSignerSet,
) -> Result<VerificationResult> {
    match verify_package(
        artifact_path,
        &TrustPolicy::strict(accepted_signers.trusted_public_keys()),
    ) {
        Ok(verification) => Ok(verification),
        Err(strict_error) => {
            let mut verification = verify_package(
                artifact_path,
                &TrustPolicy {
                    trusted_keys: accepted_signers.trusted_public_keys(),
                    allow_unsigned: true,
                    require_timestamp: false,
                    max_signature_age: 0,
                },
            )
            .with_context(|| {
                format!("strict package signature verification failed: {strict_error}")
            })?;
            verification.valid = false;
            verification.warnings.push(format!(
                "strict package signature verification failed: {strict_error}"
            ));
            Ok(verification)
        }
    }
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
    verify_command_risk_evidence(package, provenance, envelope, &mut failures)?;
    verify_foreign_boundary_evidence(provenance, envelope, &actual_identity, &mut failures)?;
    if failures.is_empty() {
        Ok(PublishLintReport::passed())
    } else {
        Ok(PublishLintReport::failed(failures))
    }
}

fn verify_command_risk_evidence(
    package: &CcsPackage,
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
        if package.v2_authority().is_some() {
            verify_v2_attested_command_risk_evidence(envelope, failures);
            return Ok(());
        }
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

fn verify_v2_attested_command_risk_evidence(
    envelope: &BuildAttestationEnvelope,
    failures: &mut Vec<PublishGateFailure>,
) {
    if envelope.payload.hermetic_evidence_hash
        != envelope.payload.output_identity.hermetic_evidence_hash
        || envelope.payload.hermetic_evidence_hash.trim().is_empty()
    {
        failures.push(failure(
            PublishGateFailureCode::UncleanCommandRiskReport,
            "v2 attestation hermetic evidence hash does not match output identity",
        ));
    }
    if envelope
        .payload
        .build_command_risk_report_hash
        .trim()
        .is_empty()
    {
        failures.push(failure(
            PublishGateFailureCode::UncleanCommandRiskReport,
            "v2 attestation is missing command-risk report hash",
        ));
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
    verify_foreign_boundary_risk_report(
        "foreign conversion build-body",
        boundary.build_risk_report.as_ref(),
        boundary.build_risk_report_hash.as_deref(),
        true,
        failures,
    )?;
    verify_foreign_boundary_risk_report(
        "foreign conversion scriptlet",
        boundary.scriptlet_risk_report.as_ref(),
        boundary.scriptlet_risk_report_hash.as_deref(),
        false,
        failures,
    )?;
    Ok(())
}

fn verify_foreign_boundary_risk_report(
    label: &str,
    report: Option<&CommandRiskReport>,
    expected_hash: Option<&str>,
    required: bool,
    failures: &mut Vec<PublishGateFailure>,
) -> Result<()> {
    match (report, expected_hash) {
        (None, None) if !required => return Ok(()),
        (None, None) => {
            failures.push(failure(
                PublishGateFailureCode::UncleanCommandRiskReport,
                &format!("{label} risk report is missing"),
            ));
            return Ok(());
        }
        (None, Some(_)) => {
            failures.push(failure(
                PublishGateFailureCode::UncleanCommandRiskReport,
                &format!("{label} risk report hash is present without embedded report"),
            ));
            return Ok(());
        }
        (Some(_), None) => {
            failures.push(failure(
                PublishGateFailureCode::UncleanCommandRiskReport,
                &format!("{label} risk report is present without hash"),
            ));
            return Ok(());
        }
        (Some(report), Some(expected_hash)) => {
            if report.classifier_version != COMMAND_RISK_CLASSIFIER_VERSION {
                failures.push(failure(
                    PublishGateFailureCode::StaleOrUnknownPolicy,
                    &format!("{label} command-risk classifier version is not accepted"),
                ));
            }
            let actual_hash = canonical_json_hash(report)?;
            if actual_hash != expected_hash {
                failures.push(failure(
                    PublishGateFailureCode::UncleanCommandRiskReport,
                    &format!("{label} risk report hash mismatch"),
                ));
            }
            if report.status != CommandRiskStatus::Clean {
                failures.push(failure(
                    PublishGateFailureCode::UncleanCommandRiskReport,
                    &format!("{label} risk report is not clean"),
                ));
            }
        }
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
mod tests {
    use super::*;
    use crate::ccs::attestation::{
        BUILD_ATTESTATION_SCHEMA_V1, BuildAttestationPayload,
        FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1, ForeignConversionBoundary, canonical_json_hash,
        compute_build_output_identity, sign_build_attestation,
    };
    use crate::ccs::builder::{BuildResult, write_ccs_package, write_signed_ccs_package};
    use crate::ccs::signing::SigningKeyPair;
    use crate::recipe::hermetic::PolicyStatus;
    use crate::repository::static_repo::publish_context::STATIC_PUBLISH_POLICY_DIGEST_V1;
    use crate::repository::static_repo::{PackageKeyEntry, PackageKeyStatus, PackageKeysFile};
    use crate::security::command_risk::{
        COMMAND_RISK_CLASSIFIER_VERSION, CommandRiskEntry, PACKAGE_MANAGER_FETCH,
    };
    use tempfile::TempDir;

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

    #[test]
    fn accepted_signer_set_canonical_hash_is_stable() {
        let set = AcceptedStaticSignerSet::from_trusted_artifact_signers(&[
            TrustedArtifactSigner {
                key_id: "b".to_string(),
                public_key: "pub-b".to_string(),
            },
            TrustedArtifactSigner {
                key_id: "a".to_string(),
                public_key: "pub-a".to_string(),
            },
        ])
        .unwrap();

        let first = set.canonical_hash().unwrap();
        let second = set.canonical_hash().unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("sha256:"));
    }

    #[test]
    fn artifact_gate_accepts_attested_hermetic_package() {
        let signer = SigningKeyPair::generate().with_key_id("publish");
        let (_temp, package_path) = attested_artifact_for_tests(&signer, &signer, |_| {}, |_| {});
        let report = verify_static_artifact_publish_eligibility(
            &package_path,
            &accepted_signers_for_key(&signer),
            STATIC_PUBLISH_POLICY_DIGEST_V1,
        )
        .unwrap();

        assert!(report.is_passed(), "{report:?}");
    }

    #[test]
    fn artifact_gate_accepts_attested_v2_package() {
        let signer = SigningKeyPair::generate().with_key_id("publish");
        let temp = tempfile::tempdir().unwrap();
        let package_path = temp.path().join("attested-v2.ccs");
        let authority =
            crate::ccs::v2::test_support::package_authority_with_one_file("attested-v2");
        let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
        let envelope = crate::ccs::attestation::test_support::sample_v2_envelope_for_tests(
            &authority,
            &signer,
            STATIC_PUBLISH_POLICY_DIGEST_V1,
        );
        crate::ccs::builder::write_v2_ccs_package(
            &authority,
            &payloads,
            &package_path,
            &signer,
            None,
            Some(&envelope),
            None,
        )
        .unwrap();

        let report = verify_static_artifact_publish_eligibility(
            &package_path,
            &accepted_signers_for_key(&signer),
            STATIC_PUBLISH_POLICY_DIGEST_V1,
        )
        .unwrap();

        assert!(report.is_passed(), "{report:?}");
    }

    #[test]
    fn m4a_preserves_active_publish_gate_failure_codes() {
        let active = active_failure_codes_for_tests();
        for expected in [
            PublishGateFailureCode::MissingAttestation,
            PublishGateFailureCode::BuildAttestationSignatureMismatch,
            PublishGateFailureCode::PackageSignatureMismatch,
            PublishGateFailureCode::TomlIntegrityMismatch,
            PublishGateFailureCode::OutputIdentityMismatch,
            PublishGateFailureCode::UnacceptedSignerKey,
            PublishGateFailureCode::NonHermeticHardeningLevel,
            PublishGateFailureCode::StaleOrUnknownPolicy,
            PublishGateFailureCode::UncleanCommandRiskReport,
            PublishGateFailureCode::ForeignConversionMissingBoundary,
            PublishGateFailureCode::ForeignConversionBoundaryHashMismatch,
            PublishGateFailureCode::RecordedDraftArtifact,
        ] {
            assert!(
                active.contains(&expected),
                "missing active publish gate code {expected:?}"
            );
        }
    }

    #[test]
    fn m4a_preserves_reserved_publish_gate_mappings() {
        let reserved = [
            PublishGateFailureCode::RetiredSignerKey,
            PublishGateFailureCode::AbsentOrUnknownProvenanceClass,
        ];
        assert_eq!(reserved.len(), 2);
    }

    #[test]
    fn artifact_gate_reports_release_policy_failures() {
        type ArtifactGateCase = (
            &'static str,
            Box<dyn FnOnce() -> (TempDir, std::path::PathBuf, String)>,
        );

        let cases: Vec<ArtifactGateCase> = vec![
            (
                "artifact is missing a build attestation",
                Box::new(|| {
                    let signer = SigningKeyPair::generate().with_key_id("publish");
                    let (temp, package_path) = artifact_without_attestation_for_tests(&signer);
                    let text = failure_text_for_artifact(
                        &package_path,
                        &accepted_signers_for_key(&signer),
                    );
                    (temp, package_path, text)
                }),
            ),
            (
                "build attestation signer is not accepted",
                Box::new(|| {
                    let signer = SigningKeyPair::generate().with_key_id("publish");
                    let other = SigningKeyPair::generate().with_key_id("other");
                    let (temp, package_path) =
                        attested_artifact_for_tests(&signer, &signer, |_| {}, |_| {});
                    let text =
                        failure_text_for_artifact(&package_path, &accepted_signers_for_key(&other));
                    (temp, package_path, text)
                }),
            ),
            (
                "build attestation policy digest is not accepted",
                Box::new(|| {
                    let signer = SigningKeyPair::generate().with_key_id("publish");
                    let (temp, package_path) = attested_artifact_for_tests(
                        &signer,
                        &signer,
                        |_| {},
                        |payload| {
                            payload.publish_policy_digest = "m1-preview-policy".to_string();
                        },
                    );
                    let text = failure_text_for_artifact(
                        &package_path,
                        &accepted_signers_for_key(&signer),
                    );
                    (temp, package_path, text)
                }),
            ),
            (
                "recorded-draft artifacts are not publishable",
                Box::new(|| {
                    let signer = SigningKeyPair::generate().with_key_id("publish");
                    let (temp, package_path) = attested_artifact_for_tests(
                        &signer,
                        &signer,
                        |_| {},
                        |payload| {
                            payload.origin_class = "recorded-draft".to_string();
                        },
                    );
                    let text = failure_text_for_artifact(
                        &package_path,
                        &accepted_signers_for_key(&signer),
                    );
                    (temp, package_path, text)
                }),
            ),
            (
                "build command-risk report is not clean",
                Box::new(|| {
                    let signer = SigningKeyPair::generate().with_key_id("publish");
                    let (temp, package_path) = attested_artifact_for_tests(
                        &signer,
                        &signer,
                        |result| {
                            result
                                .manifest
                                .provenance
                                .as_mut()
                                .unwrap()
                                .hermetic_evidence
                                .as_mut()
                                .unwrap()
                                .command_risk
                                .status = PolicyStatus::Blocked;
                        },
                        |_| {},
                    );
                    let text = failure_text_for_artifact(
                        &package_path,
                        &accepted_signers_for_key(&signer),
                    );
                    (temp, package_path, text)
                }),
            ),
        ];

        for (expected, build_case) in cases {
            let (_temp, _package_path, text) = build_case();
            assert!(
                text.contains(expected),
                "expected {expected:?} in gate failure text:\n{text}"
            );
        }
    }

    #[test]
    fn foreign_converted_publish_requires_manifest_boundary() {
        let signer = SigningKeyPair::generate().with_key_id("publish");
        let (_temp, package_path) =
            foreign_attested_artifact_for_tests(&signer, false, |_| {}, |_| {});

        let report = verify_static_artifact_publish_eligibility(
            &package_path,
            &accepted_signers_for_key(&signer),
            STATIC_PUBLISH_POLICY_DIGEST_V1,
        )
        .unwrap();

        assert!(report.failures.iter().any(|failure| {
            failure.code == PublishGateFailureCode::ForeignConversionMissingBoundary
        }));
    }

    #[test]
    fn foreign_converted_publish_rejects_boundary_hash_mismatch() {
        let signer = SigningKeyPair::generate().with_key_id("publish");
        let (_temp, package_path) = foreign_attested_artifact_for_tests(
            &signer,
            true,
            |boundary| {
                boundary.source_checksum = "sha256:mutated-after-signing".to_string();
            },
            |_| {},
        );

        let report = verify_static_artifact_publish_eligibility(
            &package_path,
            &accepted_signers_for_key(&signer),
            STATIC_PUBLISH_POLICY_DIGEST_V1,
        )
        .unwrap();

        assert!(report.failures.iter().any(|failure| {
            failure.code == PublishGateFailureCode::ForeignConversionBoundaryHashMismatch
        }));
    }

    #[test]
    fn foreign_converted_publish_rejects_boundary_output_identity_mismatch() {
        let signer = SigningKeyPair::generate().with_key_id("publish");
        let (_temp, package_path) = foreign_attested_artifact_for_tests(
            &signer,
            true,
            |boundary| {
                boundary.output_identity.package_name = "other".to_string();
            },
            |payload| {
                payload.conversion_boundary_hash = None;
            },
        );

        let report = verify_static_artifact_publish_eligibility(
            &package_path,
            &accepted_signers_for_key(&signer),
            STATIC_PUBLISH_POLICY_DIGEST_V1,
        )
        .unwrap();

        assert!(
            report
                .failures
                .iter()
                .any(|failure| { failure.message.contains("boundary output identity") })
        );
    }

    #[test]
    fn foreign_converted_publish_rejects_unclean_boundary_scriptlet_report() {
        let signer = SigningKeyPair::generate().with_key_id("publish");
        let (_temp, package_path) = foreign_attested_artifact_with_signed_boundary_for_tests(
            &signer,
            |boundary| {
                let report = crate::security::command_risk::CommandRiskReport {
                    status: CommandRiskStatus::Blocked,
                    classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
                    entries: vec![CommandRiskEntry {
                        source: "foreign-scriptlet:post-install".to_string(),
                        command: "package-manager".to_string(),
                        reason_code: PACKAGE_MANAGER_FETCH.to_string(),
                        severity: CommandRiskStatus::Blocked,
                        evidence: "npm install synthetic-atomic-lockfile".to_string(),
                    }],
                };
                boundary.scriptlet_risk_report_hash = Some(canonical_json_hash(&report).unwrap());
                boundary.scriptlet_risk_report = Some(report);
            },
            |_| {},
        );

        let report = verify_static_artifact_publish_eligibility(
            &package_path,
            &accepted_signers_for_key(&signer),
            STATIC_PUBLISH_POLICY_DIGEST_V1,
        )
        .unwrap();

        assert!(report.failures.iter().any(|failure| {
            failure.code == PublishGateFailureCode::UncleanCommandRiskReport
                && failure.message.contains("scriptlet")
        }));
    }

    fn accepted_signers_for_key(key: &SigningKeyPair) -> AcceptedStaticSignerSet {
        AcceptedStaticSignerSet::from_initial_key(
            key.key_id().unwrap_or("publish"),
            key.public_key_base64(),
        )
    }

    fn active_failure_codes_for_tests() -> Vec<PublishGateFailureCode> {
        vec![
            PublishGateFailureCode::MissingAttestation,
            PublishGateFailureCode::BuildAttestationSignatureMismatch,
            PublishGateFailureCode::PackageSignatureMismatch,
            PublishGateFailureCode::TomlIntegrityMismatch,
            PublishGateFailureCode::OutputIdentityMismatch,
            PublishGateFailureCode::UnacceptedSignerKey,
            PublishGateFailureCode::NonHermeticHardeningLevel,
            PublishGateFailureCode::StaleOrUnknownPolicy,
            PublishGateFailureCode::UncleanCommandRiskReport,
            PublishGateFailureCode::ForeignConversionMissingBoundary,
            PublishGateFailureCode::ForeignConversionBoundaryHashMismatch,
            PublishGateFailureCode::RecordedDraftArtifact,
        ]
    }

    fn failure_text_for_artifact(
        package_path: &std::path::Path,
        accepted_signers: &AcceptedStaticSignerSet,
    ) -> String {
        let report = verify_static_artifact_publish_eligibility(
            package_path,
            accepted_signers,
            STATIC_PUBLISH_POLICY_DIGEST_V1,
        )
        .unwrap();
        assert!(!report.is_passed(), "{report:?}");
        format_publish_gate_failures(&report)
    }

    fn artifact_without_attestation_for_tests(
        signer: &SigningKeyPair,
    ) -> (TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let package_path = temp.path().join("missing-attestation.ccs");
        let result = crate::ccs::builder::test_support::minimal_build_result("widget", "1.0.0");
        write_signed_ccs_package(&result, &package_path, signer).unwrap();
        (temp, package_path)
    }

    fn attested_artifact_for_tests(
        attestation_key: &SigningKeyPair,
        package_key: &SigningKeyPair,
        mutate_result: impl FnOnce(&mut BuildResult),
        mutate_payload: impl FnOnce(&mut BuildAttestationPayload),
    ) -> (TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let identity_path = temp.path().join("identity.ccs");
        let package_path = temp.path().join("artifact.ccs");
        let mut result = crate::ccs::builder::test_support::minimal_build_result("widget", "1.0.0");
        mutate_result(&mut result);
        write_ccs_package(&result, &identity_path).unwrap();
        let identity_package =
            crate::ccs::CcsPackage::parse(identity_path.to_str().unwrap()).unwrap();
        let output_identity = compute_build_output_identity(&identity_package).unwrap();
        let evidence = result
            .manifest
            .provenance
            .as_ref()
            .unwrap()
            .hermetic_evidence
            .as_ref()
            .unwrap()
            .clone();
        let mut payload = BuildAttestationPayload {
            schema_version: BUILD_ATTESTATION_SCHEMA_V1,
            origin_class: output_identity.origin_class.clone(),
            hardening_level: output_identity.hardening_level.clone(),
            build_input: evidence.build_input.clone(),
            dependency_lock: evidence.dependency_lock.clone(),
            hermetic_evidence_hash: canonical_json_hash(&evidence).unwrap(),
            output_identity,
            build_command_risk_report_hash: canonical_json_hash(&evidence.command_risk).unwrap(),
            scriptlet_risk_report_hash: None,
            conversion_boundary_hash: None,
            publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
            command_risk_classifier_version: evidence.command_risk.classifier_version.clone(),
            sandbox_profile: "kitchen-pristine-network-none".to_string(),
            seccomp_profile: Some("scriptlet-v1".to_string()),
            builder_identity: "conary-test-builder".to_string(),
            conary_version: "test".to_string(),
            issued_at: "2026-06-14T00:00:00Z".to_string(),
        };
        mutate_payload(&mut payload);
        result
            .manifest
            .provenance
            .as_mut()
            .unwrap()
            .build_attestation = Some(sign_build_attestation(payload, attestation_key).unwrap());
        write_signed_ccs_package(&result, &package_path, package_key).unwrap();
        (temp, package_path)
    }

    fn foreign_attested_artifact_for_tests(
        signer: &SigningKeyPair,
        include_manifest_boundary: bool,
        mutate_boundary_after_hash: impl FnOnce(&mut ForeignConversionBoundary),
        mutate_payload: impl FnOnce(&mut BuildAttestationPayload),
    ) -> (TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let identity_path = temp.path().join("foreign-identity.ccs");
        let package_path = temp.path().join("foreign.ccs");
        let mut result =
            crate::ccs::builder::test_support::minimal_build_result("foreign", "1.0.0");
        result.manifest.provenance.as_mut().unwrap().origin_class =
            Some("foreign-converted".to_string());
        write_ccs_package(&result, &identity_path).unwrap();
        let identity_package =
            crate::ccs::CcsPackage::parse(identity_path.to_str().unwrap()).unwrap();
        let output_identity = compute_build_output_identity(&identity_package).unwrap();
        let evidence = result
            .manifest
            .provenance
            .as_ref()
            .unwrap()
            .hermetic_evidence
            .as_ref()
            .unwrap()
            .clone();
        let build_risk_report = crate::security::command_risk::CommandRiskReport::clean();
        let mut boundary = ForeignConversionBoundary {
            schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            source_format: "rpm".to_string(),
            source_checksum: "sha256:source".to_string(),
            output_identity: output_identity.clone(),
            build_risk_report_hash: Some(canonical_json_hash(&build_risk_report).unwrap()),
            build_risk_report: Some(build_risk_report),
            scriptlet_risk_report_hash: None,
            scriptlet_risk_report: None,
            diagnostics: Vec::new(),
        };
        let signed_boundary_hash = canonical_json_hash(&boundary).unwrap();
        mutate_boundary_after_hash(&mut boundary);
        if include_manifest_boundary {
            result
                .manifest
                .provenance
                .as_mut()
                .unwrap()
                .foreign_conversion_boundary = Some(boundary);
        }
        let mut payload = BuildAttestationPayload {
            schema_version: BUILD_ATTESTATION_SCHEMA_V1,
            origin_class: output_identity.origin_class.clone(),
            hardening_level: output_identity.hardening_level.clone(),
            build_input: evidence.build_input.clone(),
            dependency_lock: evidence.dependency_lock.clone(),
            hermetic_evidence_hash: canonical_json_hash(&evidence).unwrap(),
            output_identity,
            build_command_risk_report_hash: canonical_json_hash(&evidence.command_risk).unwrap(),
            scriptlet_risk_report_hash: None,
            conversion_boundary_hash: Some(signed_boundary_hash),
            publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
            command_risk_classifier_version: evidence.command_risk.classifier_version.clone(),
            sandbox_profile: "foreign-conversion-no-exec".to_string(),
            seccomp_profile: None,
            builder_identity: "conary-foreign-converter".to_string(),
            conary_version: "test".to_string(),
            issued_at: "2026-06-14T00:00:00Z".to_string(),
        };
        mutate_payload(&mut payload);
        result
            .manifest
            .provenance
            .as_mut()
            .unwrap()
            .build_attestation = Some(sign_build_attestation(payload, signer).unwrap());
        write_signed_ccs_package(&result, &package_path, signer).unwrap();
        (temp, package_path)
    }

    fn foreign_attested_artifact_with_signed_boundary_for_tests(
        signer: &SigningKeyPair,
        mutate_boundary_before_hash: impl FnOnce(&mut ForeignConversionBoundary),
        mutate_payload: impl FnOnce(&mut BuildAttestationPayload),
    ) -> (TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let identity_path = temp.path().join("foreign-identity.ccs");
        let package_path = temp.path().join("foreign.ccs");
        let mut result =
            crate::ccs::builder::test_support::minimal_build_result("foreign", "1.0.0");
        result.manifest.provenance.as_mut().unwrap().origin_class =
            Some("foreign-converted".to_string());
        write_ccs_package(&result, &identity_path).unwrap();
        let identity_package =
            crate::ccs::CcsPackage::parse(identity_path.to_str().unwrap()).unwrap();
        let output_identity = compute_build_output_identity(&identity_package).unwrap();
        let evidence = result
            .manifest
            .provenance
            .as_ref()
            .unwrap()
            .hermetic_evidence
            .as_ref()
            .unwrap()
            .clone();
        let build_risk_report = crate::security::command_risk::CommandRiskReport::clean();
        let mut boundary = ForeignConversionBoundary {
            schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            source_format: "rpm".to_string(),
            source_checksum: "sha256:source".to_string(),
            output_identity: output_identity.clone(),
            build_risk_report_hash: Some(canonical_json_hash(&build_risk_report).unwrap()),
            build_risk_report: Some(build_risk_report),
            scriptlet_risk_report_hash: None,
            scriptlet_risk_report: None,
            diagnostics: Vec::new(),
        };
        mutate_boundary_before_hash(&mut boundary);
        let signed_boundary_hash = canonical_json_hash(&boundary).unwrap();
        result
            .manifest
            .provenance
            .as_mut()
            .unwrap()
            .foreign_conversion_boundary = Some(boundary);
        let mut payload = BuildAttestationPayload {
            schema_version: BUILD_ATTESTATION_SCHEMA_V1,
            origin_class: output_identity.origin_class.clone(),
            hardening_level: output_identity.hardening_level.clone(),
            build_input: evidence.build_input.clone(),
            dependency_lock: evidence.dependency_lock.clone(),
            hermetic_evidence_hash: canonical_json_hash(&evidence).unwrap(),
            output_identity,
            build_command_risk_report_hash: canonical_json_hash(&evidence.command_risk).unwrap(),
            scriptlet_risk_report_hash: None,
            conversion_boundary_hash: Some(signed_boundary_hash),
            publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
            command_risk_classifier_version: evidence.command_risk.classifier_version.clone(),
            sandbox_profile: "foreign-conversion-no-exec".to_string(),
            seccomp_profile: None,
            builder_identity: "conary-foreign-converter".to_string(),
            conary_version: "test".to_string(),
            issued_at: "2026-06-14T00:00:00Z".to_string(),
        };
        mutate_payload(&mut payload);
        result
            .manifest
            .provenance
            .as_mut()
            .unwrap()
            .build_attestation = Some(sign_build_attestation(payload, signer).unwrap());
        write_signed_ccs_package(&result, &package_path, signer).unwrap();
        (temp, package_path)
    }
}
