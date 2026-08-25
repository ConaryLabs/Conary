// apps/remi/src/server/conversion_crawl.rs
//! Strict full-universe conversion-crawl evidence.

use super::catalog_authority::{CatalogAuthority, PinnedProfileCatalog};
use super::conversion::ConversionService;
use super::database_writer::DatabaseWriter;
use super::profile_catalog::ProfileCatalog;
use anyhow::{Context, Result, bail, ensure};
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

pub const REMI_CONVERSION_CRAWL_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversionCrawlOutcomeStateV1 {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionCrawlFailureV1 {
    pub kind: FailureKind,
    pub incident_id: Option<String>,
}

impl From<ConversionFailure> for ConversionCrawlFailureV1 {
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
pub struct ConversionCrawlPackageOutcomeV1 {
    pub package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    pub repository_checksum: String,
    pub state: ConversionCrawlOutcomeStateV1,
    pub source_artifact_sha256: Option<String>,
    pub ccs_sha256: Option<String>,
    pub failure: Option<ConversionCrawlFailureV1>,
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
    pub fn validate_structure(&self) -> Result<()> {
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
                                && outcome.ccs_sha256.is_none(),
                            "failed crawl outcome carries success digests"
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
                .any(|outcome| outcome.state == ConversionCrawlOutcomeStateV1::Failed)
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

pub async fn run_conversion_crawl(config: &ConversionCrawlConfig) -> Result<RemiConversionCrawlV1> {
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
        profiles.push(run_profile_crawl(&service, plan, config.concurrency).await?);
    }
    let report = RemiConversionCrawlV1 {
        schema_version: REMI_CONVERSION_CRAWL_SCHEMA_V1,
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
    concurrency: usize,
) -> Result<ConversionCrawlProfileV1> {
    let expected_packages =
        u64::try_from(plan.packages.len()).context("conversion crawl package count exceeds u64")?;
    let route = plan.route.clone();
    let selection = plan.selection.clone();
    let outcomes = crawl_packages(plan.packages, concurrency, move |package| {
        let service = service.clone();
        let route = route.clone();
        let selection = selection.clone();
        async move {
            service
                .convert_catalog_package_from_selection_async(&route, package, selection)
                .await
        }
    })
    .await;
    Ok(ConversionCrawlProfileV1 {
        profile: plan.selection.source_profile,
        profile_revision_sha256: plan.selection.profile_revision_sha256,
        expected_packages,
        outcomes,
    })
}

async fn crawl_packages<F, Fut>(
    packages: Vec<CatalogPackageRecordV1>,
    concurrency: usize,
    convert: F,
) -> Vec<ConversionCrawlPackageOutcomeV1>
where
    F: Fn(CatalogPackageRecordV1) -> Fut + Clone,
    Fut: Future<Output = Result<super::ServerConversionResult>>,
{
    futures::stream::iter(packages.into_iter().map(|package| {
        let convert = convert.clone();
        async move {
            let result = convert(package.clone()).await;
            package_outcome(package, result)
        }
    }))
    .buffered(concurrency)
    .collect::<Vec<_>>()
    .await
}

fn package_outcome(
    package: CatalogPackageRecordV1,
    result: Result<super::ServerConversionResult>,
) -> ConversionCrawlPackageOutcomeV1 {
    let identity =
        |state, source_artifact_sha256, ccs_sha256, failure| ConversionCrawlPackageOutcomeV1 {
            package_key_sha256: package.package_key_sha256.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            package_release: package.package_release.clone(),
            architecture: package.architecture.clone(),
            repository_checksum: package.checksum.clone(),
            state,
            source_artifact_sha256,
            ccs_sha256,
            failure,
        };
    match result {
        Ok(result) if result.cache_state != "cold" => identity(
            ConversionCrawlOutcomeStateV1::Failed,
            None,
            None,
            Some(
                ConversionFailure::Publication {
                    detail: "initial crawl encountered unkeyed prior conversion proof".to_string(),
                }
                .into(),
            ),
        ),
        Ok(result) => match conversion_success_digests(&package, result) {
            Ok((source, ccs)) => identity(
                ConversionCrawlOutcomeStateV1::Succeeded,
                Some(source),
                Some(ccs),
                None,
            ),
            Err(error) => identity(
                ConversionCrawlOutcomeStateV1::Failed,
                None,
                None,
                Some(ConversionFailure::classify(&error).into()),
            ),
        },
        Err(error) => identity(
            ConversionCrawlOutcomeStateV1::Failed,
            None,
            None,
            Some(ConversionFailure::classify(&error).into()),
        ),
    }
}

fn conversion_success_digests(
    package: &CatalogPackageRecordV1,
    result: super::ServerConversionResult,
) -> Result<(String, String)> {
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
    let boundary: conary_core::ccs::attestation::ForeignConversionBoundary =
        serde_json::from_str(boundary_json)
            .context("parse converted CCS foreign conversion boundary")?;
    ensure!(
        boundary.output_identity.package_name == package.name
            && boundary.output_identity.package_version == package.version
            && boundary.output_identity.package_release == package.package_release
            && boundary.output_identity.architecture == package.architecture,
        "converted CCS boundary identity differs from the exact catalog package"
    );
    let source = exact_prefixed_sha256(&boundary.source_checksum, "source artifact")?;
    let ccs = exact_prefixed_sha256(&result.content_hash, "converted CCS")?;
    Ok((source.to_string(), ccs.to_string()))
}

fn exact_prefixed_sha256<'a>(value: &'a str, field: &str) -> Result<&'a str> {
    let digest = value
        .strip_prefix("sha256:")
        .with_context(|| format!("{field} digest is not SHA-256"))?;
    validate_sha256(digest, field)?;
    Ok(digest)
}

pub fn write_and_reopen_conversion_crawl(
    path: &Path,
    report: &RemiConversionCrawlV1,
) -> Result<RemiConversionCrawlV1> {
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
    let reopened: RemiConversionCrawlV1 = serde_json::from_slice(&reopened_bytes)
        .context("parse reopened conversion crawl evidence")?;
    reopened.validate_structure()?;
    ensure!(
        &reopened == report,
        "reopened conversion crawl evidence differs from the written report"
    );
    Ok(reopened)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use crate::server::conversion::ScriptletPackageMetadata;
    use conary_core::ccs::attestation::{
        BuildOutputIdentity, FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1, ForeignConversionBoundary,
    };

    fn success_outcome(name: &str, marker: &str) -> ConversionCrawlPackageOutcomeV1 {
        ConversionCrawlPackageOutcomeV1 {
            package_key_sha256: conary_core::hash::sha256(format!("key-{marker}").as_bytes()),
            name: name.to_string(),
            version: "1.0".to_string(),
            package_release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            repository_checksum: format!("sha256:{}", "a".repeat(64)),
            state: ConversionCrawlOutcomeStateV1::Succeeded,
            source_artifact_sha256: Some("b".repeat(64)),
            ccs_sha256: Some("c".repeat(64)),
            failure: None,
        }
    }

    fn valid_report() -> RemiConversionCrawlV1 {
        RemiConversionCrawlV1 {
            schema_version: REMI_CONVERSION_CRAWL_SCHEMA_V1,
            profiles: conary_core::repository::supported_profiles::public_profiles()
                .iter()
                .map(|profile| ConversionCrawlProfileV1 {
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
        outcome.state = ConversionCrawlOutcomeStateV1::Failed;
        outcome.source_artifact_sha256 = None;
        outcome.ccs_sha256 = None;
        outcome.failure = Some(ConversionCrawlFailureV1 {
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
        assert!(serde_json::from_value::<RemiConversionCrawlV1>(value).is_err());
    }

    #[test]
    fn crawl_success_requires_fresh_cold_conversion_digests() {
        let package = package("fedora-44", "demo", "1.0", "1", Some("x86_64"), 42, "demo");
        let cold = package_outcome(package.clone(), Ok(conversion_result(&package, "cold")));
        assert_eq!(cold.state, ConversionCrawlOutcomeStateV1::Succeeded);
        assert_eq!(cold.source_artifact_sha256, Some("d".repeat(64)));
        assert_eq!(cold.ccs_sha256, Some("2".repeat(64)));

        let mut wrong_identity = conversion_result(&package, "cold");
        wrong_identity.name = "other".to_string();
        let wrong = package_outcome(package.clone(), Ok(wrong_identity));
        assert_eq!(wrong.state, ConversionCrawlOutcomeStateV1::Failed);

        let hot_result = conversion_result(&package, "hot");
        let hot = package_outcome(package, Ok(hot_result));
        assert_eq!(hot.state, ConversionCrawlOutcomeStateV1::Failed);
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
        let outcomes = crawl_packages(vec![first, second], 2, {
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
        })
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
                .all(|outcome| outcome.state == ConversionCrawlOutcomeStateV1::Succeeded)
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
