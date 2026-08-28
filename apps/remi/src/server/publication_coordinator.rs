// apps/remi/src/server/publication_coordinator.rs

//! Exclusive publication ownership and typed repository-refresh handoff.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{Mutex, OwnedMutexGuard};

use super::admin_service::{RepoRefreshBatch, RepoRefreshBatchState};

/// Exact repository set requested by one refresh execution.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "profile", rename_all = "snake_case")]
pub(crate) enum RepositoryRefreshScope {
    All,
    Profile(String),
}

/// Completed repository refresh returned to its producer or an eligible waiter.
#[derive(Debug, Clone)]
pub(crate) struct RepositoryRefreshExecution {
    pub generation: u64,
    pub scope: RepositoryRefreshScope,
    pub force: bool,
    pub started_at: i64,
    pub finished_at: i64,
    pub coalesced: bool,
    pub batch: RepoRefreshBatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepositoryRefreshTerminalState {
    Batch,
    Error,
    Incomplete,
}

#[derive(Debug, Clone)]
struct RepositoryRefreshRecord {
    execution: RepositoryRefreshExecution,
    terminal_state: RepositoryRefreshTerminalState,
}

#[derive(Debug, Default)]
struct RepositoryRefreshState {
    generation: u64,
    latest: Option<RepositoryRefreshRecord>,
}

/// One process-local owner for publication exclusion and refresh generations.
pub(crate) struct PublicationCoordinator {
    exclusive: Arc<Mutex<()>>,
    repository_refresh: StdMutex<RepositoryRefreshState>,
}

impl Default for PublicationCoordinator {
    fn default() -> Self {
        Self {
            exclusive: Arc::new(Mutex::new(())),
            repository_refresh: StdMutex::new(RepositoryRefreshState::default()),
        }
    }
}

pub(crate) enum RepositoryRefreshAdmission {
    Coalesced(RepositoryRefreshExecution),
    Execute(RepositoryRefreshPermit),
}

/// Linear owner for one admitted repository refresh.
///
/// Dropping an unfinished permit records an incomplete generation before the
/// exclusive guard is released, so an older successful result cannot satisfy a
/// waiter after a cancelled or panicking producer.
pub(crate) struct RepositoryRefreshPermit {
    coordinator: Arc<PublicationCoordinator>,
    guard: Option<OwnedMutexGuard<()>>,
    generation: u64,
    scope: RepositoryRefreshScope,
    force: bool,
    started_at: i64,
    recorded: bool,
}

impl PublicationCoordinator {
    /// Acquire the complete publication exclusion boundary.
    pub(crate) async fn lock_owned(&self) -> OwnedMutexGuard<()> {
        Arc::clone(&self.exclusive).lock_owned().await
    }

    /// Acquire publication ownership for a repository mutation that is not a
    /// typed batch refresh. The mutation makes any previously retained batch
    /// stale before its first read or write.
    pub(crate) async fn lock_repository_mutation_owned(&self) -> OwnedMutexGuard<()> {
        let guard = self.lock_owned().await;
        self.refresh_state().latest = None;
        guard
    }

    /// Admit a repository refresh or return an exact eligible completed run.
    pub(crate) async fn admit_repository_refresh(
        self: &Arc<Self>,
        scope: RepositoryRefreshScope,
        force: bool,
        accept_completed_after: Option<i64>,
    ) -> RepositoryRefreshAdmission {
        self.admit_repository_refresh_with_clock(
            scope,
            force,
            accept_completed_after,
            unix_timestamp,
        )
        .await
    }

    async fn admit_repository_refresh_with_clock<Clock>(
        self: &Arc<Self>,
        scope: RepositoryRefreshScope,
        force: bool,
        accept_completed_after: Option<i64>,
        clock: Clock,
    ) -> RepositoryRefreshAdmission
    where
        Clock: FnOnce() -> i64,
    {
        let guard = self.lock_owned().await;
        let mut state = self.refresh_state();
        if force
            && scope == RepositoryRefreshScope::All
            && let Some(floor) = accept_completed_after
            && let Some(record) = state.latest.as_ref()
            && record.qualifies_all_profile_force(floor)
        {
            let mut execution = record.execution.clone();
            execution.coalesced = true;
            drop(state);
            drop(guard);
            return RepositoryRefreshAdmission::Coalesced(execution);
        }

        state.generation = state
            .generation
            .checked_add(1)
            .expect("repository refresh generation overflowed");
        let generation = state.generation;
        drop(state);

        RepositoryRefreshAdmission::Execute(RepositoryRefreshPermit {
            coordinator: Arc::clone(self),
            guard: Some(guard),
            generation,
            scope,
            force,
            started_at: clock(),
            recorded: false,
        })
    }

    fn refresh_state(&self) -> std::sync::MutexGuard<'_, RepositoryRefreshState> {
        self.repository_refresh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl RepositoryRefreshRecord {
    fn qualifies_all_profile_force(&self, floor: i64) -> bool {
        self.terminal_state == RepositoryRefreshTerminalState::Batch
            && self.execution.scope == RepositoryRefreshScope::All
            && self.execution.finished_at > floor
            && self.execution.batch.state() == RepoRefreshBatchState::Complete
            && !self.execution.batch.results.is_empty()
            && self.execution.batch.skipped_count() == 0
    }
}

impl RepositoryRefreshPermit {
    /// Retain one terminal batch before returning the publication guard.
    pub(crate) fn complete(
        mut self,
        batch: RepoRefreshBatch,
    ) -> (OwnedMutexGuard<()>, RepositoryRefreshExecution) {
        self.complete_at(batch, unix_timestamp())
    }

    fn complete_at(
        &mut self,
        batch: RepoRefreshBatch,
        finished_at: i64,
    ) -> (OwnedMutexGuard<()>, RepositoryRefreshExecution) {
        let execution = RepositoryRefreshExecution {
            generation: self.generation,
            scope: self.scope.clone(),
            force: self.force,
            started_at: self.started_at,
            finished_at,
            coalesced: false,
            batch,
        };
        self.record(execution.clone(), RepositoryRefreshTerminalState::Batch);
        (
            self.guard.take().expect("refresh permit lost its guard"),
            execution,
        )
    }

    /// Record a top-level refresh error before returning the publication guard.
    pub(crate) fn fail(mut self) -> OwnedMutexGuard<()> {
        self.record_terminal(RepositoryRefreshTerminalState::Error, unix_timestamp());
        self.guard.take().expect("refresh permit lost its guard")
    }

    fn record_terminal(
        &mut self,
        terminal_state: RepositoryRefreshTerminalState,
        finished_at: i64,
    ) {
        let execution = RepositoryRefreshExecution {
            generation: self.generation,
            scope: self.scope.clone(),
            force: self.force,
            started_at: self.started_at,
            finished_at,
            coalesced: false,
            batch: RepoRefreshBatch::default(),
        };
        self.record(execution, terminal_state);
    }

    fn record(
        &mut self,
        execution: RepositoryRefreshExecution,
        terminal_state: RepositoryRefreshTerminalState,
    ) {
        self.coordinator.refresh_state().latest = Some(RepositoryRefreshRecord {
            execution,
            terminal_state,
        });
        self.recorded = true;
    }
}

impl Drop for RepositoryRefreshPermit {
    fn drop(&mut self) {
        if !self.recorded {
            self.record_terminal(RepositoryRefreshTerminalState::Incomplete, unix_timestamp());
        }
    }
}

fn unix_timestamp() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before the Unix epoch")
            .as_secs(),
    )
    .expect("Unix timestamp exceeds i64")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::server::admin_service::{
        RepoRefreshFailure, RepoRefreshFailureKind, RepoRefreshResult,
    };

    fn complete_batch(skipped: bool) -> RepoRefreshBatch {
        RepoRefreshBatch {
            results: vec![RepoRefreshResult {
                name: "fedora-44".to_string(),
                source_profile: Some("fedora-44".to_string()),
                packages_synced: usize::from(!skipped),
                skipped,
            }],
            failures: Vec::new(),
        }
    }

    async fn admit_at(
        coordinator: &Arc<PublicationCoordinator>,
        scope: RepositoryRefreshScope,
        force: bool,
        floor: Option<i64>,
        started_at: i64,
    ) -> RepositoryRefreshAdmission {
        coordinator
            .admit_repository_refresh_with_clock(scope, force, floor, || started_at)
            .await
    }

    #[tokio::test]
    async fn waiter_consumes_one_exact_post_floor_generation_without_missed_wakeup() {
        let coordinator = Arc::new(PublicationCoordinator::default());
        let producer =
            match admit_at(&coordinator, RepositoryRefreshScope::All, false, None, 10).await {
                RepositoryRefreshAdmission::Execute(permit) => permit,
                RepositoryRefreshAdmission::Coalesced(_) => panic!("startup refresh was coalesced"),
            };

        let waiter_coordinator = Arc::clone(&coordinator);
        let mut waiter = tokio::spawn(async move {
            admit_at(
                &waiter_coordinator,
                RepositoryRefreshScope::All,
                true,
                Some(15),
                21,
            )
            .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut waiter)
                .await
                .is_err(),
            "waiter bypassed the active producer"
        );

        let (guard, produced) = {
            let mut producer = producer;
            producer.complete_at(complete_batch(false), 20)
        };
        assert_eq!(produced.generation, 1);
        drop(guard);

        let consumed = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter resumed")
            .expect("waiter task completed");
        let RepositoryRefreshAdmission::Coalesced(consumed) = consumed else {
            panic!("waiter repeated qualifying refresh work");
        };
        assert_eq!(consumed.generation, produced.generation);
        assert_eq!(consumed.started_at, 10);
        assert_eq!(consumed.finished_at, 20);
        assert!(consumed.coalesced);
        assert_eq!(consumed.batch.synced_count(), 1);
    }

    #[tokio::test]
    async fn pre_floor_scoped_partial_skipped_and_incomplete_runs_are_ineligible() {
        async fn assert_next_executes(
            coordinator: &Arc<PublicationCoordinator>,
            floor: i64,
            expected_generation: u64,
        ) {
            let admission = admit_at(
                coordinator,
                RepositoryRefreshScope::All,
                true,
                Some(floor),
                30,
            )
            .await;
            let RepositoryRefreshAdmission::Execute(permit) = admission else {
                panic!("ineligible refresh was coalesced");
            };
            assert_eq!(permit.generation, expected_generation);
            drop(permit.fail());
        }

        let pre_floor = Arc::new(PublicationCoordinator::default());
        let RepositoryRefreshAdmission::Execute(mut permit) =
            admit_at(&pre_floor, RepositoryRefreshScope::All, false, None, 10).await
        else {
            unreachable!()
        };
        drop(permit.complete_at(complete_batch(false), 20).0);
        assert_next_executes(&pre_floor, 20, 2).await;

        let scoped = Arc::new(PublicationCoordinator::default());
        let RepositoryRefreshAdmission::Execute(mut permit) = admit_at(
            &scoped,
            RepositoryRefreshScope::Profile("fedora-44".to_string()),
            false,
            None,
            10,
        )
        .await
        else {
            unreachable!()
        };
        drop(permit.complete_at(complete_batch(false), 20).0);
        assert_next_executes(&scoped, 15, 2).await;

        let partial = Arc::new(PublicationCoordinator::default());
        let RepositoryRefreshAdmission::Execute(mut permit) =
            admit_at(&partial, RepositoryRefreshScope::All, false, None, 10).await
        else {
            unreachable!()
        };
        let mut batch = complete_batch(false);
        batch.failures.push(RepoRefreshFailure {
            name: "ubuntu".to_string(),
            source_profile: Some("ubuntu-26.04".to_string()),
            kind: RepoRefreshFailureKind::Internal,
            message: "failed".to_string(),
        });
        drop(permit.complete_at(batch, 20).0);
        assert_next_executes(&partial, 15, 2).await;

        let skipped = Arc::new(PublicationCoordinator::default());
        let RepositoryRefreshAdmission::Execute(mut permit) =
            admit_at(&skipped, RepositoryRefreshScope::All, false, None, 10).await
        else {
            unreachable!()
        };
        drop(permit.complete_at(complete_batch(true), 20).0);
        assert_next_executes(&skipped, 15, 2).await;

        let incomplete = Arc::new(PublicationCoordinator::default());
        let RepositoryRefreshAdmission::Execute(permit) =
            admit_at(&incomplete, RepositoryRefreshScope::All, false, None, 10).await
        else {
            unreachable!()
        };
        drop(permit);
        assert_next_executes(&incomplete, 0, 2).await;
    }

    #[tokio::test]
    async fn force_without_causal_floor_always_executes() {
        let coordinator = Arc::new(PublicationCoordinator::default());
        let RepositoryRefreshAdmission::Execute(mut producer) =
            admit_at(&coordinator, RepositoryRefreshScope::All, false, None, 10).await
        else {
            unreachable!()
        };
        drop(producer.complete_at(complete_batch(false), 20).0);

        let admission = admit_at(&coordinator, RepositoryRefreshScope::All, true, None, 30).await;
        let RepositoryRefreshAdmission::Execute(permit) = admission else {
            panic!("ordinary force request was coalesced");
        };
        assert_eq!(permit.generation, 2);
    }

    #[tokio::test]
    async fn intervening_repository_mutation_invalidates_retained_batch() {
        let coordinator = Arc::new(PublicationCoordinator::default());
        let RepositoryRefreshAdmission::Execute(mut producer) =
            admit_at(&coordinator, RepositoryRefreshScope::All, false, None, 10).await
        else {
            unreachable!()
        };
        drop(producer.complete_at(complete_batch(false), 20).0);

        drop(coordinator.lock_repository_mutation_owned().await);
        let admission = admit_at(
            &coordinator,
            RepositoryRefreshScope::All,
            true,
            Some(15),
            30,
        )
        .await;
        let RepositoryRefreshAdmission::Execute(permit) = admission else {
            panic!("repository mutation left a stale batch eligible");
        };
        assert_eq!(permit.generation, 2);
    }
}
