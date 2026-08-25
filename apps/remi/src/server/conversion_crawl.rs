// apps/remi/src/server/conversion_crawl.rs
//! Strict full-universe conversion-crawl evidence.

mod ccs_reopen;
mod proof_reuse;
mod report;
mod target_preflight;
#[cfg(test)]
mod tests_v4;

use super::catalog_authority::{CatalogAuthority, PinnedProfileCatalog};
use super::conversion::ConversionService;
use super::database_writer::DatabaseWriter;
use super::profile_catalog::ProfileCatalog;
use anyhow::{Context, Result, ensure};
use ccs_reopen::CcsArtifactReopener;
use conary_core::corpus::ConversionFailure;
use conary_core::db::models::RemiActiveProfileRevision;
use conary_core::repository::catalog::CatalogPackageRecordV1;
use futures::StreamExt;
pub use proof_reuse::{
    CONVERSION_PROOF_KEY_SCHEMA_V1, CONVERSION_PROOF_SCHEMA_V1, ConversionProofDispositionV1,
    ConversionProofKeyV1, ConversionProofTargetContractV1, ConversionProofV1,
};
pub use report::{
    CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1, CcsArtifactReopenProofV1, ConversionCrawlFailureV4,
    ConversionCrawlOutcomeStateV4, ConversionCrawlPackageOutcomeV4, ConversionCrawlProfileV4,
    REMI_CONVERSION_CRAWL_SCHEMA_V4, RemiConversionCrawlV4,
};
use std::fs;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
pub use target_preflight::{
    CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1, CcsTargetCompatibilityProofV1,
};

struct ReopenedCcsArtifactEvidence {
    source_artifact_sha256: String,
    ccs_sha256: String,
    reopen_proof: CcsArtifactReopenProofV1,
    target_compatibility_proofs: Vec<CcsTargetCompatibilityProofV1>,
}

#[derive(Debug, Clone)]
pub struct ConversionCrawlConfig {
    pub db_path: PathBuf,
    pub catalog_dir: PathBuf,
    pub chunk_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub repository_keys_dir: PathBuf,
    pub output_path: PathBuf,
    pub concurrency: usize,
}

struct ProfileCrawlPlan {
    route: String,
    selection: RemiActiveProfileRevision,
    packages: Vec<CatalogPackageRecordV1>,
    _pin: PinnedProfileCatalog,
}

pub async fn run_conversion_crawl(config: &ConversionCrawlConfig) -> Result<RemiConversionCrawlV4> {
    ensure!(
        config.concurrency > 0,
        "conversion crawl concurrency must be greater than zero"
    );
    let database_writer = DatabaseWriter::default();
    let authority = CatalogAuthority::from_paths(
        config.db_path.clone(),
        config.catalog_dir.clone(),
        database_writer.clone(),
    );
    let plan_authority = authority.clone();
    let plans = tokio::task::spawn_blocking(move || build_crawl_plans(&plan_authority))
        .await
        .map_err(|error| anyhow::anyhow!("conversion crawl planning task panicked: {error}"))??;
    let service = ConversionService::new(
        config.chunk_dir.clone(),
        config.cache_dir.clone(),
        config.db_path.clone(),
        None,
    )
    .with_catalog_authority(authority)
    .with_database_writer(database_writer.clone())
    .with_repository_keys_dir(Some(config.repository_keys_dir.clone()));
    let proof_store =
        proof_reuse::ConversionProofStore::new(config.db_path.clone(), database_writer);

    let mut profiles = Vec::with_capacity(plans.len());
    for plan in plans {
        profiles.push(
            run_profile_crawl(
                &service,
                &proof_store,
                plan,
                &config.repository_keys_dir,
                config.concurrency,
            )
            .await?,
        );
    }
    let report = RemiConversionCrawlV4 {
        schema_version: REMI_CONVERSION_CRAWL_SCHEMA_V4,
        profiles,
    };
    write_and_reopen_conversion_crawl(&config.output_path, &report)?;
    report.validate_complete()?;
    Ok(report)
}

fn build_crawl_plans(authority: &CatalogAuthority) -> Result<Vec<ProfileCrawlPlan>> {
    conary_core::repository::supported_profiles::public_profiles()
        .iter()
        .map(|profile| {
            let pin = authority
                .open_active_profile(profile.id())
                .with_context(|| {
                    format!("pin public profile '{}' for conversion crawl", profile.id())
                })?;
            let selection = pin.activation().clone();
            let packages = ProfileCatalog::new(&pin)
                .package_records()
                .with_context(|| {
                    format!(
                        "enumerate public profile '{}' for conversion crawl",
                        profile.id()
                    )
                })?;
            ensure!(
                !packages.is_empty(),
                "public profile '{}' has no packages to crawl",
                profile.id()
            );
            ensure!(
                packages
                    .iter()
                    .all(|package| package.source_profile == profile.id()),
                "public profile '{}' catalog carries a foreign package profile",
                profile.id()
            );
            Ok(ProfileCrawlPlan {
                route: profile.remi_route_slug().to_string(),
                selection,
                packages,
                _pin: pin,
            })
        })
        .collect()
}

async fn run_profile_crawl(
    service: &ConversionService,
    proof_store: &proof_reuse::ConversionProofStore,
    plan: ProfileCrawlPlan,
    repository_keys_dir: &Path,
    concurrency: usize,
) -> Result<ConversionCrawlProfileV4> {
    let expected_packages =
        u64::try_from(plan.packages.len()).context("conversion crawl package count exceeds u64")?;
    let route = plan.route.clone();
    let selection = plan.selection.clone();
    let reopener =
        CcsArtifactReopener::for_profile(repository_keys_dir, &plan.selection.source_profile)?;
    let outcomes = crawl_packages(plan.packages, concurrency, move |package| {
        let service = service.clone();
        let proof_store = proof_store.clone();
        let reopener = reopener.clone();
        let route = route.clone();
        let selection = selection.clone();
        async move {
            proof_reuse::validate_or_reuse(
                proof_store,
                service,
                reopener,
                route,
                package,
                selection,
            )
            .await
        }
    })
    .await;
    Ok(ConversionCrawlProfileV4 {
        profile: plan.selection.source_profile,
        profile_revision_sha256: plan.selection.profile_revision_sha256,
        expected_packages,
        outcomes,
    })
}

async fn crawl_packages<F, Fut>(
    packages: Vec<CatalogPackageRecordV1>,
    concurrency: usize,
    validate: F,
) -> Vec<ConversionCrawlPackageOutcomeV4>
where
    F: Fn(CatalogPackageRecordV1) -> Fut + Clone,
    Fut: Future<Output = Result<(ConversionProofDispositionV1, ConversionProofV1)>>,
{
    futures::stream::iter(packages.into_iter().map(|package| {
        let validate = validate.clone();
        async move {
            let result = validate(package.clone()).await;
            package_outcome(package, result)
        }
    }))
    .buffered(concurrency)
    .collect::<Vec<_>>()
    .await
}

fn package_outcome(
    package: CatalogPackageRecordV1,
    result: Result<(ConversionProofDispositionV1, ConversionProofV1)>,
) -> ConversionCrawlPackageOutcomeV4 {
    let identity =
        |state, proof_disposition, conversion_proof, failure| ConversionCrawlPackageOutcomeV4 {
            package_key_sha256: package.package_key_sha256.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            package_release: package.package_release.clone(),
            architecture: package.architecture.clone(),
            repository_checksum: package.checksum.clone(),
            state,
            proof_disposition,
            conversion_proof,
            failure,
        };
    match result {
        Ok((disposition, proof)) => identity(
            ConversionCrawlOutcomeStateV4::Succeeded,
            Some(disposition),
            Some(proof),
            None,
        ),
        Err(error) => identity(
            ConversionCrawlOutcomeStateV4::Failed,
            None,
            None,
            Some(ConversionFailure::classify(&error).into()),
        ),
    }
}

pub(super) fn exact_prefixed_sha256<'a>(value: &'a str, field: &str) -> Result<&'a str> {
    let digest = value
        .strip_prefix("sha256:")
        .with_context(|| format!("{field} digest is not SHA-256"))?;
    validate_sha256(digest, field)?;
    Ok(digest)
}

pub fn write_and_reopen_conversion_crawl(
    path: &Path,
    report: &RemiConversionCrawlV4,
) -> Result<RemiConversionCrawlV4> {
    report.validate_structure()?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).context("create conversion crawl evidence directory")?;
    let canonical = serde_json::to_vec(report).context("encode conversion crawl evidence")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .context("create staged conversion crawl evidence")?;
    staged
        .write_all(&canonical)
        .context("write staged conversion crawl evidence")?;
    staged
        .as_file()
        .sync_all()
        .context("sync staged conversion crawl evidence")?;
    staged
        .persist(path)
        .map_err(|error| error.error)
        .context("publish conversion crawl evidence")?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .context("sync conversion crawl evidence directory")?;

    let reopened_bytes = fs::read(path).context("reopen conversion crawl evidence bytes")?;
    ensure!(
        reopened_bytes == canonical,
        "reopened conversion crawl evidence is not canonical"
    );
    let reopened: RemiConversionCrawlV4 = serde_json::from_slice(&reopened_bytes)
        .context("parse reopened conversion crawl evidence")?;
    reopened.validate_structure()?;
    ensure!(
        &reopened == report,
        "reopened conversion crawl evidence differs from the written report"
    );
    Ok(reopened)
}

pub(super) fn validate_sha256(value: &str, field: &str) -> Result<()> {
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
