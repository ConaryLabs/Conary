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
use tokio::time::{Instant, MissedTickBehavior};

use crate::server::admin_service::{self, RepoRefreshBatch};
use crate::server::canonical_fetch::{self, CanonicalCycleReport, CanonicalCycleState};
use crate::server::config::CanonicalSection;
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
        let (initial_refresh, initial_canonical) =
            run_ordered_initial(refresh_repositories(&schedule.state), || {
                run_canonical_cycle(
                    &schedule.state,
                    &schedule.db_path,
                    &schedule.canonical_config,
                )
            })
            .await;
        if let Some(batch) = initial_refresh {
            run_eligible_prewarm(&schedule, &batch).await;
        }
        log_canonical_cycle("Initial", &initial_canonical);

        let now = Instant::now();
        let mut refresh_tick =
            tokio::time::interval_at(now + schedule.refresh_interval, schedule.refresh_interval);
        refresh_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut canonical_tick = tokio::time::interval_at(
            now + schedule.canonical_interval,
            schedule.canonical_interval,
        );
        canonical_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                biased;
                _ = refresh_tick.tick() => {
                    if let Some(batch) = refresh_repositories(&schedule.state).await {
                        run_eligible_prewarm(&schedule, &batch).await;
                    }
                }
                _ = canonical_tick.tick() => {
                    let report = run_canonical_cycle(
                        &schedule.state,
                        &schedule.db_path,
                        &schedule.canonical_config,
                    ).await;
                    log_canonical_cycle("Periodic", &report);
                }
            }
        }
    });
}

async fn run_ordered_initial<
    RefreshFuture,
    RefreshOutput,
    Canonical,
    CanonicalFuture,
    CanonicalOutput,
>(
    refresh: RefreshFuture,
    canonical: Canonical,
) -> (RefreshOutput, CanonicalOutput)
where
    RefreshFuture: Future<Output = RefreshOutput>,
    Canonical: FnOnce() -> CanonicalFuture,
    CanonicalFuture: Future<Output = CanonicalOutput>,
{
    let refresh_output = refresh.await;
    let canonical_output = canonical().await;
    (refresh_output, canonical_output)
}

async fn refresh_repositories(state: &Arc<RwLock<ServerState>>) -> Option<RepoRefreshBatch> {
    match admin_service::refresh_repositories(state, false).await {
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

async fn run_canonical_cycle(
    state: &Arc<RwLock<ServerState>>,
    db_path: &std::path::Path,
    config: &CanonicalSection,
) -> CanonicalCycleReport {
    let database_writer = state.read().await.database_writer.clone();
    canonical_fetch::run_canonical_cycle(db_path, config, database_writer).await
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
        conary_core::db::init(&db_path).expect("initialize database");
        let writer = crate::server::database_writer::DatabaseWriter::default();
        let (transaction_started_tx, transaction_started_rx) = mpsc::channel();
        let (release_transaction_tx, release_transaction_rx) = mpsc::channel();
        let (canonical_started_tx, canonical_started_rx) = mpsc::channel();

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
            .expect("refresh transaction committed")
        };

        let canonical_db = db_path.clone();
        let canonical_writer = writer.clone();
        let initial = tokio::spawn(run_ordered_initial(refresh, move || async move {
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
            tokio::task::spawn_blocking(move || {
                canonical_fetch::persist_repology_projects(
                    &canonical_db,
                    &projects,
                    &canonical_writer,
                )
            })
            .await
            .expect("canonical persistence task completed")
            .expect("canonical persistence succeeded")
        }));

        transaction_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("repository transaction started");
        assert!(
            canonical_started_rx.try_recv().is_err(),
            "canonical persistence must not start while repository publication is held"
        );
        release_transaction_tx
            .send(())
            .expect("release repository transaction");
        tokio::time::timeout(Duration::from_secs(2), initial)
            .await
            .expect("initial publication does not wait for a periodic timer")
            .expect("initial publication task completed");

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
