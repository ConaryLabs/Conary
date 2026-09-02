// apps/remi/src/server/admin_service/profile_refresh/source_reuse.rs

//! Bounded selection of registered immutable sources for one profile refresh.

use std::collections::BTreeMap;

use anyhow::Context;
use conary_core::db::models::RemiActiveProfileRevision;
use conary_core::repository::catalog::{ProfileSourceMemberV2, SourceStreamKindV1};
use conary_core::repository::{DurableSourceCatalogReuseV1, current_profile_sync_candidate};

use super::RefreshRoots;
use crate::server::catalog_authority::{
    CatalogAuthority, PinnedProfileCatalog, ProfileRevisionInspection, ProfileRevisionSelection,
};
use crate::server::catalog_refresh::{ProfileSourcePlan, profile_revision_matches_contract};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReuseDecision {
    NoReusableRevision,
    Reused {
        profile_revision_sha256: String,
        sources: usize,
    },
    ObsoleteSchema {
        found: u32,
        required: u32,
    },
}

pub(super) struct RegisteredSourceReuse {
    pub(super) sources: BTreeMap<String, DurableSourceCatalogReuseV1>,
    pub(super) decision: ReuseDecision,
}

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
) -> anyhow::Result<RegisteredSourceReuse> {
    let mut obsolete = None;
    for selection in profile_reuse_selections(roots, source_profile).await? {
        let authority = roots.catalog_authority.clone();
        let inspected_selection = selection.clone();
        let inspection = tokio::task::spawn_blocking(move || {
            authority.inspect_selected_profile_for_upgrade(&inspected_selection)
        })
        .await
        .context("source-reuse profile inspection task panicked")??;
        let inspection = match inspection {
            ProfileRevisionInspection::Current(inspection) => inspection,
            ProfileRevisionInspection::ObsoleteSchema { found, required } => {
                obsolete.get_or_insert(ReuseDecision::ObsoleteSchema { found, required });
                continue;
            }
        };
        if !profile_members_match_plan(&inspection.manifest.members, plans) {
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
        let sources = by_repository.len();
        return Ok(RegisteredSourceReuse {
            sources: by_repository,
            decision: ReuseDecision::Reused {
                profile_revision_sha256: selection.profile_revision_sha256,
                sources,
            },
        });
    }

    Ok(RegisteredSourceReuse {
        sources: BTreeMap::new(),
        decision: obsolete.unwrap_or(ReuseDecision::NoReusableRevision),
    })
}

pub(super) async fn reusable_profile_catalog(
    roots: &RefreshRoots,
    source_profile: &str,
    members: &[ProfileSourceMemberV2],
) -> anyhow::Result<Option<PinnedProfileCatalog>> {
    for selection in profile_reuse_selections(roots, source_profile).await? {
        let authority = roots.catalog_authority.clone();
        let inspected_selection = selection.clone();
        let inspection = tokio::task::spawn_blocking(move || {
            authority.inspect_selected_profile_for_upgrade(&inspected_selection)
        })
        .await
        .context("reusable profile inspection task panicked")??;
        let inspection = match inspection {
            ProfileRevisionInspection::Current(inspection) => inspection,
            ProfileRevisionInspection::ObsoleteSchema { found, required } => {
                tracing::info!(
                    source_profile,
                    profile_revision_sha256 = %selection.profile_revision_sha256,
                    found,
                    required,
                    reuse_decision = "obsolete_schema",
                    "profile refresh rejected an obsolete profile catalog for reuse"
                );
                continue;
            }
        };
        if !profile_revision_matches_contract(&inspection.manifest, source_profile, members) {
            continue;
        }

        let authority = roots.catalog_authority.clone();
        let opened_selection = selection.clone();
        let reusable =
            tokio::task::spawn_blocking(move || authority.open_selected_profile(&opened_selection))
                .await
                .context("reusable profile reopen task panicked")??;
        if !profile_revision_matches_contract(reusable.manifest(), source_profile, members) {
            anyhow::bail!(
                "profile '{}' revision {} changed its exact member contract while reopening",
                source_profile,
                selection.profile_revision_sha256
            );
        }
        tracing::info!(
            source_profile,
            profile_revision_sha256 = %selection.profile_revision_sha256,
            "reusing exact durable immutable profile catalog"
        );
        return Ok(Some(reusable));
    }

    Ok(None)
}

pub(super) async fn current_catalog_matches_plan(
    roots: &RefreshRoots,
    source_profile: &str,
    plans: &[ProfileSourcePlan],
) -> bool {
    let authority = CatalogAuthority::from_paths(
        &roots.db_path,
        &roots.catalog_dir,
        roots.database_writer.clone(),
    );
    let db_path = roots.db_path.clone();
    let source_profile = source_profile.to_string();
    let plans = plans.to_vec();
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        if let Some(candidate) = current_profile_sync_candidate(&conn, &source_profile)? {
            let selection = ProfileRevisionSelection {
                source_profile: candidate.source_profile,
                profile_revision_sha256: candidate.profile_revision_sha256,
            };
            return authority
                .open_selected_profile(&selection)
                .map(|catalog| profile_members_match_plan(&catalog.manifest().members, &plans));
        }
        authority
            .inspect_active_profile_for_upgrade(&source_profile)
            .map(|inspection| match inspection {
                ProfileRevisionInspection::Current(catalog) => {
                    profile_members_match_plan(&catalog.manifest.members, &plans)
                }
                ProfileRevisionInspection::ObsoleteSchema { .. } => false,
            })
    })
    .await
    .is_ok_and(|result| result.unwrap_or(false))
}

pub(super) fn profile_members_match_plan(
    members: &[ProfileSourceMemberV2],
    plans: &[ProfileSourcePlan],
) -> bool {
    members.len() == plans.len()
        && members.iter().zip(plans).all(|(member, plan)| {
            let Ok(policy) = plan.repository.require_source_policy() else {
                return false;
            };
            let Some(repository_identity) = plan.repository.repository_identity.as_deref() else {
                return false;
            };
            let stream_kind = match member.stream.kind {
                SourceStreamKindV1::Release => "release",
                SourceStreamKindV1::Channel => "channel",
                SourceStreamKindV1::Rolling => "rolling",
            };
            member.ordinal == plan.ordinal
                && member.source_identity == policy.source_identity
                && member.repository_identity == repository_identity
                && stream_kind == policy.stream.kind()
                && member.stream.identity == policy.stream.identity()
                && member.role == plan.role
                && member.precedence == plan.precedence
                && member.required == plan.required
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use conary_core::db::models::{
        NativeSourceEcosystem, NativeSourceStream, Repository, RepositoryPolicyScope,
        RepositorySourcePolicy, RepositoryUpdateMode,
    };
    use conary_core::repository::catalog::PROFILE_REVISION_SCHEMA_V3;
    use conary_core::repository::{
        OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
    };

    use super::*;
    use crate::server::catalog_authority::{
        ProfileRevisionInspection, test_support::ActiveCatalogFixture,
    };

    #[tokio::test]
    async fn obsolete_revision_is_recorded_non_reusable_before_schema_three_rebuild() {
        let fixture = ActiveCatalogFixture::new();
        let profile = "fedora-44";
        let current_revision = fixture.activate(profile, 1, Vec::new());
        let current_selection = ProfileRevisionSelection {
            source_profile: profile.to_string(),
            profile_revision_sha256: current_revision.clone(),
        };
        let current = fixture
            .authority()
            .inspect_selected_profile(&current_selection)
            .expect("inspect current fixture revision");
        let obsolete_revision = fixture.replace_with_obsolete_schema(&current_revision);
        let scratch = tempfile::tempdir().expect("create refresh scratch root");
        let mut repositories = current
            .manifest
            .members
            .iter()
            .map(|member| {
                let mut repository = Repository::new(
                    format!("fixture-{}", member.repository_identity),
                    "https://example.invalid/metadata".to_string(),
                );
                repository.source_profile = Some(profile.to_string());
                repository.priority = member.precedence;
                repository.profile_member_role = Some(member.role);
                repository.profile_member_required = member.required;
                repository
                    .set_parser_config(RepositoryParserConfig::Rpm {
                        architecture: "x86_64".to_string(),
                    })
                    .expect("bind parser configuration");
                repository
                    .set_trust_policy(RepositoryTrustPolicy::Rpm {
                        metadata: RpmMetadataAuthority::Metalink {
                            url: "https://example.invalid/metalink".to_string(),
                        },
                        package_keys: vec![
                            OpenPgpTrustRoot::new(
                                "https://example.invalid/key".to_string(),
                                "A".repeat(40),
                            )
                            .expect("valid trust root"),
                        ],
                    })
                    .expect("bind trust policy");
                repository
                    .set_native_source_policy(
                        RepositorySourcePolicy::new(
                            member.source_identity.clone(),
                            RepositoryPolicyScope::repository(&member.repository_identity)
                                .expect("valid repository identity"),
                            NativeSourceEcosystem::Rpm,
                            NativeSourceStream::release(&member.stream.identity)
                                .expect("valid release stream"),
                            RepositoryUpdateMode::Follow,
                        )
                        .expect("valid source policy"),
                        member.repository_identity.clone(),
                        None,
                    )
                    .expect("bind source policy");
                repository.id = Some(
                    fixture
                        .connection()
                        .query_row(
                            "SELECT id FROM repositories WHERE repository_identity = ?1",
                            [&member.repository_identity],
                            |row| row.get(0),
                        )
                        .expect("resolve fixture repository ID"),
                );
                repository
            })
            .collect::<Vec<_>>();
        let chunk_dir = scratch.path().join("chunks");
        let cache_dir = scratch.path().join("cache");
        let catalog_candidate_dir = scratch.path().join("candidates");
        for directory in [&chunk_dir, &cache_dir, &catalog_candidate_dir] {
            std::fs::create_dir_all(directory).expect("create refresh directory");
        }
        let state = Arc::new(tokio::sync::RwLock::new(
            crate::server::ServerState::new(crate::server::ServerConfig {
                db_path: fixture.db_path().to_path_buf(),
                chunk_dir,
                cache_dir,
                catalog_dir: fixture.catalog_dir().to_path_buf(),
                catalog_candidate_dir,
                ..Default::default()
            })
            .expect("build refresh server state"),
        ));
        let current_pin = state
            .read()
            .await
            .catalog_authority
            .open_selected_profile(&current_selection)
            .expect("pin current source fixture through refresh garbage collection");

        let (results, decision) = super::super::refresh_native_profile_inner(
            &state,
            profile.to_string(),
            std::mem::take(&mut repositories),
            true,
            Some(current_selection),
        )
        .await
        .expect("refresh obsolete profile revision");
        drop(current_pin);
        assert_eq!(results.len(), current.manifest.members.len());
        assert_eq!(
            decision,
            ReuseDecision::ObsoleteSchema {
                found: 2,
                required: PROFILE_REVISION_SCHEMA_V3,
            }
        );

        let obsolete_selection = ProfileRevisionSelection {
            source_profile: profile.to_string(),
            profile_revision_sha256: obsolete_revision,
        };
        assert!(matches!(
            fixture
                .authority()
                .inspect_selected_profile_for_upgrade(&obsolete_selection)
                .expect("inspect obsolete revision"),
            ProfileRevisionInspection::ObsoleteSchema {
                found: 2,
                required: PROFILE_REVISION_SCHEMA_V3,
            }
        ));
        let candidate =
            conary_core::repository::current_profile_sync_candidate(&fixture.connection(), profile)
                .expect("read refreshed profile candidate")
                .expect("refresh produced a profile candidate");
        let rebuilt = fixture
            .authority()
            .inspect_selected_profile(&ProfileRevisionSelection {
                source_profile: profile.to_string(),
                profile_revision_sha256: candidate.profile_revision_sha256,
            })
            .expect("inspect schema-three replacement");
        assert_eq!(rebuilt.manifest.schema_version, PROFILE_REVISION_SCHEMA_V3);
    }
}
