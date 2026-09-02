// apps/remi/src/server/admin_service/profile_refresh/source_reuse.rs

//! Bounded selection of registered immutable sources for one profile refresh.

use std::collections::BTreeMap;

use anyhow::Context;
use conary_core::db::models::RemiActiveProfileRevision;
use conary_core::repository::{DurableSourceCatalogReuseV1, current_profile_sync_candidate};

use super::RefreshRoots;
use crate::server::catalog_authority::{ProfileRevisionInspection, ProfileRevisionSelection};
use crate::server::catalog_refresh::ProfileSourcePlan;

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
    use crate::server::catalog_capacity::CatalogScratchCoordinator;

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
        let roots = RefreshRoots {
            db_path: fixture.db_path().to_path_buf(),
            keyring_dir: scratch.path().join("keys"),
            catalog_candidate_dir: scratch.path().join("candidates"),
            catalog_dir: fixture.catalog_dir().to_path_buf(),
            projection_cache_dir: scratch.path().join("projections"),
            database_writer: fixture.authority().database_writer_for_test(),
            catalog_authority: fixture.authority().clone(),
            catalog_gc_coordinator: Arc::new(tokio::sync::Mutex::new(())),
            catalog_scratch_coordinator: Arc::new(CatalogScratchCoordinator::default()),
        };
        let plans = current
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
                crate::server::catalog_refresh::ProfileSourcePlan {
                    ordinal: member.ordinal,
                    role: member.role,
                    precedence: member.precedence,
                    required: member.required,
                    repository,
                }
            })
            .collect::<Vec<_>>();

        let reuse = registered_source_reuse(&roots, profile, &plans)
            .await
            .expect("classify obsolete source reuse");
        assert!(reuse.sources.is_empty());
        assert_eq!(
            reuse.decision,
            ReuseDecision::ObsoleteSchema {
                found: 2,
                required: PROFILE_REVISION_SCHEMA_V3,
            }
        );
        assert!(
            super::super::reusable_profile_catalog(&roots, profile, &current.manifest.members)
                .await
                .expect("classify obsolete profile reuse")
                .is_none()
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
        let rebuilt_revision = fixture.register(profile, 2, Vec::new());
        let rebuilt = fixture
            .authority()
            .inspect_selected_profile(&ProfileRevisionSelection {
                source_profile: profile.to_string(),
                profile_revision_sha256: rebuilt_revision,
            })
            .expect("inspect schema-three replacement");
        assert_eq!(rebuilt.manifest.schema_version, PROFILE_REVISION_SCHEMA_V3);
    }
}
