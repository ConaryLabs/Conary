// apps/remi/src/deployment/candidate_inspection.rs

//! Full and causally publication-attested private-candidate inspection.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::time::Instant;

use super::{DeploymentProfileRefreshState, refresh_diagnostics};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentCandidateState {
    pub profile: String,
    pub configured_sources: usize,
    pub profile_revision_sha256: Option<String>,
    pub run_id: Option<String>,
    pub completed_at: Option<i64>,
    pub packages: u64,
    pub latest_refresh: Option<DeploymentProfileRefreshState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentCandidateVerificationMode {
    FullReopen,
    PublicationAttested,
}

/// Exact work and authority used to validate private profile candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentCandidateVerification {
    pub mode: DeploymentCandidateVerificationMode,
    pub completed_after: Option<i64>,
    pub elapsed_micros: u64,
    pub catalog_files_reopened: u64,
    pub catalog_bytes_hashed: u64,
    pub catalog_bytes_integrity_checked: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum CandidateInspectionMode {
    FullReopen,
    PublicationAttested { completed_after: i64 },
}

pub(super) fn inspect_deployment_candidates(
    conn: &rusqlite::Connection,
    authority: &crate::server::catalog_authority::CatalogAuthority,
    configured: &[(String, usize)],
    verification: CandidateInspectionMode,
) -> Result<(
    Vec<DeploymentCandidateState>,
    DeploymentCandidateVerification,
)> {
    let started = Instant::now();
    let mut catalog_files_reopened = 0_u64;
    let mut catalog_bytes_hashed = 0_u64;
    let mut catalog_bytes_integrity_checked = 0_u64;
    let mut candidates = Vec::with_capacity(configured.len());
    for (profile, configured_sources) in configured {
        let latest_refresh = refresh_diagnostics::latest_profile_refresh(conn, profile)?;
        let candidate = conary_core::repository::current_profile_sync_candidate(conn, profile)?;
        let Some(candidate) = candidate else {
            candidates.push(DeploymentCandidateState {
                profile: profile.clone(),
                configured_sources: *configured_sources,
                profile_revision_sha256: None,
                run_id: None,
                completed_at: None,
                packages: 0,
                latest_refresh,
            });
            continue;
        };
        let selection = crate::server::catalog_authority::ProfileRevisionSelection {
            source_profile: candidate.source_profile.clone(),
            profile_revision_sha256: candidate.profile_revision_sha256.clone(),
        };
        let inspection = match verification {
            CandidateInspectionMode::FullReopen => {
                let inspection = authority
                    .verify_selected_profile(&selection)
                    .with_context(|| format!("inspect private immutable profile '{profile}'"))?;
                catalog_files_reopened = catalog_files_reopened
                    .checked_add(1)
                    .context("deployment catalog reopen count overflow")?;
                catalog_bytes_hashed = catalog_bytes_hashed
                    .checked_add(inspection.manifest.catalog.size)
                    .context("deployment catalog hash-byte count overflow")?;
                catalog_bytes_integrity_checked = catalog_bytes_integrity_checked
                    .checked_add(inspection.manifest.catalog.size)
                    .context("deployment catalog integrity-byte count overflow")?;
                inspection
            }
            CandidateInspectionMode::PublicationAttested { .. } => authority
                .inspect_selected_profile(&selection)
                .with_context(|| {
                    format!("inspect publication-attested private profile '{profile}'")
                })?,
        };
        inspection
            .manifest
            .validate_member_contract()
            .with_context(|| format!("validate private profile '{profile}' member contract"))?;
        if inspection.manifest.members.len() != *configured_sources {
            bail!(
                "private profile '{profile}' does not match its exact configured source authority"
            );
        }
        conary_core::db::models::verify_private_profile_candidate_authority(
            conn,
            profile,
            &candidate.profile_revision_sha256,
            &candidate.run_id,
        )
        .with_context(|| format!("verify private profile '{profile}' repository authority"))?;
        if conary_core::repository::current_profile_sync_candidate(conn, profile)?.as_ref()
            != Some(&candidate)
        {
            bail!("private profile '{profile}' changed during deployment inspection");
        }
        candidates.push(DeploymentCandidateState {
            profile: profile.clone(),
            configured_sources: *configured_sources,
            profile_revision_sha256: Some(candidate.profile_revision_sha256),
            run_id: Some(candidate.run_id),
            completed_at: Some(candidate.completed_at),
            packages: inspection.manifest.counts.packages,
            latest_refresh,
        });
    }
    let (mode, completed_after) = match verification {
        CandidateInspectionMode::FullReopen => {
            (DeploymentCandidateVerificationMode::FullReopen, None)
        }
        CandidateInspectionMode::PublicationAttested { completed_after } => (
            DeploymentCandidateVerificationMode::PublicationAttested,
            Some(completed_after),
        ),
    };
    Ok((
        candidates,
        DeploymentCandidateVerification {
            mode,
            completed_after,
            elapsed_micros: u64::try_from(started.elapsed().as_micros())
                .context("deployment candidate inspection duration overflow")?,
            catalog_files_reopened,
            catalog_bytes_hashed,
            catalog_bytes_integrity_checked,
        },
    ))
}
