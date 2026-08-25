// apps/remi/src/server/conversion_crawl.rs
//! Strict full-universe conversion-crawl evidence.

mod ccs_reopen;
mod target_preflight;

use super::catalog_authority::{CatalogAuthority, PinnedProfileCatalog};
use super::conversion::ConversionService;
use super::database_writer::DatabaseWriter;
use super::profile_catalog::ProfileCatalog;
use anyhow::{Context, Result, bail, ensure};
use ccs_reopen::CcsArtifactReopener;
use conary_core::corpus::{ConversionFailure, FailureKind};
use conary_core::db::models::RemiActiveProfileRevision;
use conary_core::repository::catalog::CatalogPackageRecordV1;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
pub use target_preflight::{
    CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1, CcsTargetCompatibilityProofV1,
};

pub const REMI_CONVERSION_CRAWL_SCHEMA_V3: u32 = 3;
pub const CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversionCrawlOutcomeStateV3 {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionCrawlFailureV3 {
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

struct ReopenedCcsArtifactEvidence {
    source_artifact_sha256: String,
    ccs_sha256: String,
    reopen_proof: CcsArtifactReopenProofV1,
    target_compatibility_proofs: Vec<CcsTargetCompatibilityProofV1>,
}

impl CcsArtifactReopenProofV1 {
    fn validate(&self) -> Result<()> {
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

impl From<ConversionFailure> for ConversionCrawlFailureV3 {
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
pub struct ConversionCrawlPackageOutcomeV3 {
    pub package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    pub repository_checksum: String,
    pub state: ConversionCrawlOutcomeStateV3,
    pub source_artifact_sha256: Option<String>,
    pub ccs_sha256: Option<String>,
    pub ccs_reopen_proof: Option<CcsArtifactReopenProofV1>,
    pub target_compatibility_proofs: Vec<CcsTargetCompatibilityProofV1>,
    pub failure: Option<ConversionCrawlFailureV3>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionCrawlProfileV3 {
    pub profile: String,
    pub profile_revision_sha256: String,
    pub expected_packages: u64,
    pub outcomes: Vec<ConversionCrawlPackageOutcomeV3>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemiConversionCrawlV3 {
    pub schema_version: u32,
    pub profiles: Vec<ConversionCrawlProfileV3>,
}

impl RemiConversionCrawlV3 {
    pub fn validate_structure(&self) -> Result<()> {
        ensure!(
            self.schema_version == REMI_CONVERSION_CRAWL_SCHEMA_V3,
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
                    ConversionCrawlOutcomeStateV3::Succeeded => {
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
                        outcome
                            .ccs_reopen_proof
                            .as_ref()
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "successful crawl outcome has no CCS artifact reopen proof"
                                )
                            })?
                            .validate()?;
                        target_preflight::validate_complete_target_proofs(
                            &outcome.target_compatibility_proofs,
                            ccs,
                        )?;
                        ensure!(
                            outcome.failure.is_none(),
                            "successful crawl outcome carries failure evidence"
                        );
                    }
                    ConversionCrawlOutcomeStateV3::Failed => {
                        let failure = outcome.failure.as_ref().ok_or_else(|| {
                            anyhow::anyhow!("failed crawl outcome has no typed failure evidence")
                        })?;
                        ensure!(
                            (failure.kind == FailureKind::InternalUnclassified)
                                == failure.incident_id.is_some(),
                            "failed crawl outcome has invalid incident identity"
                        );
                        ensure!(
                            outcome.source_artifact_sha256.is_none()
                                && outcome.ccs_sha256.is_none()
                                && outcome.ccs_reopen_proof.is_none()
                                && outcome.target_compatibility_proofs.is_empty(),
                            "failed crawl outcome carries success evidence"
                        );
                    }
                }
            }
        }

        Ok(())
    }

    pub fn validate_complete(&self) -> Result<()> {
        self.validate_structure()?;
        if self.profiles.iter().any(|profile| {
            profile
                .outcomes
                .iter()
                .any(|outcome| outcome.state == ConversionCrawlOutcomeStateV3::Failed)
        }) {
            bail!("conversion crawl contains failed package outcomes");
        }
        Ok(())
    }
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

pub async fn run_conversion_crawl(config: &ConversionCrawlConfig) -> Result<RemiConversionCrawlV3> {
    ensure!(
        config.concurrency > 0,
        "conversion crawl concurrency must be greater than zero"
    );
    let authority = CatalogAuthority::from_paths(
        config.db_path.clone(),
        config.catalog_dir.clone(),
        DatabaseWriter::default(),
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
    .with_repository_keys_dir(Some(config.repository_keys_dir.clone()));

    let mut profiles = Vec::with_capacity(plans.len());
    for plan in plans {
        profiles.push(
            run_profile_crawl(
                &service,
                plan,
                &config.repository_keys_dir,
                config.concurrency,
            )
            .await?,
        );
    }
    let report = RemiConversionCrawlV3 {
        schema_version: REMI_CONVERSION_CRAWL_SCHEMA_V3,
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
    plan: ProfileCrawlPlan,
    repository_keys_dir: &Path,
    concurrency: usize,
) -> Result<ConversionCrawlProfileV3> {
    let expected_packages =
        u64::try_from(plan.packages.len()).context("conversion crawl package count exceeds u64")?;
    let route = plan.route.clone();
    let selection = plan.selection.clone();
    let reopener =
        CcsArtifactReopener::for_profile(repository_keys_dir, &plan.selection.source_profile)?;
    let outcomes = crawl_packages(
        plan.packages,
        concurrency,
        move |package| {
            let service = service.clone();
            let route = route.clone();
            let selection = selection.clone();
            async move {
                service
                    .convert_catalog_package_from_selection_async(&route, package, selection)
                    .await
            }
        },
        move |package, result| reopener.reopen(package, result),
    )
    .await;
    Ok(ConversionCrawlProfileV3 {
        profile: plan.selection.source_profile,
        profile_revision_sha256: plan.selection.profile_revision_sha256,
        expected_packages,
        outcomes,
    })
}

async fn crawl_packages<F, Fut, R>(
    packages: Vec<CatalogPackageRecordV1>,
    concurrency: usize,
    convert: F,
    reopen: R,
) -> Vec<ConversionCrawlPackageOutcomeV3>
where
    F: Fn(CatalogPackageRecordV1) -> Fut + Clone,
    Fut: Future<Output = Result<super::ServerConversionResult>>,
    R: Fn(
            &CatalogPackageRecordV1,
            &super::ServerConversionResult,
        ) -> Result<ReopenedCcsArtifactEvidence>
        + Clone,
{
    futures::stream::iter(packages.into_iter().map(|package| {
        let convert = convert.clone();
        let reopen = reopen.clone();
        async move {
            let result = convert(package.clone()).await;
            package_outcome(package, result, reopen)
        }
    }))
    .buffered(concurrency)
    .collect::<Vec<_>>()
    .await
}

fn package_outcome<R>(
    package: CatalogPackageRecordV1,
    result: Result<super::ServerConversionResult>,
    reopen: R,
) -> ConversionCrawlPackageOutcomeV3
where
    R: FnOnce(
        &CatalogPackageRecordV1,
        &super::ServerConversionResult,
    ) -> Result<ReopenedCcsArtifactEvidence>,
{
    let identity = |state,
                    source_artifact_sha256,
                    ccs_sha256,
                    ccs_reopen_proof,
                    target_compatibility_proofs,
                    failure| {
        ConversionCrawlPackageOutcomeV3 {
            package_key_sha256: package.package_key_sha256.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            package_release: package.package_release.clone(),
            architecture: package.architecture.clone(),
            repository_checksum: package.checksum.clone(),
            state,
            source_artifact_sha256,
            ccs_sha256,
            ccs_reopen_proof,
            target_compatibility_proofs,
            failure,
        }
    };
    match result {
        Ok(result) if result.cache_state != "cold" => identity(
            ConversionCrawlOutcomeStateV3::Failed,
            None,
            None,
            None,
            Vec::new(),
            Some(
                ConversionFailure::Publication {
                    detail: "initial crawl encountered unkeyed prior conversion proof".to_string(),
                }
                .into(),
            ),
        ),
        Ok(result) => match reopen(&package, &result) {
            Ok(evidence) => identity(
                ConversionCrawlOutcomeStateV3::Succeeded,
                Some(evidence.source_artifact_sha256),
                Some(evidence.ccs_sha256),
                Some(evidence.reopen_proof),
                evidence.target_compatibility_proofs,
                None,
            ),
            Err(error) => identity(
                ConversionCrawlOutcomeStateV3::Failed,
                None,
                None,
                None,
                Vec::new(),
                Some(ConversionFailure::classify(&error).into()),
            ),
        },
        Err(error) => identity(
            ConversionCrawlOutcomeStateV3::Failed,
            None,
            None,
            None,
            Vec::new(),
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
    report: &RemiConversionCrawlV3,
) -> Result<RemiConversionCrawlV3> {
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
    let reopened: RemiConversionCrawlV3 = serde_json::from_slice(&reopened_bytes)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use crate::server::conversion::ScriptletPackageMetadata;
    use conary_core::ccs::attestation::{
        BuildOutputIdentity, FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1, ForeignConversionBoundary,
    };

    fn success_outcome(name: &str, marker: &str) -> ConversionCrawlPackageOutcomeV3 {
        ConversionCrawlPackageOutcomeV3 {
            package_key_sha256: conary_core::hash::sha256(format!("key-{marker}").as_bytes()),
            name: name.to_string(),
            version: "1.0".to_string(),
            package_release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            repository_checksum: format!("sha256:{}", "a".repeat(64)),
            state: ConversionCrawlOutcomeStateV3::Succeeded,
            source_artifact_sha256: Some("b".repeat(64)),
            ccs_sha256: Some("c".repeat(64)),
            ccs_reopen_proof: Some(reopen_proof()),
            target_compatibility_proofs: target_proofs(&"c".repeat(64)),
            failure: None,
        }
    }

    fn reopen_proof() -> CcsArtifactReopenProofV1 {
        CcsArtifactReopenProofV1 {
            schema_version: CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1,
            ccs_format_version: conary_core::ccs::v3::FORMAT_VERSION_V3,
            foreign_conversion_boundary_schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            signer_public_key_sha256: "3".repeat(64),
            transport_sha256: "4".repeat(64),
            verified_files: 1,
            verified_objects: 1,
        }
    }

    fn target_proofs(ccs_sha256: &str) -> Vec<CcsTargetCompatibilityProofV1> {
        conary_core::ccs::supported_target_contracts()
            .iter()
            .map(|contract| CcsTargetCompatibilityProofV1 {
                schema_version: CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                ccs_sha256: ccs_sha256.to_string(),
                compatibility: conary_core::ccs::StaticTargetCompatibilityProofV1 {
                    schema_version: conary_core::ccs::STATIC_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                    target_profile: contract.target_profile,
                    target_contract_sha256: contract.sha256().expect("target contract digest"),
                    required_capabilities: Vec::new(),
                    required_systemd_operations: Vec::new(),
                    required_linux_process_capabilities: Vec::new(),
                },
            })
            .collect()
    }

    fn valid_report() -> RemiConversionCrawlV3 {
        RemiConversionCrawlV3 {
            schema_version: REMI_CONVERSION_CRAWL_SCHEMA_V3,
            profiles: conary_core::repository::supported_profiles::public_profiles()
                .iter()
                .map(|profile| ConversionCrawlProfileV3 {
                    profile: profile.id().to_string(),
                    profile_revision_sha256: conary_core::hash::sha256(profile.id().as_bytes()),
                    expected_packages: 1,
                    outcomes: vec![success_outcome("demo", profile.id())],
                })
                .collect(),
        }
    }

    fn conversion_result(
        package: &CatalogPackageRecordV1,
        cache_state: &str,
    ) -> super::super::ServerConversionResult {
        let boundary = ForeignConversionBoundary {
            schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            source_format: "rpm".to_string(),
            source_checksum: format!("sha256:{}", "d".repeat(64)),
            output_identity: BuildOutputIdentity {
                file_merkle_root: "e".repeat(64),
                package_name: package.name.clone(),
                package_version: package.version.clone(),
                package_release: package.package_release.clone(),
                architecture: package.architecture.clone(),
                origin_class: "foreign_conversion".to_string(),
                hardening_level: "converted".to_string(),
                hermetic_evidence_hash: "f".repeat(64),
                canonical_content_identity: "1".repeat(64),
            },
            build_risk_report_hash: None,
            build_risk_report: None,
            scriptlet_risk_report_hash: None,
            scriptlet_risk_report: None,
            diagnostics: Vec::new(),
        };
        super::super::ServerConversionResult {
            name: package.name.clone(),
            version: package.version.clone(),
            source_profile: Some(package.source_profile.clone()),
            transport: conary_core::ccs::CcsTransportEnvelopeV1 {
                schema_version: conary_core::ccs::transport::CCS_TRANSPORT_SCHEMA_V1,
                manifest_base64: String::new(),
                signature_json: "{}".to_string(),
                debug_toml_base64: None,
                build_attestation_json: None,
                foreign_conversion_boundary_json: Some(
                    serde_json::to_string(&boundary).expect("serialize boundary"),
                ),
                objects: Vec::new(),
            },
            total_size: 1,
            content_hash: format!("sha256:{}", "2".repeat(64)),
            ccs_path: PathBuf::from("demo.ccs"),
            cache_state: cache_state.to_string(),
            scriptlets: ScriptletPackageMetadata {
                scriptlet_fidelity: "none".to_string(),
                evidence_digest: None,
            },
            timing: None,
        }
    }

    fn synthetic_reopen(
        package: &CatalogPackageRecordV1,
        result: &super::super::ServerConversionResult,
    ) -> Result<ReopenedCcsArtifactEvidence> {
        ensure!(
            result.name == package.name
                && result.version == package.version
                && result.source_profile.as_deref() == Some(package.source_profile.as_str()),
            "conversion result identity differs from the exact catalog package"
        );
        let boundary_json = result
            .transport
            .foreign_conversion_boundary_json
            .as_deref()
            .context("converted CCS transport has no foreign conversion boundary")?;
        let boundary: ForeignConversionBoundary = serde_json::from_str(boundary_json)?;
        ensure!(
            boundary.output_identity.package_name == package.name
                && boundary.output_identity.package_version == package.version
                && boundary.output_identity.package_release == package.package_release
                && boundary.output_identity.architecture == package.architecture,
            "converted CCS boundary identity differs from the exact catalog package"
        );
        let source_artifact_sha256 =
            exact_prefixed_sha256(&boundary.source_checksum, "source artifact")?.to_string();
        let ccs_sha256 = exact_prefixed_sha256(&result.content_hash, "converted CCS")?.to_string();
        Ok(ReopenedCcsArtifactEvidence {
            source_artifact_sha256,
            target_compatibility_proofs: target_proofs(&ccs_sha256),
            ccs_sha256,
            reopen_proof: reopen_proof(),
        })
    }

    #[test]
    fn crawl_contract_requires_every_ordered_public_profile() {
        let report = valid_report();
        report.validate_complete().expect("valid complete crawl");
        assert_eq!(
            report
                .profiles
                .iter()
                .map(|profile| profile.profile.as_str())
                .collect::<Vec<_>>(),
            vec!["fedora-44", "ubuntu-26.04", "arch"]
        );

        let mut missing = report.clone();
        missing.profiles.pop();
        assert!(missing.validate_structure().is_err());

        let mut candidate = report;
        candidate.profiles[2].profile = "solus".to_string();
        assert!(candidate.validate_structure().is_err());
    }

    #[test]
    fn crawl_contract_rejects_missing_repeated_reordered_and_failed_outcomes() {
        let mut missing = valid_report();
        missing.profiles[0].expected_packages = 2;
        assert!(missing.validate_structure().is_err());

        let mut superseded = valid_report();
        superseded.schema_version = 2;
        assert!(superseded.validate_structure().is_err());

        let mut missing_reopen = valid_report();
        missing_reopen.profiles[0].outcomes[0].ccs_reopen_proof = None;
        assert!(missing_reopen.validate_structure().is_err());

        let mut missing_target = valid_report();
        missing_target.profiles[0].outcomes[0]
            .target_compatibility_proofs
            .pop();
        assert!(missing_target.validate_structure().is_err());

        let mut reordered_target = valid_report();
        reordered_target.profiles[0].outcomes[0]
            .target_compatibility_proofs
            .swap(0, 1);
        assert!(reordered_target.validate_structure().is_err());

        let mut drifted_target = valid_report();
        drifted_target.profiles[0].outcomes[0].target_compatibility_proofs[0]
            .compatibility
            .target_contract_sha256 = "0".repeat(64);
        assert!(drifted_target.validate_structure().is_err());

        let mut repeated = valid_report();
        let duplicate = repeated.profiles[0].outcomes[0].clone();
        repeated.profiles[0].outcomes.push(duplicate);
        repeated.profiles[0].expected_packages = 2;
        assert!(repeated.validate_structure().is_err());

        let mut reordered = valid_report();
        reordered.profiles[0].outcomes = vec![
            success_outcome("z-package", "z"),
            success_outcome("a-package", "a"),
        ];
        reordered.profiles[0].expected_packages = 2;
        assert!(reordered.validate_structure().is_err());

        let mut failed = valid_report();
        let outcome = &mut failed.profiles[0].outcomes[0];
        outcome.state = ConversionCrawlOutcomeStateV3::Failed;
        outcome.source_artifact_sha256 = None;
        outcome.ccs_sha256 = None;
        outcome.ccs_reopen_proof = None;
        outcome.target_compatibility_proofs.clear();
        outcome.failure = Some(ConversionCrawlFailureV3 {
            kind: FailureKind::Publication,
            incident_id: None,
        });
        failed
            .validate_structure()
            .expect("failed evidence remains structurally inspectable");
        assert!(failed.validate_complete().is_err());
    }

    #[test]
    fn crawl_write_is_canonical_strict_and_independently_reopened() {
        let directory = tempfile::tempdir().expect("crawl evidence directory");
        let path = directory.path().join("crawl.json");
        let report = valid_report();
        let reopened = write_and_reopen_conversion_crawl(&path, &report)
            .expect("write and reopen canonical crawl evidence");
        assert_eq!(reopened, report);

        let mut value = serde_json::to_value(&report).expect("crawl JSON");
        value
            .as_object_mut()
            .expect("crawl object")
            .insert("unexpected".to_string(), serde_json::json!(true));
        assert!(serde_json::from_value::<RemiConversionCrawlV3>(value).is_err());
    }

    #[test]
    fn crawl_success_requires_fresh_cold_conversion_digests() {
        let package = package("fedora-44", "demo", "1.0", "1", Some("x86_64"), 42, "demo");
        let cold = package_outcome(
            package.clone(),
            Ok(conversion_result(&package, "cold")),
            synthetic_reopen,
        );
        assert_eq!(cold.state, ConversionCrawlOutcomeStateV3::Succeeded);
        assert_eq!(cold.source_artifact_sha256, Some("d".repeat(64)));
        assert_eq!(cold.ccs_sha256, Some("2".repeat(64)));

        let mut wrong_identity = conversion_result(&package, "cold");
        wrong_identity.name = "other".to_string();
        let wrong = package_outcome(package.clone(), Ok(wrong_identity), synthetic_reopen);
        assert_eq!(wrong.state, ConversionCrawlOutcomeStateV3::Failed);

        let hot_result = conversion_result(&package, "hot");
        let hot = package_outcome(package, Ok(hot_result), synthetic_reopen);
        assert_eq!(hot.state, ConversionCrawlOutcomeStateV3::Failed);
        assert_eq!(
            hot.failure.expect("typed hot-cache failure").kind,
            FailureKind::Publication
        );
    }

    #[tokio::test]
    async fn crawl_attempts_every_exact_variant_once_and_preserves_canonical_order() {
        let mut first = package(
            "fedora-44",
            "alpha",
            "1.0",
            "1",
            Some("x86_64"),
            42,
            "alpha",
        );
        first.package_key_sha256 = "1".repeat(64);
        let mut second = package("fedora-44", "beta", "1.0", "1", Some("x86_64"), 42, "beta");
        second.package_key_sha256 = "2".repeat(64);
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::<
            String,
            usize,
        >::new()));
        let outcomes = crawl_packages(
            vec![first, second],
            2,
            {
                let attempts = std::sync::Arc::clone(&attempts);
                move |package| {
                    let attempts = std::sync::Arc::clone(&attempts);
                    async move {
                        *attempts
                            .lock()
                            .expect("attempt counter")
                            .entry(package.package_key_sha256.clone())
                            .or_default() += 1;
                        if package.name == "alpha" {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                        Ok(conversion_result(&package, "cold"))
                    }
                }
            },
            synthetic_reopen,
        )
        .await;

        assert_eq!(
            outcomes
                .iter()
                .map(|outcome| outcome.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(
            attempts
                .lock()
                .expect("attempt counts")
                .values()
                .copied()
                .collect::<Vec<_>>(),
            vec![1, 1]
        );
        assert!(
            outcomes
                .iter()
                .all(|outcome| outcome.state == ConversionCrawlOutcomeStateV3::Succeeded)
        );
    }

    #[test]
    fn crawl_planning_pins_every_public_profile_and_excludes_candidate_tiers() {
        let fixture = ActiveCatalogFixture::new();
        for (index, profile) in conary_core::repository::supported_profiles::public_profiles()
            .iter()
            .enumerate()
        {
            fixture.activate(
                profile.id(),
                i64::try_from(index + 1).expect("fixture epoch"),
                vec![package(
                    profile.id(),
                    "demo",
                    "1.0",
                    "1",
                    Some("x86_64"),
                    42,
                    profile.id(),
                )],
            );
        }
        let plans = build_crawl_plans(fixture.authority()).expect("build public crawl plans");
        assert_eq!(
            plans
                .iter()
                .map(|plan| plan.selection.source_profile.as_str())
                .collect::<Vec<_>>(),
            vec!["fedora-44", "ubuntu-26.04", "arch"]
        );
        assert!(plans.iter().all(|plan| plan.packages.len() == 1));
        assert!(
            plans
                .iter()
                .all(|plan| plan.selection.source_profile != "solus")
        );
    }
}
