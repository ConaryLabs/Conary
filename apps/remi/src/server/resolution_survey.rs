// apps/remi/src/server/resolution_survey.rs

//! Stopped-runtime orchestration for private resolution diagnostics surveys.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use conary_core::repository::catalog::{
    ConaryResolutionSurveyCountsV1, NativeResolutionComparisonSurveyCountsV1,
    produce_conary_resolution_comparison_survey, produce_conary_resolution_survey,
};

use super::catalog_authority::CatalogAuthority;
use super::promotion_proof::{RemiPromotionProofProfileInput, validate_inputs};

#[derive(Debug, Clone)]
pub struct RemiResolutionSurveyConfig {
    pub output_dir: PathBuf,
    pub profiles: Vec<RemiPromotionProofProfileInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RemiResolutionSurveyOutcome {
    pub output_dir: PathBuf,
    pub profiles: usize,
    pub profile_results: Vec<RemiResolutionSurveyProfileOutcome>,
    pub roots_walked: u64,
    pub candidate_failures: u64,
    pub comparison_mismatches: u64,
    pub comparison_profiles: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RemiResolutionSurveyProfileOutcome {
    pub profile: String,
    pub candidate: RemiResolutionSurveyCandidateOutcome,
    pub comparison: Option<RemiResolutionSurveyComparisonOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RemiResolutionSurveyCandidateOutcome {
    pub counts: ConaryResolutionSurveyCountsV1,
    pub total_failures: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RemiResolutionSurveyComparisonOutcome {
    pub candidate_manifest_sha256: String,
    pub counts: NativeResolutionComparisonSurveyCountsV1,
    pub total_mismatches: u64,
}

pub(crate) fn produce_remi_resolution_surveys(
    config: &RemiResolutionSurveyConfig,
    authority: &CatalogAuthority,
) -> Result<RemiResolutionSurveyOutcome> {
    validate_inputs(&config.profiles)?;
    create_private_output_directory(&config.output_dir)?;

    let mut roots_walked = 0_u64;
    let mut candidate_failures = 0_u64;
    let mut comparison_mismatches = 0_u64;
    let mut comparison_profiles = 0_usize;
    let mut profile_results = Vec::with_capacity(config.profiles.len());
    for input in &config.profiles {
        let pin = authority
            .open_selected_profile_exclusively(&input.selection)
            .with_context(|| {
                format!(
                    "reopen resolution-survey profile '{}' revision {}",
                    input.selection.source_profile, input.selection.profile_revision_sha256
                )
            })?;
        ensure!(
            pin.selection() == &input.selection,
            "resolution-survey catalog selection changed while reopening '{}'",
            input.selection.source_profile
        );
        let candidate_path = config.output_dir.join(format!(
            "{}.candidate-resolution-survey.json",
            input.selection.source_profile
        ));
        let candidate = {
            let reader = pin.reader();
            produce_conary_resolution_survey(
                pin.manifest(),
                &reader,
                &input.package_oracle_dir,
                &input.architecture,
                &candidate_path,
            )
            .with_context(|| {
                format!(
                    "produce Conary resolution survey for '{}'",
                    input.selection.source_profile
                )
            })?
        };
        roots_walked = checked_add(roots_walked, candidate.counts.roots_walked)?;
        candidate_failures = checked_add(candidate_failures, candidate.total_failures)?;
        let candidate_outcome = RemiResolutionSurveyCandidateOutcome {
            counts: candidate.counts.clone(),
            total_failures: candidate.total_failures,
        };
        if candidate.total_failures != 0 {
            profile_results.push(RemiResolutionSurveyProfileOutcome {
                profile: input.selection.source_profile.clone(),
                candidate: candidate_outcome,
                comparison: None,
            });
            continue;
        }

        let comparison_path = config.output_dir.join(format!(
            "{}.native-resolution-comparison-survey.json",
            input.selection.source_profile
        ));
        let comparison = {
            let reader = pin.reader();
            produce_conary_resolution_comparison_survey(
                pin.manifest(),
                &reader,
                &input.package_oracle_dir,
                &input.native_resolution_dir,
                &input.architecture,
                &comparison_path,
            )
            .with_context(|| {
                format!(
                    "produce native resolution comparison survey for '{}'",
                    input.selection.source_profile
                )
            })?
        };
        comparison_mismatches = checked_add(comparison_mismatches, comparison.total_mismatches)?;
        comparison_profiles = comparison_profiles
            .checked_add(1)
            .context("resolution survey profile count exceeds usize")?;
        profile_results.push(RemiResolutionSurveyProfileOutcome {
            profile: input.selection.source_profile.clone(),
            candidate: candidate_outcome,
            comparison: Some(RemiResolutionSurveyComparisonOutcome {
                candidate_manifest_sha256: comparison.candidate_manifest_sha256,
                counts: comparison.counts,
                total_mismatches: comparison.total_mismatches,
            }),
        });
    }

    Ok(RemiResolutionSurveyOutcome {
        output_dir: config.output_dir.clone(),
        profiles: config.profiles.len(),
        profile_results,
        roots_walked,
        candidate_failures,
        comparison_mismatches,
        comparison_profiles,
    })
}

fn create_private_output_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!("resolution survey output {} already exists", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect resolution survey output"),
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("inspect resolution survey parent {}", parent.display()))?;
    ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "resolution survey parent {} must be a real directory",
        parent.display()
    );
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .context("create private resolution survey output")?;
    Ok(())
}

fn checked_add(left: u64, right: u64) -> Result<u64> {
    left.checked_add(right)
        .context("resolution survey aggregate count exceeds u64")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::catalog_authority::ProfileRevisionSelection;
    use crate::server::catalog_authority::test_support::ActiveCatalogFixture;

    fn inputs() -> Vec<RemiPromotionProofProfileInput> {
        vec![
            input("fedora-44", 'a', "x86_64"),
            input("ubuntu-26.04", 'b', "amd64"),
            input("arch", 'c', "x86_64"),
        ]
    }

    fn input(profile: &str, digest: char, architecture: &str) -> RemiPromotionProofProfileInput {
        RemiPromotionProofProfileInput {
            selection: ProfileRevisionSelection {
                source_profile: profile.to_string(),
                profile_revision_sha256: digest.to_string().repeat(64),
            },
            package_oracle_dir: PathBuf::from("package"),
            native_resolution_dir: PathBuf::from("native"),
            architecture: architecture.to_string(),
        }
    }

    #[test]
    fn binding_and_architecture_fail_before_output_creation() {
        let fixture = ActiveCatalogFixture::new();
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("surveys");
        let mut profiles = inputs();
        profiles[0].architecture = "aarch64".to_string();
        let error = produce_remi_resolution_surveys(
            &RemiResolutionSurveyConfig {
                output_dir: output.clone(),
                profiles,
            },
            fixture.authority(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("must match profile authority"));
        assert!(!output.exists());
    }

    #[test]
    fn output_directory_is_private_create_only_and_refuses_overwrite() {
        let fixture = ActiveCatalogFixture::new();
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("surveys");
        let error = produce_remi_resolution_surveys(
            &RemiResolutionSurveyConfig {
                output_dir: output.clone(),
                profiles: inputs(),
            },
            fixture.authority(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("reopen resolution-survey profile")
        );
        assert!(output.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let error = produce_remi_resolution_surveys(
            &RemiResolutionSurveyConfig {
                output_dir: output.clone(),
                profiles: inputs(),
            },
            fixture.authority(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("already exists"));
    }
}
