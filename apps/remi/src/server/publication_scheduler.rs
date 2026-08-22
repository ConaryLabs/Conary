// apps/remi/src/server/publication_scheduler.rs

//! Ordered repository and canonical publication scheduling.
//!
//! One task owns both clocks. Startup completes a repository refresh attempt
//! before it starts the canonical fetch-and-rebuild cycle, and periodic cycles
//! are awaited in the same task so they cannot overlap.

use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, Semaphore};
use tokio::time::Instant;

use crate::server::admin_service::{self, RepoRefreshBatch, RepoRefreshBatchState};
use crate::server::canonical_fetch::{self, CanonicalCycleReport, CanonicalCycleState};
use crate::server::config::CanonicalSection;
use crate::server::readiness::PublicationPhaseState;
use crate::server::{ConversionService, PrewarmConfig, ServerState, prewarm};

pub(crate) struct PublicationSchedule {
    pub state: Arc<RwLock<ServerState>>,
    pub refresh_interval: Duration,
    pub canonical_interval: Duration,
    pub canonical_config: CanonicalSection,
    pub db_path: PathBuf,
    pub prewarm_jobs: Vec<PrewarmConfig>,
    pub prewarm_conversion_permits: Arc<Semaphore>,
    pub prewarm_conversion_service: ConversionService,
}

/// Start the sole owner of repository and canonical background publication.
pub(crate) fn spawn(schedule: PublicationSchedule) {
    tokio::spawn(async move {
        let (initial_refresh, initial_canonical) = run_initial_publication_with(
            &schedule.state,
            refresh_repositories_uncoordinated(&schedule.state),
            || {
                run_canonical_cycle_uncoordinated(
                    &schedule.state,
                    &schedule.db_path,
                    &schedule.canonical_config,
                )
            },
        )
        .await;
        log_canonical_cycle("Initial", &initial_canonical);
        let mut next_canonical = Instant::now() + schedule.canonical_interval;
        if let Some(batch) = initial_refresh {
            run_eligible_prewarm(&schedule, &batch).await;
        }
        let mut next_refresh = Instant::now() + schedule.refresh_interval;

        loop {
            tokio::time::sleep_until(next_refresh.min(next_canonical)).await;

            if Instant::now() >= next_refresh {
                if let Some(batch) = refresh_repositories(&schedule.state).await {
                    run_eligible_prewarm(&schedule, &batch).await;
                }
                next_refresh = Instant::now() + schedule.refresh_interval;
            }

            // Re-evaluate after refresh and prewarm. If they crossed the
            // canonical deadline, service it now instead of letting a shorter
            // refresh interval starve canonical publication indefinitely.
            if Instant::now() >= next_canonical {
                let report = run_canonical_cycle(&schedule).await;
                log_canonical_cycle("Periodic", &report);
                next_canonical = Instant::now() + schedule.canonical_interval;
            }
        }
    });
}

async fn run_initial_publication_with<RefreshFuture, Canonical, CanonicalFuture>(
    state: &Arc<RwLock<ServerState>>,
    refresh: RefreshFuture,
    canonical: Canonical,
) -> (Option<RepoRefreshBatch>, CanonicalCycleReport)
where
    RefreshFuture: Future<Output = Option<RepoRefreshBatch>>,
    Canonical: FnOnce() -> CanonicalFuture,
    CanonicalFuture: Future<Output = CanonicalCycleReport>,
{
    let coordinator = state.read().await.publication_coordinator.clone();
    let _publication_guard = coordinator.lock_owned().await;
    let refresh_output = refresh.await;
    let canonical_output = canonical().await;
    record_repository_readiness(state, refresh_output.as_ref()).await;
    record_canonical_readiness(state, &canonical_output).await;
    (refresh_output, canonical_output)
}

async fn refresh_repositories(state: &Arc<RwLock<ServerState>>) -> Option<RepoRefreshBatch> {
    let coordinator = state.read().await.publication_coordinator.clone();
    let _publication_guard = coordinator.lock_owned().await;
    let batch = refresh_repositories_uncoordinated(state).await;
    record_repository_readiness(state, batch.as_ref()).await;
    batch
}

async fn refresh_repositories_uncoordinated(
    state: &Arc<RwLock<ServerState>>,
) -> Option<RepoRefreshBatch> {
    match admin_service::refresh_repositories_uncoordinated(state, false).await {
        Ok(batch) => {
            tracing::info!(
                "Background metadata refresh {}: {} synced, {} skipped, {} failed",
                batch.state().as_str(),
                batch.synced_count(),
                batch.skipped_count(),
                batch.failures.len()
            );
            for failure in &batch.failures {
                tracing::warn!(
                    repository = %failure.name,
                    source_profile = failure.source_profile.as_deref().unwrap_or("<none>"),
                    kind = ?failure.kind,
                    "Repository metadata refresh failed: {}",
                    failure.message
                );
            }
            Some(batch)
        }
        Err(error) => {
            tracing::warn!("Background metadata refresh failed: {error}");
            None
        }
    }
}

async fn run_canonical_cycle(schedule: &PublicationSchedule) -> CanonicalCycleReport {
    let coordinator = schedule.state.read().await.publication_coordinator.clone();
    let _publication_guard = coordinator.lock_owned().await;
    let report = run_canonical_cycle_uncoordinated(
        &schedule.state,
        &schedule.db_path,
        &schedule.canonical_config,
    )
    .await;
    record_canonical_readiness(&schedule.state, &report).await;
    report
}

async fn run_canonical_cycle_uncoordinated(
    state: &Arc<RwLock<ServerState>>,
    db_path: &std::path::Path,
    config: &CanonicalSection,
) -> CanonicalCycleReport {
    let database_writer = state.read().await.database_writer.clone();
    let report = canonical_fetch::run_canonical_cycle(db_path, config, database_writer).await;
    if !matches!(
        report.rebuild,
        crate::server::canonical_fetch::CanonicalRebuildOutcome::Completed { .. }
    ) {
        return report;
    }
    match crate::server::universe_publish::publish_current_universe_from_state(state).await {
        Ok(_) => report,
        Err(error) => report.with_publication_failure(&error),
    }
}

async fn record_repository_readiness(
    state: &Arc<RwLock<ServerState>>,
    batch: Option<&RepoRefreshBatch>,
) {
    let outcome = match batch.map(RepoRefreshBatch::state) {
        Some(RepoRefreshBatchState::Complete) => PublicationPhaseState::Complete,
        Some(RepoRefreshBatchState::Partial) => PublicationPhaseState::Partial,
        Some(RepoRefreshBatchState::Failed) => PublicationPhaseState::Failed,
        None => PublicationPhaseState::Unavailable,
    };
    state
        .write()
        .await
        .publication_readiness
        .record_repository(outcome);
}

pub(crate) async fn record_canonical_readiness(
    state: &Arc<RwLock<ServerState>>,
    report: &CanonicalCycleReport,
) {
    let outcome = match report.state {
        CanonicalCycleState::Complete => PublicationPhaseState::Complete,
        CanonicalCycleState::Partial => PublicationPhaseState::Partial,
        CanonicalCycleState::Failed => PublicationPhaseState::Failed,
    };
    state
        .write()
        .await
        .publication_readiness
        .record_canonical(outcome);
}

fn log_canonical_cycle(label: &str, report: &CanonicalCycleReport) {
    match report.state {
        CanonicalCycleState::Complete => {
            tracing::info!("{label} canonical publication completed: {report:?}")
        }
        CanonicalCycleState::Partial => {
            tracing::warn!("{label} canonical publication was partial: {report:?}")
        }
        CanonicalCycleState::Failed => {
            tracing::warn!("{label} canonical publication failed: {report:?}")
        }
    }
}

async fn run_eligible_prewarm(schedule: &PublicationSchedule, batch: &RepoRefreshBatch) {
    let successful_profiles = successful_refresh_profile_ids(batch);
    let eligible_jobs = schedule
        .prewarm_jobs
        .iter()
        .filter_map(|job| {
            if prewarm_route_refreshed(&job.distro, &successful_profiles) {
                Some(job.clone())
            } else {
                tracing::warn!(
                    "Post-refresh pre-warm for {} skipped: its exact profile did not complete refresh",
                    job.distro
                );
                None
            }
        })
        .collect();

    for outcome in prewarm::run_prewarm_jobs(
        eligible_jobs,
        Arc::clone(&schedule.prewarm_conversion_permits),
        schedule.prewarm_conversion_service.clone(),
    )
    .await
    {
        match outcome.result {
            Ok(result) => tracing::info!(
                "Post-refresh pre-warm for {}: {} converted, {} skipped, {} failed",
                outcome.distro,
                result.packages_converted,
                result.packages_skipped,
                result.packages_failed
            ),
            Err(error) => tracing::warn!(
                "Post-refresh pre-warm for {} failed: {}",
                outcome.distro,
                error
            ),
        }
    }
}

fn successful_refresh_profile_ids(batch: &RepoRefreshBatch) -> HashSet<&str> {
    batch
        .results
        .iter()
        .filter_map(|result| result.source_profile.as_deref())
        .collect()
}

fn prewarm_route_refreshed(route: &str, successful_profile_ids: &HashSet<&str>) -> bool {
    conary_core::repository::supported_profiles::profile_for_remi_route(route)
        .is_some_and(|profile| successful_profile_ids.contains(profile.id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::canonical::repology::{RepologyImplementation, RepologyProject};
    use conary_core::db::models::{Repository, RepositoryPackage};
    use conary_core::repository::versioning::VersionScheme;
    use rusqlite::TransactionBehavior;
    use std::sync::mpsc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn initial_canonical_persistence_waits_for_held_repository_transaction() {
        let temp_dir = tempfile::tempdir().expect("create tempdir");
        let db_path = temp_dir.path().join("remi.db");
        let chunk_dir = temp_dir.path().join("chunks");
        let cache_dir = temp_dir.path().join("cache");
        std::fs::create_dir_all(&chunk_dir).expect("create chunk directory");
        std::fs::create_dir_all(&cache_dir).expect("create cache directory");
        conary_core::db::init(&db_path).expect("initialize database");
        let state = Arc::new(RwLock::new(
            ServerState::new(crate::server::ServerConfig {
                db_path: db_path.clone(),
                chunk_dir,
                cache_dir,
                ..Default::default()
            })
            .expect("create server state"),
        ));
        let writer = state.read().await.database_writer.clone();
        let (transaction_started_tx, transaction_started_rx) = mpsc::channel();
        let (release_transaction_tx, release_transaction_rx) = mpsc::channel();
        let (canonical_started_tx, canonical_started_rx) = mpsc::channel();
        let (contender_acquired_tx, contender_acquired_rx) = mpsc::channel();

        let refresh_db = db_path.clone();
        let refresh_writer = writer.clone();
        let refresh = async move {
            tokio::task::spawn_blocking(move || {
                refresh_writer.execute(|| {
                    let mut conn = crate::server::open_runtime_db(&refresh_db)?;
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let mut repository = Repository::new(
                        "fedora-bootstrap".to_string(),
                        "https://example.invalid/fedora".to_string(),
                    );
                    repository.source_profile = Some("fedora-44".to_string());
                    let repository_id = repository.insert(&tx)?;
                    let mut package = RepositoryPackage::new(
                        repository_id,
                        "bash".to_string(),
                        "5.2.0".to_string(),
                        VersionScheme::Rpm,
                        "sha256:bootstrap".to_string(),
                        42,
                        "https://example.invalid/bash.rpm".to_string(),
                    );
                    package.source_profile = Some("fedora-44".to_string());
                    package.insert(&tx)?;
                    transaction_started_tx.send(()).expect("signal transaction");
                    release_transaction_rx.recv().expect("release transaction");
                    tx.commit()?;
                    Ok::<_, conary_core::Error>(())
                })
            })
            .await
            .expect("refresh task completed")
            .expect("refresh transaction committed");
            Some(RepoRefreshBatch {
                results: vec![crate::server::admin_service::RepoRefreshResult {
                    name: "fedora-bootstrap".to_string(),
                    source_profile: Some("fedora-44".to_string()),
                    packages_synced: 1,
                    skipped: false,
                }],
                failures: Vec::new(),
            })
        };

        let canonical_db = db_path.clone();
        let canonical_writer = writer.clone();
        let initial_state = Arc::clone(&state);
        let initial = tokio::spawn(async move {
            run_initial_publication_with(&initial_state, refresh, move || async move {
                canonical_started_tx
                    .send(())
                    .expect("signal canonical start");
                let projects = vec![RepologyProject {
                    name: "bash".to_string(),
                    implementations: vec![RepologyImplementation {
                        repo: "fedora_44".to_string(),
                        visiblename: "bash".to_string(),
                        version: "5.2.0".to_string(),
                        status: "newest".to_string(),
                    }],
                }];
                let records = tokio::task::spawn_blocking(move || {
                    canonical_fetch::persist_repology_projects(
                        &canonical_db,
                        &projects,
                        &canonical_writer,
                    )
                })
                .await
                .expect("canonical persistence task completed")
                .expect("canonical persistence succeeded");
                canonical_fetch::CanonicalCycleReport::new(
                    canonical_fetch::CanonicalSourceOutcome::Persisted { records },
                    canonical_fetch::CanonicalSourceOutcome::Persisted { records: 0 },
                    canonical_fetch::CanonicalRebuildOutcome::Completed { new_mappings: 0 },
                )
            })
            .await
        });

        transaction_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("repository transaction started");
        assert!(
            canonical_started_rx.try_recv().is_err(),
            "canonical persistence must not start while repository publication is held"
        );
        let contender_coordinator = state.read().await.publication_coordinator.clone();
        let contender = tokio::spawn(async move {
            let _guard = contender_coordinator.lock_owned().await;
            contender_acquired_tx
                .send(())
                .expect("signal coordinator acquisition");
        });
        assert!(
            contender_acquired_rx.try_recv().is_err(),
            "manual publication must wait for the complete startup cycle"
        );
        release_transaction_tx
            .send(())
            .expect("release repository transaction");
        tokio::time::timeout(Duration::from_secs(2), initial)
            .await
            .expect("initial publication does not wait for a periodic timer")
            .expect("initial publication task completed");
        contender.await.expect("publication contender completed");
        contender_acquired_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("publication coordinator released after canonical persistence");

        let conn = crate::server::open_runtime_db(&db_path).expect("open database");
        let repository_packages: i64 = conn
            .query_row("SELECT COUNT(*) FROM repository_packages", [], |row| {
                row.get(0)
            })
            .expect("count repository packages");
        let canonical_cache: i64 = conn
            .query_row("SELECT COUNT(*) FROM repology_cache", [], |row| row.get(0))
            .expect("count canonical cache rows");
        assert_eq!(repository_packages, 1);
        assert_eq!(canonical_cache, 1);
        assert!(state.read().await.publication_readiness.is_ready());
    }

    #[test]
    fn failed_source_does_not_remove_successful_prewarm_profiles() {
        use crate::server::admin_service::{
            RepoRefreshFailure, RepoRefreshFailureKind, RepoRefreshResult,
        };

        let batch = RepoRefreshBatch {
            results: vec![
                RepoRefreshResult {
                    name: "fedora".to_string(),
                    source_profile: Some("fedora-44".to_string()),
                    packages_synced: 76_354,
                    skipped: false,
                },
                RepoRefreshResult {
                    name: "ubuntu".to_string(),
                    source_profile: Some("ubuntu-26.04".to_string()),
                    packages_synced: 6_487,
                    skipped: false,
                },
            ],
            failures: vec![RepoRefreshFailure {
                name: "arch-extra".to_string(),
                source_profile: Some("arch".to_string()),
                kind: RepoRefreshFailureKind::Internal,
                message: "source unavailable".to_string(),
            }],
        };

        let profiles = successful_refresh_profile_ids(&batch);
        assert_eq!(profiles.len(), 2);
        assert!(profiles.contains("fedora-44"));
        assert!(profiles.contains("ubuntu-26.04"));
        assert!(!profiles.contains("arch"));
        assert!(prewarm_route_refreshed("fedora", &profiles));
        assert!(prewarm_route_refreshed("ubuntu", &profiles));
        assert!(!prewarm_route_refreshed("arch", &profiles));
    }
}
