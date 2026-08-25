// apps/remi/src/server/conversion_crawl/report.rs
//! Strict schema-4 full-universe conversion crawl report.

use super::proof_reuse::{ConversionProofDispositionV1, ConversionProofV1};
use super::{exact_prefixed_sha256, validate_sha256};
use anyhow::{Result, bail, ensure};
use conary_core::corpus::{ConversionFailure, FailureKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const REMI_CONVERSION_CRAWL_SCHEMA_V4: u32 = 4;
pub const CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversionCrawlOutcomeStateV4 {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionCrawlFailureV4 {
    pub kind: FailureKind,
    pub incident_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CcsArtifactReopenProofV1 {
    pub schema_version: u32,
    pub ccs_format_version: u16,
    pub foreign_conversion_boundary_schema_version: u32,
    pub signer_public_key_sha256: String,
    pub transport_sha256: String,
    pub verified_files: u64,
    pub verified_objects: u64,
}

impl CcsArtifactReopenProofV1 {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1,
            "unsupported CCS artifact reopen proof schema {}",
            self.schema_version
        );
        ensure!(
            self.ccs_format_version == conary_core::ccs::v3::FORMAT_VERSION_V3,
            "unsupported reopened CCS format version {}",
            self.ccs_format_version
        );
        ensure!(
            self.foreign_conversion_boundary_schema_version
                == conary_core::ccs::attestation::FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            "unsupported reopened foreign conversion boundary schema {}",
            self.foreign_conversion_boundary_schema_version
        );
        validate_sha256(
            &self.signer_public_key_sha256,
            "reopened CCS signer public key",
        )?;
        validate_sha256(&self.transport_sha256, "reopened CCS transport")
    }
}

impl From<ConversionFailure> for ConversionCrawlFailureV4 {
    fn from(failure: ConversionFailure) -> Self {
        let kind = failure.kind();
        let incident_id = match failure {
            ConversionFailure::InternalUnclassified { incident_id, .. } => Some(incident_id),
            _ => None,
        };
        Self { kind, incident_id }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionCrawlPackageOutcomeV4 {
    pub package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    pub repository_checksum: String,
    pub state: ConversionCrawlOutcomeStateV4,
    pub proof_disposition: Option<ConversionProofDispositionV1>,
    pub conversion_proof: Option<ConversionProofV1>,
    pub failure: Option<ConversionCrawlFailureV4>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionCrawlProfileV4 {
    pub profile: String,
    pub profile_revision_sha256: String,
    pub expected_packages: u64,
    pub outcomes: Vec<ConversionCrawlPackageOutcomeV4>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiConversionCrawlV4 {
    pub schema_version: u32,
    pub profiles: Vec<ConversionCrawlProfileV4>,
}

impl RemiConversionCrawlV4 {
    pub fn validate_structure(&self) -> Result<()> {
        ensure!(
            self.schema_version == REMI_CONVERSION_CRAWL_SCHEMA_V4,
            "unsupported Remi conversion crawl schema {}",
            self.schema_version
        );

        let expected_profiles = conary_core::repository::supported_profiles::public_profiles();
        ensure!(
            self.profiles.len() == expected_profiles.len(),
            "conversion crawl names {} profiles but {} public profiles are required",
            self.profiles.len(),
            expected_profiles.len()
        );
        for (profile, expected) in self.profiles.iter().zip(expected_profiles) {
            ensure!(
                profile.profile == expected.id(),
                "conversion crawl profile order or membership differs from the public profile contract"
            );
            validate_sha256(
                &profile.profile_revision_sha256,
                "conversion crawl profile revision",
            )?;
            ensure!(
                profile.expected_packages == profile.outcomes.len() as u64,
                "conversion crawl profile '{}' expected {} packages but carries {} outcomes",
                profile.profile,
                profile.expected_packages,
                profile.outcomes.len()
            );
            ensure!(
                profile.expected_packages > 0,
                "conversion crawl profile '{}' has an empty package universe",
                profile.profile
            );
            validate_outcomes(profile)?;
        }
        Ok(())
    }

    pub fn validate_complete(&self) -> Result<()> {
        self.validate_structure()?;
        if self.profiles.iter().any(|profile| {
            profile
                .outcomes
                .iter()
                .any(|outcome| outcome.state == ConversionCrawlOutcomeStateV4::Failed)
        }) {
            bail!("conversion crawl contains failed package outcomes");
        }
        Ok(())
    }
}

fn validate_outcomes(profile: &ConversionCrawlProfileV4) -> Result<()> {
    let mut prior_identity: Option<(&str, &str, &str, &Option<String>, &str)> = None;
    let mut keys = BTreeSet::new();
    for outcome in &profile.outcomes {
        validate_sha256(&outcome.package_key_sha256, "conversion crawl package key")?;
        let identity = (
            outcome.name.as_str(),
            outcome.version.as_str(),
            outcome.package_release.as_str(),
            &outcome.architecture,
            outcome.package_key_sha256.as_str(),
        );
        if let Some(prior) = prior_identity {
            ensure!(
                prior < identity,
                "conversion crawl package outcomes are repeated or not canonically ordered"
            );
        }
        prior_identity = Some(identity);
        ensure!(
            keys.insert(outcome.package_key_sha256.as_str()),
            "conversion crawl repeats package key {}",
            outcome.package_key_sha256
        );
        ensure!(
            !outcome.name.is_empty()
                && !outcome.version.is_empty()
                && !outcome.repository_checksum.is_empty(),
            "conversion crawl package identity is incomplete"
        );
        match outcome.state {
            ConversionCrawlOutcomeStateV4::Succeeded => {
                let proof = outcome.conversion_proof.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("successful crawl outcome has no exact conversion proof")
                })?;
                proof.validate_current()?;
                let repository_source = exact_prefixed_sha256(
                    &outcome.repository_checksum,
                    "conversion crawl repository source",
                )?;
                ensure!(
                    outcome.failure.is_none()
                        && proof.key.source_profile == profile.profile
                        && proof.key.package_key_sha256 == outcome.package_key_sha256
                        && proof.key.package_name == outcome.name
                        && proof.key.package_version == outcome.version
                        && proof.key.package_release == outcome.package_release
                        && proof.key.package_architecture == outcome.architecture
                        && proof.key.source_artifact_sha256 == repository_source,
                    "successful crawl outcome differs from exact conversion proof authority"
                );
                match outcome.proof_disposition {
                    Some(ConversionProofDispositionV1::Validated) => ensure!(
                        proof.validated_profile_revision_sha256 == profile.profile_revision_sha256,
                        "validated crawl proof originated from a different profile revision"
                    ),
                    Some(ConversionProofDispositionV1::Reused) => ensure!(
                        proof.validated_profile_revision_sha256 != profile.profile_revision_sha256,
                        "crawl outcome invents reuse for proof validated by this revision"
                    ),
                    None => bail!("successful crawl outcome has no proof disposition"),
                }
            }
            ConversionCrawlOutcomeStateV4::Failed => {
                let failure = outcome.failure.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("failed crawl outcome has no typed failure evidence")
                })?;
                ensure!(
                    (failure.kind == FailureKind::InternalUnclassified)
                        == failure.incident_id.is_some(),
                    "failed crawl outcome has invalid incident identity"
                );
                ensure!(
                    outcome.proof_disposition.is_none() && outcome.conversion_proof.is_none(),
                    "failed crawl outcome carries success evidence"
                );
            }
        }
    }
    Ok(())
}
