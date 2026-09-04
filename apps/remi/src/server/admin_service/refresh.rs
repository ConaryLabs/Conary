// apps/remi/src/server/admin_service/refresh.rs

//! Typed outcomes and bounded collection for multi-source refresh.

use futures::StreamExt;
use std::future::Future;

use super::ServiceError;

mod operations;
pub(crate) use operations::refresh_repositories_uncoordinated;
pub use operations::sync_repo;
pub(crate) use operations::{refresh_profile_repositories, refresh_repositories};

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
    StorageCapacity,
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
        Self::from_service_error_ref(name, source_profile, &error)
    }

    pub(super) fn from_service_error_ref(
        name: String,
        source_profile: Option<String>,
        error: &ServiceError,
    ) -> Self {
        let kind = match &error {
            ServiceError::BadRequest(_) => RepoRefreshFailureKind::SourceRejected,
            ServiceError::NotFound(_) => RepoRefreshFailureKind::SourceNotFound,
            ServiceError::Conflict(_) => RepoRefreshFailureKind::Conflict,
            ServiceError::StorageCapacity(_) => RepoRefreshFailureKind::StorageCapacity,
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
#[derive(Debug, Clone, Default)]
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

    pub(super) fn push(&mut self, outcome: RepoRefreshOutcome) {
        match outcome {
            RepoRefreshOutcome::Success(result) => self.results.push(result),
            RepoRefreshOutcome::Failure(failure) => self.failures.push(failure),
        }
    }

    pub(super) fn sort(&mut self) {
        self.results
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.failures
            .sort_by(|left, right| left.name.cmp(&right.name));
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
        batch.push(outcome);
    }
    batch.sort();
    batch
}

#[cfg(test)]
mod tests;
