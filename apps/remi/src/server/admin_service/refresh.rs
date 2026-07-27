// apps/remi/src/server/admin_service/refresh.rs

//! Typed outcomes and bounded collection for multi-source refresh.

use futures::StreamExt;
use std::future::Future;

use super::ServiceError;

/// Result of one successful repository metadata refresh.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoRefreshResult {
    pub name: String,
    pub source_profile: Option<String>,
    pub packages_synced: usize,
    pub skipped: bool,
}

/// Stable failure class for one repository refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoRefreshFailureKind {
    SourceRejected,
    SourceNotFound,
    Conflict,
    Internal,
}

/// Failure from one source in a multi-repository refresh.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoRefreshFailure {
    pub name: String,
    pub source_profile: Option<String>,
    pub kind: RepoRefreshFailureKind,
    pub message: String,
}

impl RepoRefreshFailure {
    pub(super) fn from_service_error(
        name: String,
        source_profile: Option<String>,
        error: ServiceError,
    ) -> Self {
        let kind = match &error {
            ServiceError::BadRequest(_) => RepoRefreshFailureKind::SourceRejected,
            ServiceError::NotFound(_) => RepoRefreshFailureKind::SourceNotFound,
            ServiceError::Conflict(_) => RepoRefreshFailureKind::Conflict,
            ServiceError::Internal(_) => RepoRefreshFailureKind::Internal,
        };
        Self {
            name,
            source_profile,
            kind,
            message: error.to_string(),
        }
    }
}

/// Aggregate state of a multi-repository refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoRefreshBatchState {
    Complete,
    Partial,
    Failed,
}

impl RepoRefreshBatchState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}

/// Typed per-source outcomes for one multi-repository refresh.
#[derive(Debug, Default)]
pub struct RepoRefreshBatch {
    pub results: Vec<RepoRefreshResult>,
    pub failures: Vec<RepoRefreshFailure>,
}

impl RepoRefreshBatch {
    pub fn state(&self) -> RepoRefreshBatchState {
        match (self.results.is_empty(), self.failures.is_empty()) {
            (_, true) => RepoRefreshBatchState::Complete,
            (false, false) => RepoRefreshBatchState::Partial,
            (true, false) => RepoRefreshBatchState::Failed,
        }
    }

    pub fn synced_count(&self) -> usize {
        self.results.iter().filter(|result| !result.skipped).count()
    }

    pub fn skipped_count(&self) -> usize {
        self.results.iter().filter(|result| result.skipped).count()
    }
}

pub(super) enum RepoRefreshOutcome {
    Success(RepoRefreshResult),
    Failure(RepoRefreshFailure),
}

pub(super) async fn collect_refresh_outcomes<I, F>(jobs: I) -> RepoRefreshBatch
where
    I: IntoIterator<Item = F>,
    F: Future<Output = RepoRefreshOutcome>,
{
    let mut stream = futures::stream::iter(jobs).buffer_unordered(4);
    let mut batch = RepoRefreshBatch::default();
    while let Some(outcome) = stream.next().await {
        match outcome {
            RepoRefreshOutcome::Success(result) => batch.results.push(result),
            RepoRefreshOutcome::Failure(failure) => batch.failures.push(failure),
        }
    }
    batch
        .results
        .sort_by(|left, right| left.name.cmp(&right.name));
    batch
        .failures
        .sort_by(|left, right| left.name.cmp(&right.name));
    batch
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn one_source_failure_does_not_discard_successful_outcomes() {
        let jobs = vec![
            futures::future::ready(RepoRefreshOutcome::Failure(RepoRefreshFailure {
                name: "fedora-44".to_string(),
                source_profile: Some("fedora-44".to_string()),
                kind: RepoRefreshFailureKind::SourceRejected,
                message: "invalid RPM metadata".to_string(),
            })),
            futures::future::ready(RepoRefreshOutcome::Success(RepoRefreshResult {
                name: "ubuntu-26.04".to_string(),
                source_profile: Some("ubuntu-26.04".to_string()),
                packages_synced: 42,
                skipped: false,
            })),
        ];

        let batch = collect_refresh_outcomes(jobs).await;
        assert_eq!(batch.state(), RepoRefreshBatchState::Partial);
        assert_eq!(batch.results.len(), 1);
        assert_eq!(batch.failures.len(), 1);
        assert_eq!(batch.synced_count(), 1);
        assert_eq!(batch.skipped_count(), 0);
    }

    #[tokio::test]
    async fn configured_source_failure_preserves_current_source_result() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let db_path = temp.path().join("conary.db");
        let chunk_dir = temp.path().join("chunks");
        let cache_dir = temp.path().join("cache");
        std::fs::create_dir_all(&chunk_dir).expect("create chunks");
        std::fs::create_dir_all(&cache_dir).expect("create cache");

        {
            let conn = rusqlite::Connection::open(&db_path).expect("open database");
            conary_core::db::schema::ensure_current(&conn).expect("prepare schema");

            let mut current = conary_core::db::models::Repository::new(
                "current-fedora".to_string(),
                "https://current.example.test".to_string(),
            );
            current.source_profile = Some("fedora-44".to_string());
            current.metadata_expire = 21_600;
            current.insert(&conn).expect("insert current source");
            conn.execute(
                "UPDATE repositories SET last_sync = ?1 WHERE name = ?2",
                rusqlite::params![
                    conary_core::repository::current_timestamp(),
                    "current-fedora"
                ],
            )
            .expect("mark source current");

            let mut broken = conary_core::db::models::Repository::new(
                "broken-fedora".to_string(),
                "https://broken.example.test".to_string(),
            );
            broken.source_profile = Some("fedora-44".to_string());
            broken.insert(&conn).expect("insert broken source");
        }

        let config = crate::server::ServerConfig {
            db_path,
            chunk_dir,
            cache_dir,
            ..Default::default()
        };
        let state = Arc::new(RwLock::new(
            crate::server::ServerState::new(config).expect("build server state"),
        ));

        let batch = super::super::refresh_repositories(&state, false)
            .await
            .expect("enumerate configured sources");
        assert_eq!(batch.state(), RepoRefreshBatchState::Partial);
        assert_eq!(batch.results.len(), 1);
        assert_eq!(batch.results[0].name, "current-fedora");
        assert!(batch.results[0].skipped);
        assert_eq!(batch.failures.len(), 1);
        assert_eq!(batch.failures[0].name, "broken-fedora");
        assert_eq!(batch.failures[0].kind, RepoRefreshFailureKind::Internal);
    }
}
