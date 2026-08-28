// apps/remi/src/server/admin_service/publication.rs

//! Coordination and typed readiness outcomes for repository mutation surfaces.

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::{OwnedMutexGuard, RwLock};

use super::{RepoRefreshResult, ServiceError};
use crate::server::ServerState;
use crate::server::catalog_refresh::StagedProfileCatalog;
use crate::server::readiness::PublicationPhaseState;

/// Send-safe immutable identity captured before staged catalog readers cross an
/// async database boundary.
pub(super) struct ProfilePublicationIntent {
    pub(super) profile: String,
    pub(super) candidate_sources: BTreeMap<i64, String>,
    pub(super) profile_digest: String,
}

pub(super) fn staged_profile_intent(
    staged: &StagedProfileCatalog,
) -> Result<ProfilePublicationIntent, ServiceError> {
    let candidate_sources = staged
        .sources
        .iter()
        .map(|source| {
            Ok((
                i64::from(source.ordinal),
                source
                    .manifest
                    .manifest_sha256()
                    .map_err(ServiceError::from)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, ServiceError>>()?;
    let profile_digest = staged
        .manifest
        .manifest_sha256()
        .map_err(ServiceError::from)?;
    Ok(ProfilePublicationIntent {
        profile: staged.profile.clone(),
        candidate_sources,
        profile_digest,
    })
}

pub(super) async fn guard(state: &Arc<RwLock<ServerState>>) -> OwnedMutexGuard<()> {
    let coordinator = state.read().await.publication_coordinator.clone();
    coordinator.lock_repository_mutation_owned().await
}

pub(super) async fn record_single_repository_outcome(
    state: &Arc<RwLock<ServerState>>,
    result: &Result<Option<RepoRefreshResult>, ServiceError>,
) {
    let outcome = match result {
        Ok(Some(_)) => PublicationPhaseState::Partial,
        Ok(None) => PublicationPhaseState::Unavailable,
        Err(_) => PublicationPhaseState::Failed,
    };
    state
        .write()
        .await
        .publication_readiness
        .record_repository(outcome);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use conary_core::db::models::Repository;
    use conary_core::repository::RepositoryParserConfig;

    use super::*;
    use crate::server::admin_service::sync_repo;
    use crate::server::{ServerConfig, ServerState};

    #[tokio::test]
    async fn single_repository_sync_waits_for_owner_and_records_recovery() {
        let root = tempfile::tempdir().expect("create tempdir");
        let db_path = root.path().join("remi.db");
        let chunk_dir = root.path().join("chunks");
        let cache_dir = root.path().join("cache");
        std::fs::create_dir_all(&chunk_dir).expect("create chunk directory");
        std::fs::create_dir_all(&cache_dir).expect("create cache directory");
        conary_core::db::init(&db_path).expect("initialize database");

        let conn = conary_core::db::open_fast(&db_path).expect("open database");
        let mut repository = Repository::new(
            "fedora-bootstrap".to_string(),
            "https://example.invalid/fedora".to_string(),
        );
        repository.source_profile = Some("fedora-44".to_string());
        let published_at = conary_core::repository::current_timestamp();
        repository.last_checked_at = Some(published_at.clone());
        repository.last_changed_at = Some(published_at.clone());
        repository.last_validated_at = Some(published_at.clone());
        repository.last_published_at = Some(published_at);
        repository
            .set_parser_config(RepositoryParserConfig::Json)
            .expect("set parser configuration");
        repository.insert(&conn).expect("insert repository");
        repository.update(&conn).expect("persist fresh sync time");
        drop(conn);

        let state = Arc::new(RwLock::new(
            ServerState::new(ServerConfig {
                db_path,
                chunk_dir,
                cache_dir,
                ..Default::default()
            })
            .expect("create server state"),
        ));
        let coordinator = state.read().await.publication_coordinator.clone();
        let held_publication = coordinator.lock_owned().await;
        let sync_state = Arc::clone(&state);
        let mut sync =
            tokio::spawn(async move { sync_repo(&sync_state, "fedora-bootstrap", false).await });

        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut sync)
                .await
                .is_err(),
            "single-repository sync bypassed the publication coordinator"
        );
        let state_writer = tokio::time::timeout(Duration::from_secs(2), state.write())
            .await
            .expect("waiting sync retained the server-state read lock");
        drop(state_writer);
        drop(held_publication);

        let result = tokio::time::timeout(Duration::from_secs(2), sync)
            .await
            .expect("sync resumed after publication owner released")
            .expect("sync task completed")
            .expect("sync succeeded")
            .expect("repository exists");
        assert!(
            result.skipped,
            "fresh repository should not perform network I/O"
        );
        assert_eq!(
            state.read().await.publication_readiness.repository,
            PublicationPhaseState::Partial
        );
    }
}
