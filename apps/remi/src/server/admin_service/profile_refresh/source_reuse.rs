// apps/remi/src/server/admin_service/profile_refresh/source_reuse.rs

//! Bounded selection of registered immutable sources for one profile refresh.

use std::collections::BTreeMap;

use anyhow::Context;
use conary_core::db::models::RemiActiveProfileRevision;
use conary_core::repository::{DurableSourceCatalogReuseV1, current_profile_sync_candidate};

use super::RefreshRoots;
use crate::server::catalog_authority::ProfileRevisionSelection;
use crate::server::catalog_refresh::ProfileSourcePlan;

pub(super) async fn profile_reuse_selections(
    roots: &RefreshRoots,
    source_profile: &str,
) -> anyhow::Result<Vec<ProfileRevisionSelection>> {
    let db_path = roots.db_path.clone();
    let profile_for_query = source_profile.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        let mut selections = Vec::new();
        if let Some(candidate) = current_profile_sync_candidate(&conn, &profile_for_query)? {
            selections.push(ProfileRevisionSelection {
                source_profile: candidate.source_profile,
                profile_revision_sha256: candidate.profile_revision_sha256,
            });
        }
        if let Some(active) = RemiActiveProfileRevision::find(&conn, &profile_for_query)? {
            let selection = ProfileRevisionSelection::from(&active);
            if !selections.contains(&selection) {
                selections.push(selection);
            }
        }
        Ok::<_, conary_core::Error>(selections)
    })
    .await
    .context("reusable profile selection task panicked")?
    .map_err(anyhow::Error::from)
}

pub(super) async fn registered_source_reuse(
    roots: &RefreshRoots,
    source_profile: &str,
    plans: &[ProfileSourcePlan],
) -> anyhow::Result<BTreeMap<String, DurableSourceCatalogReuseV1>> {
    for selection in profile_reuse_selections(roots, source_profile).await? {
        let authority = roots.catalog_authority.clone();
        let inspected_selection = selection.clone();
        let inspection = tokio::task::spawn_blocking(move || {
            authority.inspect_selected_profile(&inspected_selection)
        })
        .await
        .context("source-reuse profile inspection task panicked")??;
        if !super::profile_members_match_plan(&inspection.manifest.members, plans) {
            continue;
        }

        let authority = roots.catalog_authority.clone();
        let source_selection = selection.clone();
        let reusable = tokio::task::spawn_blocking(move || {
            authority.inspect_source_reuse_for_selection(&source_selection)
        })
        .await
        .context("registered source selection task panicked")??;
        if reusable.len() != plans.len() {
            anyhow::bail!(
                "profile '{}' revision {} resolved {} registered sources for {} planned members",
                source_profile,
                selection.profile_revision_sha256,
                reusable.len(),
                plans.len()
            );
        }

        let mut by_repository = BTreeMap::new();
        for (ordinal, source) in reusable {
            let plan = plans
                .iter()
                .find(|plan| plan.ordinal == ordinal)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "profile '{}' reusable source ordinal {} is not planned",
                        source_profile,
                        ordinal
                    )
                })?;
            let repository_identity = plan
                .repository
                .repository_identity
                .as_ref()
                .expect("planned repository identity")
                .clone();
            if source.manifest().repository_identity != repository_identity {
                anyhow::bail!(
                    "profile '{}' reusable source ordinal {} changed repository identity",
                    source_profile,
                    ordinal
                );
            }
            if by_repository
                .insert(repository_identity.clone(), source)
                .is_some()
            {
                anyhow::bail!(
                    "profile '{}' repeats reusable repository identity '{}'",
                    source_profile,
                    repository_identity
                );
            }
        }
        tracing::info!(
            source_profile,
            profile_revision_sha256 = %selection.profile_revision_sha256,
            reusable_sources = by_repository.len(),
            "selected registered durable source candidates"
        );
        return Ok(by_repository);
    }

    Ok(BTreeMap::new())
}
