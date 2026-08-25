// apps/remi/src/server/promotion_proof.rs

//! Atomic operator materialization of candidate resolution and promotion proof.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use conary_core::repository::catalog::{
    NativeResolutionOracleV1, produce_conary_resolution_candidate,
    verify_native_parity_oracle_bundle, verify_native_resolution_oracle_bundle,
};

use super::catalog_authority::{CatalogAuthority, PinnedProfileCatalog, ProfileRevisionSelection};
use super::handlers::canonical::load_canonical_map_snapshot;
use super::promotion_evidence::{
    RemiPromotionEvidenceConfig, RemiPromotionEvidenceV1, RemiPromotionProfileEvidenceInput,
    produce_remi_promotion_evidence, reopen_remi_promotion_evidence,
};

#[derive(Debug, Clone)]
pub struct RemiPromotionProofProfileInput {
    pub selection: ProfileRevisionSelection,
    pub package_oracle_dir: PathBuf,
    pub native_resolution_dir: PathBuf,
    pub architecture: String,
}

#[derive(Debug, Clone)]
pub struct RemiPromotionProofConfig {
    pub db_path: PathBuf,
    pub catalog_dir: PathBuf,
    pub conversion_crawl_path: PathBuf,
    pub output_dir: PathBuf,
    pub profiles: Vec<RemiPromotionProofProfileInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RemiPromotionProofOutcome {
    pub output_dir: PathBuf,
    pub promotion_evidence_path: PathBuf,
    pub promotion_evidence_sha256: String,
    pub profiles: usize,
}

struct ProducedProfile {
    pin: PinnedProfileCatalog,
    package_oracle_dir: PathBuf,
    native_resolution_dir: PathBuf,
    candidate_manifest: NativeResolutionOracleV1,
}

pub(crate) fn produce_remi_promotion_proof(
    config: &RemiPromotionProofConfig,
    authority: &CatalogAuthority,
) -> Result<RemiPromotionProofOutcome> {
    validate_inputs(&config.profiles)?;
    let parent = output_parent(&config.output_dir)?;
    match fs::symlink_metadata(&config.output_dir) {
        Ok(_) => bail!(
            "promotion proof output {} already exists",
            config.output_dir.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect promotion proof output"),
    }

    let staged = tempfile::Builder::new()
        .prefix("remi-promotion-proof-")
        .tempdir_in(&parent)
        .context("create private staged promotion proof directory")?;
    let candidate_root = staged.path().join("candidate-resolution");
    fs::create_dir(&candidate_root).context("create candidate resolution evidence root")?;

    let mut produced = Vec::with_capacity(config.profiles.len());
    let mut evidence_profiles = Vec::with_capacity(config.profiles.len());
    for input in &config.profiles {
        let pin = authority
            .open_selected_profile_exclusively(&input.selection)
            .with_context(|| {
                format!(
                    "reopen promotion-proof profile '{}' revision {}",
                    input.selection.source_profile, input.selection.profile_revision_sha256
                )
            })?;
        ensure!(
            pin.selection() == &input.selection,
            "promotion-proof catalog selection changed while reopening '{}'",
            input.selection.source_profile
        );
        let candidate_dir = candidate_root.join(&input.selection.source_profile);
        let candidate = {
            let reader = pin.reader();
            produce_conary_resolution_candidate(
                pin.manifest(),
                &reader,
                &input.package_oracle_dir,
                &input.native_resolution_dir,
                &input.architecture,
                &candidate_dir,
            )
            .with_context(|| {
                format!(
                    "produce complete Conary candidate resolution for '{}'",
                    input.selection.source_profile
                )
            })?
        };
        evidence_profiles.push(RemiPromotionProfileEvidenceInput {
            revision: pin.manifest().clone(),
            package_oracle_dir: input.package_oracle_dir.clone(),
            native_resolution_dir: input.native_resolution_dir.clone(),
            candidate_resolution_dir: candidate_dir,
        });
        produced.push(ProducedProfile {
            pin,
            package_oracle_dir: input.package_oracle_dir.clone(),
            native_resolution_dir: input.native_resolution_dir.clone(),
            candidate_manifest: candidate.manifest,
        });
    }

    let conn = super::open_runtime_db(&config.db_path)?;
    let canonical_map = load_canonical_map_snapshot(&conn)?;
    drop(conn);
    let staged_evidence_path = staged.path().join("promotion.json");
    let evidence = produce_remi_promotion_evidence(
        &RemiPromotionEvidenceConfig {
            catalog_dir: config.catalog_dir.clone(),
            conversion_crawl_path: config.conversion_crawl_path.clone(),
            output_path: staged_evidence_path,
            profiles: evidence_profiles,
        },
        &canonical_map,
    )?;

    File::open(&candidate_root)?.sync_all()?;
    File::open(staged.path())?.sync_all()?;
    let staged_path = staged.path().to_path_buf();
    fs::rename(&staged_path, &config.output_dir).with_context(|| {
        format!(
            "publish complete promotion proof directory {}",
            config.output_dir.display()
        )
    })?;
    File::open(&parent)?.sync_all()?;
    drop(staged);

    reopen_published_outputs(&config.output_dir, &produced, &evidence)?;
    let evidence_path = config.output_dir.join("promotion.json");
    let evidence_bytes = fs::read(&evidence_path).context("reopen published promotion evidence")?;
    Ok(RemiPromotionProofOutcome {
        output_dir: config.output_dir.clone(),
        promotion_evidence_path: evidence_path,
        promotion_evidence_sha256: conary_core::hash::sha256(&evidence_bytes),
        profiles: produced.len(),
    })
}

fn validate_inputs(inputs: &[RemiPromotionProofProfileInput]) -> Result<()> {
    let public = conary_core::repository::supported_profiles::public_profiles();
    ensure!(
        inputs.len() == public.len(),
        "promotion proof requires exactly {} ordered public profiles",
        public.len()
    );
    for (input, expected) in inputs.iter().zip(public) {
        ensure!(
            input.selection.source_profile == expected.id(),
            "promotion-proof profile '{}' must be '{}' at this ordinal",
            input.selection.source_profile,
            expected.id()
        );
        validate_sha256(&input.selection.profile_revision_sha256)?;
        ensure!(
            !input.architecture.is_empty()
                && input
                    .architecture
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')),
            "promotion-proof architecture for '{}' is invalid",
            expected.id()
        );
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            && value == value.to_ascii_lowercase(),
        "promotion-proof revision must be an exact lowercase SHA-256 digest"
    );
    Ok(())
}

fn output_parent(output: &Path) -> Result<PathBuf> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("inspect promotion proof parent {}", parent.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!(
            "promotion proof parent {} must be a real directory",
            parent.display()
        );
    }
    Ok(parent.to_path_buf())
}

fn reopen_published_outputs(
    output: &Path,
    profiles: &[ProducedProfile],
    expected_evidence: &RemiPromotionEvidenceV1,
) -> Result<()> {
    let reopened = reopen_remi_promotion_evidence(&output.join("promotion.json"))?;
    ensure!(
        &reopened == expected_evidence,
        "published promotion evidence differs from its produced value"
    );
    for profile in profiles {
        let package = verify_native_parity_oracle_bundle(
            &profile.package_oracle_dir,
            profile.pin.manifest(),
        )?;
        let native = verify_native_resolution_oracle_bundle(
            &profile.native_resolution_dir,
            profile.pin.manifest(),
            &package,
        )?;
        let candidate = verify_native_resolution_oracle_bundle(
            output
                .join("candidate-resolution")
                .join(profile.pin.source_profile()),
            profile.pin.manifest(),
            &package,
        )?;
        ensure!(
            candidate.manifest() == &profile.candidate_manifest,
            "published candidate resolution for '{}' differs from its produced value",
            profile.pin.source_profile()
        );
        conary_core::repository::catalog::compare_native_resolution_oracle(
            profile.pin.manifest(),
            &package,
            &native,
            &candidate,
        )
        .map_err(anyhow::Error::from)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use conary_core::repository::catalog::ProfileRevisionV2;

    use super::*;
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use crate::server::conversion_crawl::{
        ConversionCrawlProfileV4, REMI_CONVERSION_CRAWL_SCHEMA_V4, RemiConversionCrawlV4,
        write_and_reopen_conversion_crawl,
    };
    use crate::server::promotion_evidence::tests::{
        architecture, outcome, write_package_oracle, write_resolution,
    };

    fn input(profile: &str, revision: &str, architecture: &str) -> RemiPromotionProofProfileInput {
        RemiPromotionProofProfileInput {
            selection: ProfileRevisionSelection {
                source_profile: profile.to_string(),
                profile_revision_sha256: revision.to_string(),
            },
            package_oracle_dir: PathBuf::from("package"),
            native_resolution_dir: PathBuf::from("resolution"),
            architecture: architecture.to_string(),
        }
    }

    fn valid_inputs() -> Vec<RemiPromotionProofProfileInput> {
        vec![
            input("fedora-44", &"a".repeat(64), "x86_64"),
            input("ubuntu-26.04", &"b".repeat(64), "amd64"),
            input("arch", &"c".repeat(64), "x86_64"),
        ]
    }

    #[test]
    fn ordered_public_promotion_proof_bindings_are_exact() {
        validate_inputs(&valid_inputs()).unwrap();
        let mut reordered = valid_inputs();
        reordered.swap(0, 1);
        assert!(validate_inputs(&reordered).is_err());
        let mut candidate_tier = valid_inputs();
        candidate_tier[2].selection.source_profile = "solus".to_string();
        assert!(validate_inputs(&candidate_tier).is_err());
    }

    #[test]
    fn promotion_proof_rejects_noncanonical_revision_and_architecture() {
        let mut inputs = valid_inputs();
        inputs[0].selection.profile_revision_sha256 = "A".repeat(64);
        assert!(validate_inputs(&inputs).is_err());
        let mut inputs = valid_inputs();
        inputs[0].architecture = "x86 64".to_string();
        assert!(validate_inputs(&inputs).is_err());
    }

    #[test]
    fn failed_operator_proof_removes_staged_partial_output() {
        let catalogs = ActiveCatalogFixture::new();
        let output_parent = tempfile::tempdir().expect("promotion proof output parent");
        let output = output_parent.path().join("proof");

        let error = produce_remi_promotion_proof(
            &RemiPromotionProofConfig {
                db_path: catalogs.db_path().to_path_buf(),
                catalog_dir: catalogs.catalog_dir().to_path_buf(),
                conversion_crawl_path: output_parent.path().join("missing-crawl.json"),
                output_dir: output.clone(),
                profiles: valid_inputs(),
            },
            catalogs.authority(),
        )
        .expect_err("unregistered candidate revision must fail");

        assert!(error.to_string().contains("reopen promotion-proof profile"));
        assert!(!output.exists());
        assert_eq!(
            fs::read_dir(output_parent.path())
                .expect("inspect output parent")
                .count(),
            0,
            "failed proof must remove its private staged directory"
        );
    }

    struct OracleFixture {
        input: RemiPromotionProofProfileInput,
        revision: ProfileRevisionV2,
        packages: Vec<conary_core::repository::catalog::CatalogPackageRecordV1>,
        _package: tempfile::TempDir,
        _native: tempfile::TempDir,
    }

    #[test]
    fn complete_operator_proof_is_atomic_and_reopened() {
        let catalogs = ActiveCatalogFixture::new();
        let mut fixtures = Vec::new();
        for (index, profile) in conary_core::repository::supported_profiles::public_profiles()
            .iter()
            .enumerate()
        {
            let mut row = package(
                profile.id(),
                "demo",
                "1.0",
                "1",
                Some(architecture(profile.id())),
                42,
                profile.id(),
            );
            row.checksum = format!(
                "sha256:{}",
                conary_core::hash::sha256(profile.id().as_bytes())
            );
            let revision_sha256 =
                catalogs.activate(profile.id(), i64::try_from(index + 1).unwrap(), vec![row]);
            let pin = catalogs
                .authority()
                .open_selected_profile_exclusively(&ProfileRevisionSelection {
                    source_profile: profile.id().to_string(),
                    profile_revision_sha256: revision_sha256.clone(),
                })
                .expect("open fixture promotion profile");
            let revision = pin.manifest().clone();
            let packages = pin.reader().packages().expect("read fixture packages");
            let package_oracle = write_package_oracle(&revision, &packages);
            let native_resolution = write_resolution(
                &revision,
                package_oracle.path(),
                &packages,
                "native-fixture",
            );
            fixtures.push(OracleFixture {
                input: RemiPromotionProofProfileInput {
                    selection: ProfileRevisionSelection {
                        source_profile: profile.id().to_string(),
                        profile_revision_sha256: revision_sha256,
                    },
                    package_oracle_dir: package_oracle.path().to_path_buf(),
                    native_resolution_dir: native_resolution.path().to_path_buf(),
                    architecture: architecture(profile.id()).to_string(),
                },
                revision,
                packages,
                _package: package_oracle,
                _native: native_resolution,
            });
        }

        let output_parent = tempfile::tempdir().expect("promotion proof output parent");
        let crawl_path = output_parent.path().join("crawl.json");
        write_and_reopen_conversion_crawl(
            &crawl_path,
            &RemiConversionCrawlV4 {
                schema_version: REMI_CONVERSION_CRAWL_SCHEMA_V4,
                profiles: fixtures
                    .iter()
                    .map(|fixture| ConversionCrawlProfileV4 {
                        profile: fixture.revision.profile.clone(),
                        profile_revision_sha256: fixture
                            .revision
                            .manifest_sha256()
                            .expect("fixture revision digest"),
                        expected_packages: u64::try_from(fixture.packages.len()).unwrap(),
                        outcomes: fixture
                            .packages
                            .iter()
                            .map(|package| {
                                outcome(
                                    package,
                                    &fixture
                                        .revision
                                        .manifest_sha256()
                                        .expect("fixture revision digest"),
                                )
                            })
                            .collect(),
                    })
                    .collect(),
            },
        )
        .expect("write complete fixture crawl");
        let output = output_parent.path().join("proof");
        let result = produce_remi_promotion_proof(
            &RemiPromotionProofConfig {
                db_path: catalogs.db_path().to_path_buf(),
                catalog_dir: catalogs.catalog_dir().to_path_buf(),
                conversion_crawl_path: crawl_path,
                output_dir: output.clone(),
                profiles: fixtures
                    .iter()
                    .map(|fixture| fixture.input.clone())
                    .collect(),
            },
            catalogs.authority(),
        )
        .expect("produce complete operator proof");

        assert_eq!(result.profiles, 3);
        assert_eq!(result.output_dir, output);
        assert_eq!(
            reopen_remi_promotion_evidence(&result.promotion_evidence_path)
                .expect("reopen published operator evidence")
                .profiles
                .len(),
            3
        );
        for profile in conary_core::repository::supported_profiles::public_profiles() {
            assert!(
                result
                    .output_dir
                    .join("candidate-resolution")
                    .join(profile.id())
                    .is_dir()
            );
        }
    }
}
