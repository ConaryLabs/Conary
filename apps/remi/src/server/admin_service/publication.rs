// apps/remi/src/server/admin_service/publication.rs

//! Coordination and typed readiness outcomes for repository mutation surfaces.

use std::sync::Arc;

use tokio::sync::{OwnedMutexGuard, RwLock};

use super::{RepoRefreshResult, ServiceError};
use crate::server::ServerState;
use crate::server::readiness::PublicationPhaseState;

pub(super) async fn guard(state: &Arc<RwLock<ServerState>>) -> OwnedMutexGuard<()> {
    state
        .read()
        .await
        .publication_coordinator
        .clone()
        .lock_owned()
        .await
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
        repository.last_sync = Some(conary_core::repository::current_timestamp());
        repository.insert(&conn).expect("insert repository");
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
