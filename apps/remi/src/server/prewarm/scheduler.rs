// apps/remi/src/server/prewarm/scheduler.rs
//! Bounded, fair scheduling across exact source-profile prewarm jobs.

use super::{PrewarmConfig, PrewarmResult, run_prewarm_with_permits};
use anyhow::Result;
use futures::future::join_all;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Completed result for one configured prewarm route.
#[derive(Debug)]
pub(crate) struct PrewarmJobOutcome {
    pub(crate) distro: String,
    pub(crate) result: Result<PrewarmResult>,
}

/// Run every exact-profile prewarm job against the shared conversion budget.
///
/// All configured profiles begin together. Each profile retains its own top-N
/// ordering and sequential conversion semantics, while every individual
/// conversion acquires the same permit used by request-driven conversion.
pub(crate) async fn run_prewarm_jobs(
    jobs: Vec<PrewarmConfig>,
    conversion_permits: Arc<Semaphore>,
) -> Vec<PrewarmJobOutcome> {
    run_prewarm_jobs_with(jobs, conversion_permits, |job, permits| async move {
        run_prewarm_with_permits(&job, Some(permits.as_ref())).await
    })
    .await
}

async fn run_prewarm_jobs_with<Runner, RunFuture>(
    jobs: Vec<PrewarmConfig>,
    conversion_permits: Arc<Semaphore>,
    runner: Runner,
) -> Vec<PrewarmJobOutcome>
where
    Runner: Fn(PrewarmConfig, Arc<Semaphore>) -> RunFuture + Clone,
    RunFuture: Future<Output = Result<PrewarmResult>>,
{
    join_all(jobs.into_iter().map(move |job| {
        let runner = runner.clone();
        let distro = job.distro.clone();
        let conversion_permits = Arc::clone(&conversion_permits);
        async move {
            PrewarmJobOutcome {
                distro,
                result: runner(job, conversion_permits).await,
            }
        }
    }))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn job(distro: &str) -> PrewarmConfig {
        PrewarmConfig {
            db_path: "unused.db".to_string(),
            chunk_dir: "unused-chunks".to_string(),
            cache_dir: "unused-cache".to_string(),
            repository_keys_dir: None,
            distro: distro.to_string(),
            max_packages: 1000,
            popularity_file: None,
            pattern: None,
            dry_run: false,
        }
    }

    fn empty_result() -> PrewarmResult {
        PrewarmResult {
            packages_processed: 0,
            packages_converted: 0,
            packages_skipped: 0,
            packages_failed: 0,
            total_bytes: 0,
            converted: Vec::new(),
            failed: Vec::new(),
        }
    }

    #[tokio::test]
    async fn slow_first_profile_does_not_starve_later_profiles() {
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let release_first = Arc::new(Semaphore::new(0));
        let runner_release = Arc::clone(&release_first);
        let conversion_permits = Arc::new(Semaphore::new(1));
        let runner = move |job: PrewarmConfig, permits: Arc<Semaphore>| {
            let started_tx = started_tx.clone();
            let release_first = Arc::clone(&runner_release);
            async move {
                started_tx.send(job.distro.clone()).unwrap();
                let _permit = permits.acquire().await.unwrap();
                if job.distro == "arch" {
                    let release = release_first.acquire().await.unwrap();
                    release.forget();
                }
                Ok(empty_result())
            }
        };

        let scheduler = tokio::spawn(run_prewarm_jobs_with(
            vec![job("arch"), job("fedora"), job("ubuntu")],
            conversion_permits,
            runner,
        ));

        let started = tokio::time::timeout(Duration::from_secs(1), async {
            let mut started = HashSet::new();
            while started.len() < 3 {
                started.insert(started_rx.recv().await.unwrap());
            }
            started
        })
        .await
        .expect("all profiles should start while the first profile holds the only permit");

        assert_eq!(
            started,
            ["arch", "fedora", "ubuntu"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        release_first.add_permits(1);
        assert_eq!(scheduler.await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn profile_failure_is_reported_and_shared_concurrency_is_bounded() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let runner_active = Arc::clone(&active);
        let runner_peak = Arc::clone(&peak);
        let runner = move |job: PrewarmConfig, permits: Arc<Semaphore>| {
            let active = Arc::clone(&runner_active);
            let peak = Arc::clone(&runner_peak);
            async move {
                let _permit = permits.acquire().await.unwrap();
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                if job.distro == "fedora" {
                    anyhow::bail!("source failed");
                }
                Ok(empty_result())
            }
        };

        let outcomes = run_prewarm_jobs_with(
            vec![job("arch"), job("fedora"), job("ubuntu")],
            Arc::new(Semaphore::new(2)),
            runner,
        )
        .await;

        assert_eq!(outcomes.len(), 3);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert!(
            outcomes
                .iter()
                .any(|outcome| outcome.distro == "fedora" && outcome.result.is_err())
        );
        assert!(
            outcomes
                .iter()
                .any(|outcome| outcome.distro == "arch" && outcome.result.is_ok())
        );
        assert!(
            outcomes
                .iter()
                .any(|outcome| outcome.distro == "ubuntu" && outcome.result.is_ok())
        );
    }
}
