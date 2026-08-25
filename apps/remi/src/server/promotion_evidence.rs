// apps/remi/src/server/promotion_evidence.rs
//! Complete proof binding for one exact public Remi candidate set.

use anyhow::{Result, ensure};
use conary_core::repository::catalog::{
    NATIVE_PARITY_COMPARISON_SCHEMA_V1, NATIVE_RESOLUTION_COMPARISON_SCHEMA_V1,
    NativeParityComparisonV1, NativeResolutionComparisonV1,
};
use serde::{Deserialize, Serialize};

pub const REMI_PROMOTION_EVIDENCE_SCHEMA_V1: u32 = 1;

/// Exact canonical-map candidate validated against the supplied catalogs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiPromotionCanonicalMapV1 {
    pub sha256: String,
    pub revision: u64,
    pub entry_count: u64,
}

/// Complete parity bindings for one exact public profile candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiPromotionProfileEvidenceV1 {
    pub ordinal: u32,
    pub profile: String,
    pub profile_revision_sha256: String,
    pub catalog_sha256: String,
    pub catalog_size: u64,
    pub package_parity: NativeParityComparisonV1,
    pub resolution_parity: NativeResolutionComparisonV1,
}

/// One deterministic promotion proof for the exact ordered public universe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiPromotionEvidenceV1 {
    pub schema_version: u32,
    pub conversion_crawl_sha256: String,
    pub canonical_map: RemiPromotionCanonicalMapV1,
    pub profiles: Vec<RemiPromotionProfileEvidenceV1>,
}

impl RemiPromotionEvidenceV1 {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == REMI_PROMOTION_EVIDENCE_SCHEMA_V1,
            "unsupported Remi promotion evidence schema {}",
            self.schema_version
        );
        validate_sha256(
            &self.conversion_crawl_sha256,
            "promotion conversion-crawl SHA-256",
        )?;
        validate_sha256(
            &self.canonical_map.sha256,
            "promotion canonical-map SHA-256",
        )?;

        let expected_profiles = conary_core::repository::supported_profiles::public_profiles();
        ensure!(
            self.profiles.len() == expected_profiles.len(),
            "promotion evidence names {} profiles but {} public profiles are required",
            self.profiles.len(),
            expected_profiles.len()
        );
        for (index, (profile, expected)) in self.profiles.iter().zip(expected_profiles).enumerate()
        {
            let ordinal = u32::try_from(index)?;
            ensure!(
                profile.ordinal == ordinal && profile.profile == expected.id(),
                "promotion profile order or membership differs from the public profile contract"
            );
            profile.validate()?;
        }
        Ok(())
    }
}

impl RemiPromotionProfileEvidenceV1 {
    fn validate(&self) -> Result<()> {
        validate_sha256(
            &self.profile_revision_sha256,
            "promotion profile revision SHA-256",
        )?;
        validate_sha256(&self.catalog_sha256, "promotion catalog SHA-256")?;
        ensure!(self.catalog_size > 0, "promotion catalog is empty");
        ensure!(
            self.package_parity.schema_version == NATIVE_PARITY_COMPARISON_SCHEMA_V1
                && self.package_parity.profile == self.profile
                && self.package_parity.profile_revision_sha256 == self.profile_revision_sha256,
            "promotion package parity differs from its exact profile candidate"
        );
        ensure!(
            self.resolution_parity.schema_version == NATIVE_RESOLUTION_COMPARISON_SCHEMA_V1
                && self.resolution_parity.profile == self.profile
                && self.resolution_parity.profile_revision_sha256 == self.profile_revision_sha256
                && self.resolution_parity.package_oracle_manifest_sha256
                    == self.package_parity.oracle_manifest_sha256,
            "promotion resolution parity differs from its exact package evidence"
        );
        ensure!(
            self.package_parity.counts.packages == self.resolution_parity.counts.roots,
            "promotion package and resolution root counts differ"
        );
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
