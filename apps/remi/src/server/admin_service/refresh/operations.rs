// apps/remi/src/server/admin_service/refresh/operations.rs

//! Single-source compatibility and profile-scoped refresh orchestration.

use std::path::PathBuf;
use std::sync::Arc;

use conary_core::db::models::{RemiActiveProfileRevision, Repository};
use futures::StreamExt;
use tokio::sync::RwLock;

use super::{
    RepoRefreshBatch, RepoRefreshBatchState, RepoRefreshFailure, RepoRefreshOutcome,
    RepoRefreshResult, collect_refresh_outcomes,
};
use crate::server::ServerState;
use crate::server::database_writer::DatabaseWriter;

use super::super::{ServiceError, blocking_anyhow, db_path, profile_refresh, publication};

enum RepoRefreshPlan {
    Missing,
    Skipped(RepoRefreshResult),
    Sync(Box<Repository>),
    NativeProfile {
        requested_name: String,
        source_profile: String,
        repositories: Vec<Repository>,
    },
}

async fn refresh_loaded_repo(
    db: PathBuf,
    repo: Repository,
    database_writer: DatabaseWriter,
) -> Result<RepoRefreshResult, ServiceError> {
    let name = repo.name.clone();
    let source_profile = repo.source_profile.clone();
    let packages_synced =
        conary_core::repository::sync_repository_from_db_path(db, repo, database_writer)
            .await
            .map_err(ServiceError::from)?;

    Ok(RepoRefreshResult {
        name,
        source_profile,
        packages_synced,
        skipped: false,
    })
}

/// Synchronize a single repository by name.
///
/// Native members refresh their complete containing profile. Returns
/// `Ok(None)` only when the named repository does not exist.
pub async fn sync_repo(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
    force: bool,
) -> Result<Option<RepoRefreshResult>, ServiceError> {
    let _publication_guard = publication::guard(state).await;
    let result = sync_repo_uncoordinated(state, name, force).await;
    publication::record_single_repository_outcome(state, &result).await;
    result
}

async fn sync_repo_uncoordinated(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
    force: bool,
) -> Result<Option<RepoRefreshResult>, ServiceError> {
    let db = db_path(state).await;
    let database_writer = state.read().await.database_writer.clone();
    let name_owned = name.to_string();
    let db_for_lookup = db.clone();
    let plan = blocking_anyhow(move || {
        let conn = conary_core::db::open_fast(&db).map_err(|e| anyhow::anyhow!("{e}"))?;
        let repo = match Repository::find_by_name(&conn, &name_owned)
            .map_err(|e| anyhow::anyhow!("{e}"))?
        {
            Some(repo) => repo,
            None => return Ok(RepoRefreshPlan::Missing),
        };

        if profile_refresh::is_native_profile_repository(&repo) {
            let source_profile = repo.source_profile.clone().ok_or_else(|| {
                anyhow::anyhow!("native repository '{}' has no source profile", repo.name)
            })?;
            let repositories = Repository::list_enabled(&conn)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .into_iter()
                .filter(|candidate| {
                    candidate.source_profile.as_deref() == Some(source_profile.as_str())
                        && profile_refresh::is_native_profile_repository(candidate)
                })
                .collect();
            return Ok(RepoRefreshPlan::NativeProfile {
                requested_name: repo.name,
                source_profile,
                repositories,
            });
        }

        if !force && !conary_core::repository::needs_sync(&repo) {
            return Ok(RepoRefreshPlan::Skipped(RepoRefreshResult {
                name: repo.name,
                source_profile: repo.source_profile,
                packages_synced: 0,
                skipped: true,
            }));
        }

        Ok(RepoRefreshPlan::Sync(Box::new(repo)))
    })
    .await?;

    match plan {
        RepoRefreshPlan::Missing => Ok(None),
        RepoRefreshPlan::Skipped(result) => Ok(Some(result)),
        RepoRefreshPlan::Sync(repo) => refresh_loaded_repo(db_for_lookup, *repo, database_writer)
            .await
            .map(Some),
        RepoRefreshPlan::NativeProfile {
            requested_name,
            source_profile,
            repositories,
        } => {
            if repositories.is_empty() {
                deactivate_profiles_without_enabled_members(
                    db_for_lookup,
                    database_writer,
                    vec![source_profile.clone()],
                )
                .await?;
                return Ok(Some(RepoRefreshResult {
                    name: requested_name,
                    source_profile: Some(source_profile),
                    packages_synced: 0,
                    skipped: true,
                }));
            }
            let results = profile_refresh::refresh_native_profile(
                state,
                source_profile.clone(),
                repositories,
                force,
            )
            .await?;
            Ok(Some(
                results
                    .into_iter()
                    .find(|result| result.name == requested_name)
                    .unwrap_or(RepoRefreshResult {
                        name: requested_name,
                        source_profile: Some(source_profile),
                        packages_synced: 0,
                        skipped: true,
                    }),
            ))
        }
    }
}

/// Synchronize all enabled repositories.
pub async fn refresh_repositories(
    state: &Arc<RwLock<ServerState>>,
    force: bool,
) -> Result<RepoRefreshBatch, ServiceError> {
    let _publication_guard = publication::guard(state).await;
    let result = refresh_repositories_uncoordinated(state, force).await;
    record_repository_readiness(state, &result).await;
    result
}

/// Synchronize only one exact configured native source profile.
///
/// This is the retry boundary for a partial all-profile ceremony. It never
/// refreshes legacy repositories, deactivates another profile, or upgrades the
/// global publication-readiness phase from a profile-local result.
pub async fn refresh_profile_repositories(
    state: &Arc<RwLock<ServerState>>,
    source_profile: &str,
    force: bool,
) -> Result<RepoRefreshBatch, ServiceError> {
    let _publication_guard = publication::guard(state).await;
    refresh_repositories_scoped_uncoordinated(state, force, Some(source_profile)).await
}

async fn record_repository_readiness(
    state: &Arc<RwLock<ServerState>>,
    result: &Result<RepoRefreshBatch, ServiceError>,
) {
    let outcome = match result.as_ref().map(RepoRefreshBatch::state) {
        Ok(RepoRefreshBatchState::Complete) => {
            crate::server::readiness::PublicationPhaseState::Complete
        }
        Ok(RepoRefreshBatchState::Partial) => {
            crate::server::readiness::PublicationPhaseState::Partial
        }
        Ok(RepoRefreshBatchState::Failed) => {
            crate::server::readiness::PublicationPhaseState::Failed
        }
        Err(_) => crate::server::readiness::PublicationPhaseState::Unavailable,
    };
    state
        .write()
        .await
        .publication_readiness
        .record_repository(outcome);
}

/// Synchronize enabled repositories while the caller owns the complete
/// publication-cycle coordinator.
pub(crate) async fn refresh_repositories_uncoordinated(
    state: &Arc<RwLock<ServerState>>,
    force: bool,
) -> Result<RepoRefreshBatch, ServiceError> {
    refresh_repositories_scoped_uncoordinated(state, force, None).await
}

async fn refresh_repositories_scoped_uncoordinated(
    state: &Arc<RwLock<ServerState>>,
    force: bool,
    requested_profile: Option<&str>,
) -> Result<RepoRefreshBatch, ServiceError> {
    let db = db_path(state).await;
    let database_writer = state.read().await.database_writer.clone();
    let (repos, active_profiles) = blocking_anyhow({
        let db = db.clone();
        move || {
            let conn = conary_core::db::open_fast(&db).map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok((
                Repository::list_enabled(&conn).map_err(|e| anyhow::anyhow!("{e}"))?,
                RemiActiveProfileRevision::list(&conn).map_err(|e| anyhow::anyhow!("{e}"))?,
            ))
        }
    })
    .await?;

    let mut native_profiles = std::collections::BTreeMap::<String, Vec<Repository>>::new();
    let mut legacy_repositories = Vec::new();
    for repository in repos {
        if profile_refresh::is_native_profile_repository(&repository) {
            let source_profile = repository.source_profile.clone().ok_or_else(|| {
                ServiceError::Internal(format!(
                    "native repository '{}' has no source profile",
                    repository.name
                ))
            })?;
            native_profiles
                .entry(source_profile)
                .or_default()
                .push(repository);
        } else {
            legacy_repositories.push(repository);
        }
    }
    let empty_active_profiles = if requested_profile.is_none() {
        active_profiles
            .into_iter()
            .map(|active| active.source_profile)
            .filter(|profile| !native_profiles.contains_key(profile))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    deactivate_profiles_without_enabled_members(
        db.clone(),
        database_writer.clone(),
        empty_active_profiles,
    )
    .await?;

    if let Some(requested_profile) = requested_profile {
        restrict_to_profile(
            &mut native_profiles,
            &mut legacy_repositories,
            requested_profile,
        )?;
    }

    let profile_jobs = native_profiles
        .into_iter()
        .map(|(source_profile, repositories)| {
            let failure_sources = repositories
                .iter()
                .map(|repository| (repository.name.clone(), repository.source_profile.clone()))
                .collect::<Vec<_>>();
            async move {
                match profile_refresh::refresh_native_profile(
                    state,
                    source_profile,
                    repositories,
                    force,
                )
                .await
                {
                    Ok(results) => results
                        .into_iter()
                        .map(RepoRefreshOutcome::Success)
                        .collect::<Vec<_>>(),
                    Err(error) => failure_sources
                        .into_iter()
                        .map(|(name, source_profile)| {
                            RepoRefreshOutcome::Failure(RepoRefreshFailure::from_service_error_ref(
                                name,
                                source_profile,
                                &error,
                            ))
                        })
                        .collect(),
                }
            }
        });
    let mut batch = RepoRefreshBatch::default();
    let mut profile_stream = futures::stream::iter(profile_jobs).buffer_unordered(4);
    while let Some(outcomes) = profile_stream.next().await {
        for outcome in outcomes {
            batch.push(outcome);
        }
    }

    let jobs =
        legacy_repositories.into_iter().map(|repo| {
            let db = db.clone();
            let database_writer = database_writer.clone();
            let name = repo.name.clone();
            let source_profile = repo.source_profile.clone();
            async move {
                let result = if !force && !conary_core::repository::needs_sync(&repo) {
                    Ok(RepoRefreshResult {
                        name: name.clone(),
                        source_profile: source_profile.clone(),
                        packages_synced: 0,
                        skipped: true,
                    })
                } else {
                    refresh_loaded_repo(db, repo, database_writer).await
                };
                match result {
                    Ok(result) => RepoRefreshOutcome::Success(result),
                    Err(error) => RepoRefreshOutcome::Failure(
                        RepoRefreshFailure::from_service_error(name, source_profile, error),
                    ),
                }
            }
        });
    let legacy_batch = collect_refresh_outcomes(jobs).await;
    batch.results.extend(legacy_batch.results);
    batch.failures.extend(legacy_batch.failures);
    batch.sort();
    Ok(batch)
}

pub(super) fn restrict_to_profile(
    native_profiles: &mut std::collections::BTreeMap<String, Vec<Repository>>,
    legacy_repositories: &mut Vec<Repository>,
    requested_profile: &str,
) -> Result<(), ServiceError> {
    let repositories = native_profiles.remove(requested_profile).ok_or_else(|| {
        ServiceError::NotFound(format!(
            "native source profile '{requested_profile}' is not configured"
        ))
    })?;
    native_profiles.clear();
    native_profiles.insert(requested_profile.to_string(), repositories);
    legacy_repositories.clear();
    Ok(())
}

async fn deactivate_profiles_without_enabled_members(
    db: PathBuf,
    database_writer: DatabaseWriter,
    source_profiles: Vec<String>,
) -> Result<(), ServiceError> {
    if source_profiles.is_empty() {
        return Ok(());
    }
    blocking_anyhow(move || {
        database_writer
            .execute(|| {
                let mut conn = conary_core::db::open_fast(&db)?;
                let tx =
                    conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let enabled = Repository::list_enabled(&tx)?;
                for source_profile in source_profiles {
                    let has_enabled_native_member = enabled.iter().any(|repository| {
                        repository.source_profile.as_deref() == Some(source_profile.as_str())
                            && profile_refresh::is_native_profile_repository(repository)
                    });
                    if !has_enabled_native_member {
                        RemiActiveProfileRevision::retire(&tx, &source_profile)?;
                    }
                }
                tx.commit()?;
                Ok::<(), conary_core::Error>(())
            })
            .map_err(anyhow::Error::from)
    })
    .await
}
