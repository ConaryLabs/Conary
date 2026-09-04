// apps/remi/src/server/promotion_evidence.rs
//! Complete proof binding for one exact public Remi candidate set.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use conary_core::canonical::{CanonicalMapSnapshot, validate_canonical_map_snapshot};
use conary_core::db::models::RemiCatalogPhysicalAttestation;
use conary_core::repository::catalog::{
    CatalogPackageRecordV1, CatalogReader, NATIVE_PARITY_COMPARISON_SCHEMA_V1,
    NATIVE_RESOLUTION_COMPARISON_SCHEMA_V2, NativeParityComparisonV1, NativeResolutionComparisonV1,
    ProfileRevisionV2, compare_native_parity_oracle, compare_native_resolution_oracle,
    verify_native_parity_oracle_bundle, verify_native_resolution_oracle_bundle,
    verify_registered_profile_catalog_bundle_complete,
};
use serde::{Deserialize, Serialize};

use super::conversion_crawl::{ConversionCrawlProfileV4, reopen_conversion_crawl};
use super::universe_validation::validate_canonical_candidate;

pub const REMI_PROMOTION_EVIDENCE_SCHEMA_V1: u32 = 1;
const MAX_PROMOTION_EVIDENCE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RemiPromotionProfileEvidenceInput {
    pub revision: ProfileRevisionV2,
    pub physical_attestation: RemiCatalogPhysicalAttestation,
    pub package_oracle_dir: PathBuf,
    pub native_resolution_dir: PathBuf,
    pub candidate_resolution_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RemiPromotionEvidenceConfig {
    pub catalog_dir: PathBuf,
    pub conversion_crawl_path: PathBuf,
    pub output_path: PathBuf,
    pub profiles: Vec<RemiPromotionProfileEvidenceInput>,
}

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
            self.resolution_parity.schema_version == NATIVE_RESOLUTION_COMPARISON_SCHEMA_V2
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

pub fn produce_remi_promotion_evidence(
    config: &RemiPromotionEvidenceConfig,
    canonical_map: &CanonicalMapSnapshot,
) -> Result<RemiPromotionEvidenceV1> {
    validate_candidate_inputs(&config.profiles)?;
    let (conversion_crawl, conversion_crawl_bytes) =
        reopen_conversion_crawl(&config.conversion_crawl_path)?;
    let conversion_crawl_sha256 = conary_core::hash::sha256(&conversion_crawl_bytes);

    validate_canonical_map_snapshot(canonical_map).map_err(anyhow::Error::from)?;
    validate_canonical_candidate(
        &config.catalog_dir,
        canonical_map,
        config
            .profiles
            .iter()
            .map(|input| (&input.revision, &input.physical_attestation)),
    )
    .context("validate canonical contracts against promotion candidates")?;
    let canonical_map_bytes = conary_core::json::canonical_json(canonical_map)
        .map_err(anyhow::Error::msg)
        .context("serialize promotion canonical map")?;

    let mut profiles = Vec::with_capacity(config.profiles.len());
    for (input, crawl_profile) in config.profiles.iter().zip(&conversion_crawl.profiles) {
        profiles.push(produce_profile_evidence(
            &config.catalog_dir,
            input,
            crawl_profile,
        )?);
    }
    let evidence = RemiPromotionEvidenceV1 {
        schema_version: REMI_PROMOTION_EVIDENCE_SCHEMA_V1,
        conversion_crawl_sha256,
        canonical_map: RemiPromotionCanonicalMapV1 {
            sha256: conary_core::hash::sha256(&canonical_map_bytes),
            revision: canonical_map.revision,
            entry_count: u64::try_from(canonical_map.entries.len())
                .context("promotion canonical-map entry count exceeds u64")?,
        },
        profiles,
    };
    write_and_reopen_promotion_evidence(&config.output_path, &evidence)
}

pub fn reopen_remi_promotion_evidence(path: &Path) -> Result<RemiPromotionEvidenceV1> {
    let bytes = read_plain_file(path, "promotion evidence")?;
    let evidence: RemiPromotionEvidenceV1 =
        serde_json::from_slice(&bytes).context("parse Remi promotion evidence")?;
    evidence.validate()?;
    let canonical = conary_core::json::canonical_json(&evidence)
        .map_err(anyhow::Error::msg)
        .context("serialize reopened Remi promotion evidence")?;
    ensure!(
        bytes == canonical,
        "reopened Remi promotion evidence is not canonical"
    );
    Ok(evidence)
}

fn validate_candidate_inputs(inputs: &[RemiPromotionProfileEvidenceInput]) -> Result<()> {
    let expected = conary_core::repository::supported_profiles::public_profiles();
    ensure!(
        inputs.len() == expected.len(),
        "promotion input names {} profiles but {} public profiles are required",
        inputs.len(),
        expected.len()
    );
    for (input, profile) in inputs.iter().zip(expected) {
        input.revision.validate().map_err(anyhow::Error::from)?;
        ensure!(
            input.revision.profile == profile.id(),
            "promotion input order or membership differs from the public profile contract"
        );
    }
    Ok(())
}

fn produce_profile_evidence(
    catalog_dir: &Path,
    input: &RemiPromotionProfileEvidenceInput,
    crawl: &ConversionCrawlProfileV4,
) -> Result<RemiPromotionProfileEvidenceV1> {
    let revision_sha256 = input.revision.manifest_sha256()?;
    ensure!(
        crawl.profile == input.revision.profile && crawl.profile_revision_sha256 == revision_sha256,
        "conversion crawl differs from promotion candidate profile '{}'",
        input.revision.profile
    );
    let catalog_path = catalog_dir
        .join("profiles")
        .join(&input.revision.profile)
        .join(&revision_sha256);
    let catalog = verify_registered_profile_catalog_bundle_complete(
        &catalog_path,
        &input.revision,
        &input.physical_attestation.portable_manifest,
    )
    .with_context(|| {
        format!(
            "reopen promotion candidate profile '{}' revision {}",
            input.revision.profile, revision_sha256
        )
    })?;
    compare_conversion_crawl(&catalog, crawl)?;

    let package_oracle =
        verify_native_parity_oracle_bundle(&input.package_oracle_dir, &input.revision)
            .with_context(|| {
                format!(
                    "reopen package-fact oracle for promotion profile '{}'",
                    input.revision.profile
                )
            })?;
    let package_parity = compare_native_parity_oracle(&input.revision, &catalog, &package_oracle)
        .map_err(anyhow::Error::from)
        .with_context(|| {
            format!(
                "compare package-fact parity for promotion profile '{}'",
                input.revision.profile
            )
        })?;
    let native_resolution = verify_native_resolution_oracle_bundle(
        &input.native_resolution_dir,
        &input.revision,
        &package_oracle,
    )
    .with_context(|| {
        format!(
            "reopen native resolution oracle for promotion profile '{}'",
            input.revision.profile
        )
    })?;
    let candidate_resolution = verify_native_resolution_oracle_bundle(
        &input.candidate_resolution_dir,
        &input.revision,
        &package_oracle,
    )
    .with_context(|| {
        format!(
            "reopen Conary resolution candidate for promotion profile '{}'",
            input.revision.profile
        )
    })?;
    let resolution_parity = compare_native_resolution_oracle(
        &input.revision,
        &package_oracle,
        &native_resolution,
        &candidate_resolution,
    )
    .map_err(anyhow::Error::from)
    .with_context(|| {
        format!(
            "compare resolution parity for promotion profile '{}'",
            input.revision.profile
        )
    })?;

    Ok(RemiPromotionProfileEvidenceV1 {
        ordinal: conary_core::repository::supported_profiles::public_profiles()
            .iter()
            .position(|profile| profile.id() == input.revision.profile)
            .map(u32::try_from)
            .transpose()?
            .context("promotion profile is outside the public contract")?,
        profile: input.revision.profile.clone(),
        profile_revision_sha256: revision_sha256,
        catalog_sha256: input.revision.catalog.sha256.clone(),
        catalog_size: input.revision.catalog.size,
        package_parity,
        resolution_parity,
    })
}

fn compare_conversion_crawl(
    catalog: &CatalogReader,
    crawl: &ConversionCrawlProfileV4,
) -> Result<()> {
    let mut packages = catalog.packages().map_err(anyhow::Error::from)?;
    packages.sort_by(package_identity_order);
    ensure!(
        crawl.expected_packages == u64::try_from(packages.len())?
            && crawl.outcomes.len() == packages.len(),
        "conversion crawl package count differs from candidate profile '{}'",
        crawl.profile
    );
    for (package, outcome) in packages.iter().zip(&crawl.outcomes) {
        ensure!(
            outcome.package_key_sha256 == package.package_key_sha256
                && outcome.name == package.name
                && outcome.version == package.version
                && outcome.package_release == package.package_release
                && outcome.architecture == package.architecture
                && outcome.repository_checksum == package.checksum,
            "conversion crawl package evidence differs from candidate package '{}:{}:{}:{}'",
            package.name,
            package.version,
            package.package_release,
            package.architecture.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

fn package_identity_order(
    left: &CatalogPackageRecordV1,
    right: &CatalogPackageRecordV1,
) -> std::cmp::Ordering {
    (
        &left.name,
        &left.version,
        &left.package_release,
        &left.architecture,
        &left.package_key_sha256,
    )
        .cmp(&(
            &right.name,
            &right.version,
            &right.package_release,
            &right.architecture,
            &right.package_key_sha256,
        ))
}

fn write_and_reopen_promotion_evidence(
    path: &Path,
    evidence: &RemiPromotionEvidenceV1,
) -> Result<RemiPromotionEvidenceV1> {
    evidence.validate()?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).context("create promotion evidence directory")?;
    let canonical = conary_core::json::canonical_json(evidence)
        .map_err(anyhow::Error::msg)
        .context("serialize Remi promotion evidence")?;
    let mut staged =
        tempfile::NamedTempFile::new_in(parent).context("create staged Remi promotion evidence")?;
    staged
        .write_all(&canonical)
        .context("write staged Remi promotion evidence")?;
    staged
        .as_file()
        .sync_all()
        .context("sync staged Remi promotion evidence")?;
    staged
        .persist(path)
        .map_err(|error| error.error)
        .context("publish Remi promotion evidence")?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .context("sync Remi promotion evidence directory")?;
    let reopened = reopen_remi_promotion_evidence(path)?;
    ensure!(
        reopened == *evidence,
        "reopened Remi promotion evidence differs from produced value"
    );
    Ok(reopened)
}

fn read_plain_file(path: &Path, label: &str) -> Result<Vec<u8>> {
    crate::server::private_output::read_regular_nofollow(path, label, MAX_PROMOTION_EVIDENCE_BYTES)
}

fn validate_sha256(value: &str, field: &str) -> Result<()> {
    ensure!(
        conary_core::hash::is_canonical_sha256(value),
        "{field} must be an exact lowercase SHA-256 digest"
    );
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
