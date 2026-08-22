// apps/remi/src/server/admin_service/profile_refresh.rs

//! Profile-scoped immutable native catalog refresh and activation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use conary_core::db::models::{
    RemiActiveProfileRevision, RemiProfileRevisionActivation, RemiProfileRevisionMember,
    Repository, activate_profile_revision,
};
use conary_core::repository::catalog::{ProfileSourceMemberV1, SourceStreamKindV1};
use conary_core::repository::{
    PROFILE_SYNC_HEARTBEAT_INTERVAL, ProfileSyncFailureCategory, ProfileSyncFailureStage,
    ProfileSyncRun, ProfileSyncRunMember, RepositoryFormat, abort_profile_sync_run,
    acknowledge_profile_sync_candidate_cleanup, begin_profile_sync_run_with_members,
    heartbeat_profile_sync_run, ready_profile_sync_run, record_profile_sync_run_member,
};

use super::refresh::RepoRefreshResult;
use super::{ServiceError, blocking_anyhow};
use crate::server::catalog_authority::CatalogAuthority;
use crate::server::catalog_refresh::{
    PublishedProfileCatalog, StagedProfileCatalog, cleanup_candidate_run, plan_profile_sources,
    publish_staged_profile_catalog, stage_profile_catalog,
};
use crate::server::{ServerState, database_writer::DatabaseWriter};

struct RefreshRoots {
    db_path: PathBuf,
    keyring_dir: PathBuf,
    catalog_candidate_dir: PathBuf,
    catalog_dir: PathBuf,
    projection_cache_dir: PathBuf,
    database_writer: DatabaseWriter,
    catalog_gc_coordinator: Arc<tokio::sync::Mutex<()>>,
}

pub(super) fn is_native_profile_repository(repository: &Repository) -> bool {
    repository.source_profile.is_some()
        && matches!(
            repository.package_format,
            RepositoryFormat::Arch
                | RepositoryFormat::Debian
                | RepositoryFormat::Fedora
                | RepositoryFormat::Eopkg
        )
}

pub(super) async fn refresh_native_profile(
    state: &std::sync::Arc<tokio::sync::RwLock<ServerState>>,
    source_profile: String,
    repositories: Vec<Repository>,
    force: bool,
) -> Result<Vec<RepoRefreshResult>, ServiceError> {
    let roots = {
        let state = state.read().await;
        RefreshRoots {
            db_path: state.config.db_path.clone(),
            keyring_dir: conary_core::db::paths::keyring_dir(
                &state.config.db_path.display().to_string(),
            ),
            catalog_candidate_dir: state.config.catalog_candidate_dir.clone(),
            catalog_dir: state.config.catalog_dir.clone(),
            projection_cache_dir: state.config.cache_dir.join("native-projections"),
            database_writer: state.database_writer.clone(),
            catalog_gc_coordinator: Arc::clone(&state.catalog_gc_coordinator),
        }
    };

    let plans = plan_profile_sources(&source_profile, repositories.clone())
        .map_err(|error| ServiceError::Internal(format!("{error:#}")))?;
    if !force
        && repositories
            .iter()
            .all(|repository| !conary_core::repository::needs_sync(repository))
        && active_catalog_matches_plan(&roots, &source_profile, &plans).await
    {
        return Ok(repositories
            .into_iter()
            .map(|repository| RepoRefreshResult {
                name: repository.name,
                source_profile: repository.source_profile,
                packages_synced: 0,
                skipped: true,
            })
            .collect());
    }

    let names = plans
        .iter()
        .map(|plan| {
            (
                plan.repository
                    .repository_identity
                    .clone()
                    .expect("validated"),
                plan.repository.name.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let owner_instance_uuid = uuid::Uuid::new_v4().to_string();
    let run = begin_run(&roots, &source_profile, &owner_instance_uuid, &plans).await?;
    for recovery_run_id in &run.recovery_run_ids {
        if let Err(error) = cleanup_run(&roots, recovery_run_id).await {
            abort_run(
                &roots,
                &run,
                ProfileSyncFailureStage::Publishing,
                &error.to_string(),
            )
            .await;
            log_cleanup_failure(cleanup_run(&roots, &run.run_id).await, &run.run_id);
            return Err(error);
        }
    }
    if let Err(error) = collect_catalog_garbage(&roots).await {
        abort_run(
            &roots,
            &run,
            ProfileSyncFailureStage::Publishing,
            &error.to_string(),
        )
        .await;
        log_cleanup_failure(cleanup_run(&roots, &run.run_id).await, &run.run_id);
        return Err(error);
    }

    let (staged, heartbeat) =
        match stage_profile_catalog_with_heartbeat(&roots, &run, &source_profile, repositories)
            .await
        {
            Ok(published) => published,
            Err(error) => {
                abort_run(
                    &roots,
                    &run,
                    ProfileSyncFailureStage::FetchingObjects,
                    &format!("{error:#}"),
                )
                .await;
                log_cleanup_failure(cleanup_run(&roots, &run.run_id).await, &run.run_id);
                let primary = ServiceError::Internal(format!("{error:#}"));
                return match collect_catalog_garbage(&roots).await {
                    Ok(_) => Err(primary),
                    Err(cleanup) => Err(ServiceError::Internal(format!(
                        "{primary}; exact catalog recovery also failed: {cleanup}"
                    ))),
                };
            }
        };

    let results = match profile_refresh_results(&source_profile, &names, &staged) {
        Ok(results) => results,
        Err(error) => {
            stop_profile_heartbeat_after_error(heartbeat, &run.run_id).await;
            abort_run(
                &roots,
                &run,
                ProfileSyncFailureStage::Publishing,
                &error.to_string(),
            )
            .await;
            log_cleanup_failure(cleanup_run(&roots, &run.run_id).await, &run.run_id);
            return Err(error);
        }
    };
    if let Err(error) = record_publication_intent(&roots, &run, &plans, &staged).await {
        stop_profile_heartbeat_after_error(heartbeat, &run.run_id).await;
        abort_run(
            &roots,
            &run,
            ProfileSyncFailureStage::Publishing,
            &error.to_string(),
        )
        .await;
        log_cleanup_failure(cleanup_run(&roots, &run.run_id).await, &run.run_id);
        return Err(error);
    }
    let published = match publish_staged_profile_catalog(staged, &roots.catalog_dir).await {
        Ok(published) => published,
        Err(error) => {
            stop_profile_heartbeat_after_error(heartbeat, &run.run_id).await;
            abort_run(
                &roots,
                &run,
                ProfileSyncFailureStage::Publishing,
                &format!("{error:#}"),
            )
            .await;
            log_cleanup_failure(cleanup_run(&roots, &run.run_id).await, &run.run_id);
            let primary = ServiceError::Internal(format!("{error:#}"));
            return match collect_catalog_garbage(&roots).await {
                Ok(_) => Err(primary),
                Err(cleanup) => Err(ServiceError::Internal(format!(
                    "{primary}; exact catalog recovery also failed: {cleanup}"
                ))),
            };
        }
    };
    if let Err(error) = stop_profile_heartbeat(heartbeat).await {
        abort_run(
            &roots,
            &run,
            ProfileSyncFailureStage::Publishing,
            &format!("profile refresh heartbeat failed: {error:#}"),
        )
        .await;
        log_cleanup_failure(cleanup_run(&roots, &run.run_id).await, &run.run_id);
        return Err(ServiceError::Internal(format!(
            "profile refresh heartbeat failed: {error:#}"
        )));
    }
    let finalize = finalize_profile(&roots, &run, &plans, &published).await;
    if let Err(error) = finalize {
        abort_run(
            &roots,
            &run,
            ProfileSyncFailureStage::Publishing,
            &error.to_string(),
        )
        .await;
        log_cleanup_failure(cleanup_run(&roots, &run.run_id).await, &run.run_id);
        return match collect_catalog_garbage(&roots).await {
            Ok(_) => Err(error),
            Err(cleanup) => Err(ServiceError::Internal(format!(
                "{error}; exact catalog recovery also failed: {cleanup}"
            ))),
        };
    }
    log_cleanup_failure(cleanup_run(&roots, &run.run_id).await, &run.run_id);

    if let Err(error) = collect_catalog_garbage(&roots).await {
        tracing::error!(%error, "profile activated but exact catalog collection failed");
    }

    Ok(results)
}

async fn stage_profile_catalog_with_heartbeat(
    roots: &RefreshRoots,
    run: &ProfileSyncRun,
    source_profile: &str,
    repositories: Vec<Repository>,
) -> anyhow::Result<(StagedProfileCatalog, ProfileHeartbeat)> {
    let heartbeat = spawn_profile_heartbeat(roots, run)?;
    let staged = stage_profile_catalog(
        &run.run_id,
        source_profile,
        repositories,
        &roots.keyring_dir,
        &roots.catalog_candidate_dir,
        &roots.projection_cache_dir,
    )
    .await;
    match staged {
        Ok(staged) => Ok((staged, heartbeat)),
        Err(error) => match stop_profile_heartbeat(heartbeat).await {
            Ok(()) => Err(error),
            Err(heartbeat) => Err(error.context(format!(
                "profile refresh heartbeat also failed: {heartbeat:#}"
            ))),
        },
    }
}

type ProfileHeartbeat = (
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<anyhow::Result<()>>,
);

fn spawn_profile_heartbeat(
    roots: &RefreshRoots,
    run: &ProfileSyncRun,
) -> anyhow::Result<ProfileHeartbeat> {
    let (stop, stopped) = std::sync::mpsc::channel();
    let db_path = roots.db_path.clone();
    let database_writer = roots.database_writer.clone();
    let run = run.clone();
    let heartbeat = std::thread::Builder::new()
        .name(format!("profile-heartbeat-{}", run.source_profile))
        .spawn(move || {
            loop {
                match stopped.recv_timeout(PROFILE_SYNC_HEARTBEAT_INTERVAL) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        database_writer
                            .execute(|| {
                                let conn = conary_core::db::open_fast(&db_path)?;
                                heartbeat_profile_sync_run(&conn, &run)
                            })
                            .map_err(anyhow::Error::from)?;
                    }
                }
            }
        })?;
    Ok((stop, heartbeat))
}

async fn stop_profile_heartbeat((stop, heartbeat): ProfileHeartbeat) -> anyhow::Result<()> {
    let _ = stop.send(());
    tokio::task::spawn_blocking(move || heartbeat.join())
        .await
        .map_err(|error| anyhow::anyhow!("profile heartbeat join task panicked: {error}"))?
        .map_err(|_| anyhow::anyhow!("profile heartbeat thread panicked"))?
}

async fn stop_profile_heartbeat_after_error(heartbeat: ProfileHeartbeat, run_id: &str) {
    if let Err(error) = stop_profile_heartbeat(heartbeat).await {
        tracing::error!(run_id, %error, "profile refresh heartbeat also failed");
    }
}

fn profile_refresh_results(
    source_profile: &str,
    names: &BTreeMap<String, String>,
    staged: &StagedProfileCatalog,
) -> Result<Vec<RepoRefreshResult>, ServiceError> {
    staged
        .sources
        .iter()
        .map(|source| {
            let name = names
                .get(&source.manifest.repository_identity)
                .cloned()
                .ok_or_else(|| {
                    ServiceError::Internal(format!(
                        "staged source '{}' was not in the profile plan",
                        source.manifest.repository_identity
                    ))
                })?;
            Ok(RepoRefreshResult {
                name,
                source_profile: Some(source_profile.to_string()),
                packages_synced: usize::try_from(source.manifest.counts.packages).map_err(
                    |_| {
                        ServiceError::Internal(
                            "source catalog package count exceeds usize".to_string(),
                        )
                    },
                )?,
                skipped: false,
            })
        })
        .collect()
}

async fn active_catalog_matches_plan(
    roots: &RefreshRoots,
    source_profile: &str,
    plans: &[crate::server::catalog_refresh::ProfileSourcePlan],
) -> bool {
    let authority = CatalogAuthority::from_paths(
        &roots.db_path,
        &roots.catalog_dir,
        roots.database_writer.clone(),
    );
    let source_profile = source_profile.to_string();
    let plans = plans.to_vec();
    tokio::task::spawn_blocking(move || {
        authority
            .open_active_profile(&source_profile)
            .is_ok_and(|catalog| profile_members_match_plan(&catalog.manifest().members, &plans))
    })
    .await
    .unwrap_or(false)
}

pub(super) fn profile_members_match_plan(
    members: &[ProfileSourceMemberV1],
    plans: &[crate::server::catalog_refresh::ProfileSourcePlan],
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
                && member.priority == plan.priority
                && member.required == plan.required
        })
}

async fn begin_run(
    roots: &RefreshRoots,
    source_profile: &str,
    owner_instance_uuid: &str,
    plans: &[crate::server::catalog_refresh::ProfileSourcePlan],
) -> Result<ProfileSyncRun, ServiceError> {
    let db_path = roots.db_path.clone();
    let database_writer = roots.database_writer.clone();
    let source_profile = source_profile.to_string();
    let owner_instance_uuid = owner_instance_uuid.to_string();
    let repositories = plans
        .iter()
        .map(|plan| {
            (
                plan.ordinal,
                plan.priority,
                plan.required,
                plan.repository.clone(),
            )
        })
        .collect::<Vec<_>>();
    blocking_anyhow(move || {
        database_writer
            .execute(|| {
                let conn = conary_core::db::open_fast(&db_path)?;
                let active = RemiActiveProfileRevision::find(&conn, &source_profile)?;
                let input_digest = active
                    .as_ref()
                    .map(|active| active.profile_revision_sha256.as_str());
                let input_members = match input_digest {
                    Some(digest) => RemiProfileRevisionMember::list_for_revision(&conn, digest)?,
                    None => Vec::new(),
                };
                let input_by_repository = input_members
                    .into_iter()
                    .map(|member| (member.repository_identity, member.source_snapshot_sha256))
                    .collect::<BTreeMap<_, _>>();
                let members = repositories
                    .iter()
                    .map(|(ordinal, priority, required, repository)| {
                        let policy = repository.require_source_policy()?;
                        let repository_identity =
                            repository.repository_identity.clone().ok_or_else(|| {
                                conary_core::Error::ConfigError(format!(
                                    "repository '{}' has no exact repository identity",
                                    repository.name
                                ))
                            })?;
                        Ok(ProfileSyncRunMember {
                            ordinal: i64::from(*ordinal),
                            repository_id: repository.id.ok_or_else(|| {
                                conary_core::Error::MissingId(format!(
                                    "repository '{}' has no ID",
                                    repository.name
                                ))
                            })?,
                            source_identity: policy.source_identity.clone(),
                            repository_identity: repository_identity.clone(),
                            stream_kind: policy.stream.kind().to_string(),
                            stream_identity: policy.stream.identity().to_string(),
                            priority: i64::from(*priority),
                            required: *required,
                            input_source_snapshot_sha256: input_by_repository
                                .get(&repository_identity)
                                .cloned(),
                            candidate_source_snapshot_sha256: None,
                        })
                    })
                    .collect::<conary_core::Result<Vec<_>>>()?;
                begin_profile_sync_run_with_members(
                    &conn,
                    &source_profile,
                    input_digest,
                    &owner_instance_uuid,
                    &members,
                )
            })
            .map_err(anyhow::Error::from)
    })
    .await
}

async fn record_publication_intent(
    roots: &RefreshRoots,
    run: &ProfileSyncRun,
    plans: &[crate::server::catalog_refresh::ProfileSourcePlan],
    staged: &StagedProfileCatalog,
) -> Result<(), ServiceError> {
    let db_path = roots.db_path.clone();
    let database_writer = roots.database_writer.clone();
    let run = run.clone();
    let mut members = plans
        .iter()
        .map(|plan| {
            let policy = plan.repository.require_source_policy()?;
            Ok(ProfileSyncRunMember {
                ordinal: i64::from(plan.ordinal),
                repository_id: plan.repository.id.ok_or_else(|| {
                    conary_core::Error::MissingId(format!(
                        "repository '{}' has no ID",
                        plan.repository.name
                    ))
                })?,
                source_identity: policy.source_identity.clone(),
                repository_identity: plan.repository.repository_identity.clone().ok_or_else(
                    || {
                        conary_core::Error::ConfigError(format!(
                            "repository '{}' has no exact repository identity",
                            plan.repository.name
                        ))
                    },
                )?,
                stream_kind: policy.stream.kind().to_string(),
                stream_identity: policy.stream.identity().to_string(),
                priority: i64::from(plan.priority),
                required: plan.required,
                input_source_snapshot_sha256: None,
                candidate_source_snapshot_sha256: None,
            })
        })
        .collect::<conary_core::Result<Vec<_>>>()?;
    let input_members = load_run_input_members(&db_path, &run).await?;
    for member in &mut members {
        member.input_source_snapshot_sha256 =
            input_members.get(&member.repository_identity).cloned();
        let source = staged
            .sources
            .iter()
            .find(|source| i64::from(source.ordinal) == member.ordinal)
            .ok_or_else(|| {
                ServiceError::Internal(format!(
                    "profile '{}' lacks staged source ordinal {}",
                    staged.profile, member.ordinal
                ))
            })?;
        member.candidate_source_snapshot_sha256 = Some(
            source
                .manifest
                .manifest_sha256()
                .map_err(ServiceError::from)?,
        );
    }
    let profile_digest = staged
        .manifest
        .manifest_sha256()
        .map_err(ServiceError::from)?;
    blocking_anyhow(move || {
        database_writer
            .execute(|| {
                let conn = conary_core::db::open_fast(&db_path)?;
                for member in &members {
                    record_profile_sync_run_member(&conn, &run, member)?;
                }
                ready_profile_sync_run(&conn, &run, &profile_digest)
            })
            .map_err(anyhow::Error::from)
    })
    .await
}

async fn finalize_profile(
    roots: &RefreshRoots,
    run: &ProfileSyncRun,
    plans: &[crate::server::catalog_refresh::ProfileSourcePlan],
    published: &PublishedProfileCatalog,
) -> Result<(), ServiceError> {
    let db_path = roots.db_path.clone();
    let database_writer = roots.database_writer.clone();
    let run = run.clone();
    let source_manifests = published
        .sources
        .iter()
        .map(|source| source.manifest.clone())
        .collect::<Vec<_>>();
    let profile_manifest = published.manifest.clone();
    let profile_digest = profile_manifest
        .manifest_sha256()
        .map_err(ServiceError::from)?;
    let activation = RemiProfileRevisionActivation {
        source_profile: published.profile.clone(),
        profile_revision_sha256: profile_digest.clone(),
        artifact_sha256: profile_manifest.catalog.sha256.clone(),
        artifact_size: i64::try_from(profile_manifest.catalog.size)
            .map_err(|_| ServiceError::Internal("profile artifact size exceeds i64".to_string()))?,
        logical_digest_sha256: profile_manifest.logical_digest_sha256.clone(),
        run_id: run.run_id.clone(),
        owner_instance_uuid: run.owner_instance_uuid.clone(),
        fencing_epoch: run.fencing_epoch,
    };
    let repository_ids = plans
        .iter()
        .map(|plan| {
            plan.repository.id.ok_or_else(|| {
                ServiceError::Internal(format!("repository '{}' has no ID", plan.repository.name))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    blocking_anyhow(move || {
        database_writer
            .execute(|| {
                let conn = conary_core::db::open_fast(&db_path)?;
                conary_core::db::models::register_profile_catalog_revision(
                    &conn,
                    &source_manifests,
                    &profile_manifest,
                    unix_seconds()?,
                )?;
                activate_profile_revision(&conn, &activation)?;
                let completed_at = conary_core::repository::current_timestamp();
                for repository_id in repository_ids {
                    let (input, candidate) = conn.query_row(
                        "SELECT input_source_snapshot_sha256, candidate_source_snapshot_sha256
                     FROM repository_sync_run_members
                     WHERE run_id = ?1 AND repository_id = ?2",
                        rusqlite::params![&run.run_id, repository_id],
                        |row| {
                            Ok((
                                row.get::<_, Option<String>>(0)?,
                                row.get::<_, Option<String>>(1)?,
                            ))
                        },
                    )?;
                    let changed = source_snapshot_changed(input.as_deref(), candidate.as_deref())?;
                    conn.execute(
                        "UPDATE repositories
                     SET last_checked_at = ?1,
                         last_changed_at = CASE WHEN ?2 THEN ?1 ELSE last_changed_at END,
                         last_validated_at = ?1,
                         last_published_at = CASE WHEN ?2 THEN ?1 ELSE last_published_at END
                     WHERE id = ?3",
                        rusqlite::params![&completed_at, changed, repository_id],
                    )?;
                }
                Ok::<(), conary_core::Error>(())
            })
            .map_err(anyhow::Error::from)
    })
    .await
}

async fn load_run_input_members(
    db_path: &Path,
    run: &ProfileSyncRun,
) -> Result<BTreeMap<String, String>, ServiceError> {
    let db_path = db_path.to_path_buf();
    let run_id = run.run_id.clone();
    blocking_anyhow(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        let input_digest = conn.query_row(
            "SELECT input_profile_digest FROM repository_sync_runs WHERE run_id = ?1",
            [&run_id],
            |row| row.get::<_, Option<String>>(0),
        )?;
        let members = match input_digest {
            Some(digest) => RemiProfileRevisionMember::list_for_revision(&conn, &digest)?,
            None => Vec::new(),
        };
        Ok(members
            .into_iter()
            .map(|member| (member.repository_identity, member.source_snapshot_sha256))
            .collect())
    })
    .await
}

async fn abort_run(
    roots: &RefreshRoots,
    run: &ProfileSyncRun,
    stage: ProfileSyncFailureStage,
    evidence: &str,
) {
    let db_path = roots.db_path.clone();
    let database_writer = roots.database_writer.clone();
    let run = run.clone();
    let evidence = evidence.to_string();
    let result = tokio::task::spawn_blocking(move || {
        database_writer.execute(|| {
            let conn = conary_core::db::open_fast(&db_path)?;
            abort_profile_sync_run(
                &conn,
                &run,
                stage,
                ProfileSyncFailureCategory::Internal,
                &evidence,
            )
        })
    })
    .await;
    if let Err(error) = result {
        tracing::error!(%error, "profile refresh abort task panicked");
    } else if let Ok(Err(error)) = result {
        tracing::error!(%error, "failed to record profile refresh abort");
    }
}

async fn cleanup_run(roots: &RefreshRoots, run_id: &str) -> Result<(), ServiceError> {
    let root = roots.catalog_candidate_dir.clone();
    let db_path = roots.db_path.clone();
    let database_writer = roots.database_writer.clone();
    let run_id = run_id.to_string();
    match tokio::task::spawn_blocking(move || {
        cleanup_candidate_run(&root, &run_id)?;
        let acknowledged = database_writer
            .execute(|| {
                let conn = conary_core::db::open_fast(&db_path)?;
                acknowledge_profile_sync_candidate_cleanup(&conn, &run_id)
            })
            .map_err(anyhow::Error::from)?;
        if !acknowledged {
            anyhow::bail!(
                "terminal profile candidate run {run_id} was not pending cleanup acknowledgement"
            );
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(ServiceError::Internal(format!(
            "failed to clean exact profile candidate run: {error:#}"
        ))),
        Err(error) => Err(ServiceError::Internal(format!(
            "profile candidate cleanup task panicked: {error}"
        ))),
    }
}

async fn collect_catalog_garbage(
    roots: &RefreshRoots,
) -> Result<crate::server::catalog_gc::CatalogGcReport, ServiceError> {
    crate::server::catalog_gc::collect_catalog_garbage_serialized(
        Arc::clone(&roots.catalog_gc_coordinator),
        roots.db_path.clone(),
        roots.catalog_dir.clone(),
        roots.database_writer.clone(),
    )
    .await
    .map_err(|error| ServiceError::Internal(format!("exact catalog collection failed: {error:#}")))
}

fn log_cleanup_failure(result: Result<(), ServiceError>, run_id: &str) {
    if let Err(error) = result {
        tracing::error!(run_id, %error, "failed to clean exact profile candidate run");
    }
}

fn unix_seconds() -> conary_core::Result<i64> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            conary_core::Error::InternalError(format!("system time precedes Unix epoch: {error}"))
        })?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|_| conary_core::Error::InternalError("system time exceeds i64".to_string()))
}

fn source_snapshot_changed(
    input: Option<&str>,
    candidate: Option<&str>,
) -> conary_core::Result<bool> {
    let candidate = candidate.ok_or_else(|| {
        conary_core::Error::ConflictError(
            "profile activation has no candidate source snapshot identity".to_string(),
        )
    })?;
    Ok(input != Some(candidate))
}

#[cfg(test)]
mod timestamp_tests {
    use super::source_snapshot_changed;

    #[test]
    fn timestamp_change_authority_distinguishes_noop_change_and_missing_candidate() {
        let digest = "a".repeat(64);
        let changed = "b".repeat(64);

        assert!(!source_snapshot_changed(Some(&digest), Some(&digest)).unwrap());
        assert!(source_snapshot_changed(Some(&digest), Some(&changed)).unwrap());
        assert!(source_snapshot_changed(None, Some(&digest)).unwrap());
        assert!(source_snapshot_changed(Some(&digest), None).is_err());
    }
}
