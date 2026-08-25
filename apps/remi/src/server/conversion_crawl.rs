// apps/remi/src/server/conversion_crawl.rs
//! Strict full-universe conversion-crawl evidence.

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const REMI_CONVERSION_CRAWL_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversionCrawlOutcomeStateV1 {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionCrawlPackageOutcomeV1 {
    pub package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: String,
    pub repository_checksum: String,
    pub state: ConversionCrawlOutcomeStateV1,
    pub source_artifact_sha256: Option<String>,
    pub ccs_sha256: Option<String>,
    pub failure: Option<conary_core::corpus::ConversionFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionCrawlProfileV1 {
    pub profile: String,
    pub profile_revision_sha256: String,
    pub expected_packages: u64,
    pub outcomes: Vec<ConversionCrawlPackageOutcomeV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiConversionCrawlV1 {
    pub schema_version: u32,
    pub profiles: Vec<ConversionCrawlProfileV1>,
}

impl RemiConversionCrawlV1 {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == REMI_CONVERSION_CRAWL_SCHEMA_V1,
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

            let mut prior_key: Option<&str> = None;
            let mut keys = BTreeSet::new();
            for outcome in &profile.outcomes {
                validate_sha256(&outcome.package_key_sha256, "conversion crawl package key")?;
                if let Some(prior) = prior_key {
                    ensure!(
                        prior < outcome.package_key_sha256.as_str(),
                        "conversion crawl package outcomes are repeated or not canonically ordered"
                    );
                }
                prior_key = Some(&outcome.package_key_sha256);
                ensure!(
                    keys.insert(outcome.package_key_sha256.as_str()),
                    "conversion crawl repeats package key {}",
                    outcome.package_key_sha256
                );
                ensure!(
                    !outcome.name.is_empty()
                        && !outcome.version.is_empty()
                        && !outcome.architecture.is_empty()
                        && !outcome.repository_checksum.is_empty(),
                    "conversion crawl package identity is incomplete"
                );
                match outcome.state {
                    ConversionCrawlOutcomeStateV1::Succeeded => {
                        let source =
                            outcome.source_artifact_sha256.as_deref().ok_or_else(|| {
                                anyhow::anyhow!(
                                    "successful crawl outcome has no source artifact digest"
                                )
                            })?;
                        let ccs = outcome.ccs_sha256.as_deref().ok_or_else(|| {
                            anyhow::anyhow!("successful crawl outcome has no CCS digest")
                        })?;
                        validate_sha256(source, "conversion crawl source artifact")?;
                        validate_sha256(ccs, "conversion crawl CCS")?;
                        ensure!(
                            outcome.failure.is_none(),
                            "successful crawl outcome carries failure evidence"
                        );
                    }
                    ConversionCrawlOutcomeStateV1::Failed => {
                        ensure!(
                            outcome.failure.is_some(),
                            "failed crawl outcome has no typed failure evidence"
                        );
                        ensure!(
                            outcome.source_artifact_sha256.is_none()
                                && outcome.ccs_sha256.is_none(),
                            "failed crawl outcome carries success digests"
                        );
                    }
                }
            }
        }

        if self.profiles.iter().any(|profile| {
            profile
                .outcomes
                .iter()
                .any(|outcome| outcome.state == ConversionCrawlOutcomeStateV1::Failed)
        }) {
            bail!("conversion crawl contains failed package outcomes");
        }
        Ok(())
    }
}

fn validate_sha256(value: &str, field: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{field} must be an exact SHA-256 digest"
    );
    ensure!(
        value == value.to_ascii_lowercase(),
        "{field} must be lowercase"
    );
    Ok(())
}
