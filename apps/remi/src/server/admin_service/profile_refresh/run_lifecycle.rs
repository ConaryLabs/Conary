// apps/remi/src/server/admin_service/profile_refresh/run_lifecycle.rs

use super::*;

pub(super) async fn begin_run(
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
                plan.role,
                plan.precedence,
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
                    .map(|(ordinal, role, precedence, required, repository)| {
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
                            role: *role,
                            precedence: i64::from(*precedence),
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

pub(super) async fn record_publication_intent(
    roots: &RefreshRoots,
    run: &ProfileSyncRun,
    plans: &[crate::server::catalog_refresh::ProfileSourcePlan],
    staged: ProfilePublicationIntent,
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
                role: plan.role,
                precedence: i64::from(plan.precedence),
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
        member.candidate_source_snapshot_sha256 = Some(
            staged
                .candidate_sources
                .get(&member.ordinal)
                .cloned()
                .ok_or_else(|| {
                    ServiceError::Internal(format!(
                        "profile '{}' lacks staged source ordinal {}",
                        staged.profile, member.ordinal
                    ))
                })?,
        );
    }
    blocking_anyhow(move || {
        database_writer
            .execute(|| {
                let conn = conary_core::db::open_fast(&db_path)?;
                for member in &members {
                    record_profile_sync_run_member(&conn, &run, member)?;
                }
                ready_profile_sync_run(&conn, &run, &staged.profile_digest)
            })
            .map_err(anyhow::Error::from)
    })
    .await
}

pub(super) async fn finalize_profile(
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
    let mut source_physical_attestations = BTreeMap::new();
    for source in &published.sources {
        if let Some(existing) = source_physical_attestations.insert(
            source.manifest.catalog.sha256.clone(),
            source.physical_attestation.clone(),
        ) && existing != source.physical_attestation
        {
            return Err(ServiceError::Internal(format!(
                "published source aliases for catalog artifact {} carry unequal portable attestations",
                source.manifest.catalog.sha256
            )));
        }
    }
    let profile_manifest = published.manifest.clone();
    let profile_physical_attestation = published.physical_attestation.clone();
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
                    &source_physical_attestations,
                    &profile_manifest,
                    profile_physical_attestation,
                    unix_seconds()?,
                )?;
                complete_profile_sync_candidate(&conn, &run)?;
                record_candidate_timestamps(
                    &conn,
                    &run.run_id,
                    &repository_ids,
                    &conary_core::repository::current_timestamp(),
                )?;
                Ok::<(), conary_core::Error>(())
            })
            .map_err(anyhow::Error::from)
    })
    .await
}

pub(super) fn record_candidate_timestamps(
    conn: &rusqlite::Connection,
    run_id: &str,
    repository_ids: &[i64],
    completed_at: &str,
) -> conary_core::Result<()> {
    for repository_id in repository_ids {
        let (input, candidate) = conn.query_row(
            "SELECT input_source_snapshot_sha256, candidate_source_snapshot_sha256
             FROM repository_sync_run_members
             WHERE run_id = ?1 AND repository_id = ?2",
            rusqlite::params![run_id, repository_id],
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
                 last_validated_at = ?1
             WHERE id = ?3",
            rusqlite::params![completed_at, changed, repository_id],
        )?;
    }
    Ok(())
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

pub(super) async fn abort_run(
    roots: &RefreshRoots,
    run: &ProfileSyncRun,
    stage: ProfileSyncFailureStage,
    category: ProfileSyncFailureCategory,
    evidence: &str,
) {
    let db_path = roots.db_path.clone();
    let database_writer = roots.database_writer.clone();
    let run = run.clone();
    let evidence = evidence.to_string();
    let result = tokio::task::spawn_blocking(move || {
        database_writer.execute(|| {
            let conn = conary_core::db::open_fast(&db_path)?;
            abort_profile_sync_run(&conn, &run, stage, category, &evidence)
        })
    })
    .await;
    if let Err(error) = result {
        tracing::error!(%error, "profile refresh abort task panicked");
    } else if let Ok(Err(error)) = result {
        tracing::error!(%error, "failed to record profile refresh abort");
    }
}

pub(super) async fn cleanup_run(roots: &RefreshRoots, run_id: &str) -> Result<(), ServiceError> {
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

pub(super) async fn collect_catalog_garbage(
    roots: &RefreshRoots,
) -> Result<crate::server::catalog_gc::CatalogGcReport, ServiceError> {
    crate::server::catalog_gc::collect_catalog_garbage_serialized(
        Arc::clone(&roots.catalog_gc_coordinator),
        roots.db_path.clone(),
        roots.catalog_dir.clone(),
        roots.database_writer.clone(),
        roots.catalog_authority.clone(),
    )
    .await
    .map_err(|error| ServiceError::Internal(format!("exact catalog collection failed: {error:#}")))
}

pub(super) fn log_cleanup_failure(result: Result<(), ServiceError>, run_id: &str) {
    if let Err(error) = result {
        tracing::error!(run_id, %error, "failed to clean exact profile candidate run");
    }
}
