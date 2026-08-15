// conary-core/src/repository/dependencies.rs

//! Dependency downloads
//!
//! Resolution happens in the SAT provider and returns exact repository row
//! identities. This module only downloads those already-selected rows.

use crate::error::{Error, Result};
use futures::{StreamExt, stream};
use std::future::Future;
use std::path::{Path, PathBuf};
use tracing::info;

use super::download::{
    DownloadOptions, DownloadProgress, download_package_verified_with_progress,
    download_static_package_verified_with_progress,
};
use super::remi::{RemiClient, RemiFetchRequest};
use super::selector::PackageWithRepo;

const MAX_CONCURRENT_DEPENDENCY_ACQUISITIONS: usize = 8;

struct PreparedDependencyAcquisition {
    input_index: usize,
    dependency_name: String,
    package: crate::db::models::RepositoryPackage,
    declared_size: u64,
    strategy: PreparedDependencyStrategy,
}

enum PreparedDependencyStrategy {
    Static,
    Native {
        trust: DownloadOptions,
    },
    Remi {
        client: RemiClient,
        route: String,
        trust_policy: crate::ccs::TrustPolicy,
    },
}

struct DependencyAcquisitionOutcome {
    input_index: usize,
    dependency_name: String,
    declared_size: u64,
    result: Result<PathBuf>,
}

/// Download all dependencies to a directory with bounded concurrency.
///
/// All SQLite-backed repository and trust decisions are resolved into immutable
/// plans before network or CAS work starts. Acquisitions may complete out of
/// order, but the returned paths and any reported failures retain the exact
/// resolver-selected input order.
///
/// # Arguments
/// * `dependencies` - List of (name, package info) tuples to download
/// * `dest_dir` - Directory to download packages to
/// * `keyring_dir` - Prepared native repository trust store
///
/// # Returns
/// Vec<(dependency_name, downloaded_path)> on success
pub async fn download_dependencies(
    conn: &rusqlite::Connection,
    dependencies: &[(String, PackageWithRepo)],
    dest_dir: &Path,
    keyring_dir: &Path,
    cas: &crate::filesystem::CasStore,
) -> Result<Vec<(String, PathBuf)>> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }

    let plans = prepare_dependency_acquisitions(conn, dependencies, keyring_dir)?;
    let total_size = plans.iter().map(|plan| plan.declared_size).sum();
    let total_mb = total_size as f64 / 1_048_576.0;

    info!(
        "Downloading {} dependencies in parallel ({:.2} MB total)...",
        dependencies.len(),
        total_mb
    );

    // Create multi-progress manager with aggregate tracking
    let progress = DownloadProgress::with_aggregate(dependencies.len(), total_size);

    // Pre-create progress bars for all downloads
    let progress_bars: Vec<_> = plans
        .iter()
        .map(|plan| progress.add_download(&plan.dependency_name, plan.declared_size))
        .collect();

    let individual_results = acquire_dependency_plans_with(
        plans,
        progress_bars,
        MAX_CONCURRENT_DEPENDENCY_ACQUISITIONS,
        |plan, progress_bar| async move {
            acquire_prepared_dependency(&plan, dest_dir, cas, Some(&progress_bar)).await
        },
    )
    .await;

    finish_dependency_acquisitions(&progress, dependencies.len(), individual_results)
}

fn finish_dependency_acquisitions(
    progress: &DownloadProgress,
    expected_count: usize,
    individual_results: Vec<DependencyAcquisitionOutcome>,
) -> Result<Vec<(String, PathBuf)>> {
    let mut succeeded_results = Vec::new();
    let mut failures = Vec::new();
    let mut bytes_downloaded: u64 = 0;

    for outcome in individual_results {
        match outcome.result {
            Ok(path) => {
                bytes_downloaded += outcome.declared_size;
                succeeded_results.push((outcome.dependency_name, path));
            }
            Err(e) => {
                failures.push(format!("{}: {e}", outcome.dependency_name));
            }
        }
    }

    let failed_count = failures.len();
    progress.finish_all(succeeded_results.len(), failed_count, bytes_downloaded);

    // If any downloads failed, return error
    if failed_count > 0 {
        let details = failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let suffix = if failed_count > 5 {
            format!("; ... and {} more", failed_count - 5)
        } else {
            String::new()
        };
        return Err(Error::DownloadError(format!(
            "{} of {} dependency downloads failed: {}{}",
            failed_count, expected_count, details, suffix
        )));
    }

    Ok(succeeded_results)
}

fn prepare_dependency_acquisitions(
    conn: &rusqlite::Connection,
    dependencies: &[(String, PackageWithRepo)],
    keyring_dir: &Path,
) -> Result<Vec<PreparedDependencyAcquisition>> {
    dependencies
        .iter()
        .enumerate()
        .map(|(input_index, (dependency_name, pkg_with_repo))| {
            let declared_size = u64::try_from(pkg_with_repo.package.size).map_err(|_| {
                Error::DownloadError(format!(
                    "negative package size for dependency '{}'",
                    dependency_name
                ))
            })?;
            let strategy = match pkg_with_repo.repository.default_strategy.as_deref() {
                Some("static") => PreparedDependencyStrategy::Static,
                Some("remi") => prepare_remi_dependency(conn, pkg_with_repo)?,
                None | Some("binary") => PreparedDependencyStrategy::Native {
                    trust: DownloadOptions::for_repository(&pkg_with_repo.repository, keyring_dir)?,
                },
                Some(strategy) => {
                    return Err(Error::ConfigError(format!(
                        "repository '{}' has unsupported dependency download strategy '{}'",
                        pkg_with_repo.repository.name, strategy
                    )));
                }
            };
            Ok(PreparedDependencyAcquisition {
                input_index,
                dependency_name: dependency_name.clone(),
                package: pkg_with_repo.package.clone(),
                declared_size,
                strategy,
            })
        })
        .collect()
}

fn prepare_remi_dependency(
    conn: &rusqlite::Connection,
    pkg_with_repo: &PackageWithRepo,
) -> Result<PreparedDependencyStrategy> {
    let repository = &pkg_with_repo.repository;
    let endpoint = repository
        .default_strategy_endpoint
        .as_deref()
        .filter(|endpoint| !endpoint.trim().is_empty())
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "Remi repository '{}' has no conversion endpoint",
                repository.name
            ))
        })?;
    let source_profile =
        super::selector::candidate_source_profile(&pkg_with_repo.package, repository)?.ok_or_else(
            || {
                Error::ConfigError(format!(
                    "Remi repository package '{}-{}' has no source profile",
                    pkg_with_repo.package.name, pkg_with_repo.package.version
                ))
            },
        )?;
    let profile =
        super::supported_profiles::profile_for_remi_target(source_profile).ok_or_else(|| {
            Error::ConfigError(format!(
                "Remi repository package '{}-{}' has unsupported source profile '{}'",
                pkg_with_repo.package.name, pkg_with_repo.package.version, source_profile
            ))
        })?;
    let repository_id = repository
        .id
        .ok_or_else(|| Error::ConfigError("Remi repository has no persisted identity".into()))?;

    Ok(PreparedDependencyStrategy::Remi {
        client: RemiClient::new(endpoint)?,
        route: profile.remi_route_slug().to_string(),
        trust_policy: super::resolution::remi_transport_trust_policy(conn, repository_id)?,
    })
}

async fn acquire_prepared_dependency(
    plan: &PreparedDependencyAcquisition,
    dest_dir: &Path,
    cas: &crate::filesystem::CasStore,
    progress_bar: Option<&indicatif::ProgressBar>,
) -> Result<PathBuf> {
    match &plan.strategy {
        PreparedDependencyStrategy::Static => {
            download_static_package_verified_with_progress(
                &plan.package,
                dest_dir,
                None,
                progress_bar,
            )
            .await
        }
        PreparedDependencyStrategy::Native { trust } => {
            download_package_verified_with_progress(&plan.package, dest_dir, trust, progress_bar)
                .await
        }
        PreparedDependencyStrategy::Remi {
            client,
            route,
            trust_policy,
        } => {
            client
                .fetch_package(RemiFetchRequest {
                    distro: route,
                    name: &plan.package.name,
                    version: Some(&plan.package.version),
                    architecture: plan.package.architecture.as_deref(),
                    output_dir: dest_dir,
                    cas,
                    trust_policy,
                })
                .await
        }
    }
}

async fn acquire_dependency_plans_with<F, Fut>(
    plans: Vec<PreparedDependencyAcquisition>,
    progress_bars: Vec<indicatif::ProgressBar>,
    concurrency: usize,
    acquire: F,
) -> Vec<DependencyAcquisitionOutcome>
where
    F: Fn(PreparedDependencyAcquisition, indicatif::ProgressBar) -> Fut,
    Fut: Future<Output = Result<PathBuf>>,
{
    assert!(
        concurrency > 0,
        "dependency acquisition bound must be nonzero"
    );
    assert_eq!(
        plans.len(),
        progress_bars.len(),
        "every dependency acquisition must have one progress bar"
    );
    let mut outcomes = stream::iter(plans.into_iter().zip(progress_bars))
        .map(|(plan, progress_bar)| {
            let input_index = plan.input_index;
            let dependency_name = plan.dependency_name.clone();
            let declared_size = plan.declared_size;
            info!("Downloading dependency: {}", dependency_name);
            let acquisition = acquire(plan, progress_bar.clone());
            async move {
                let result = acquisition.await;
                match &result {
                    Ok(_) => DownloadProgress::finish_download(&progress_bar, &dependency_name),
                    Err(error) => DownloadProgress::fail_download(
                        &progress_bar,
                        &dependency_name,
                        &error.to_string(),
                    ),
                }
                DependencyAcquisitionOutcome {
                    input_index,
                    dependency_name,
                    declared_size,
                    result,
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<DependencyAcquisitionOutcome>>()
        .await;
    outcomes.sort_by_key(|outcome| outcome.input_index);
    outcomes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{Repository, RepositoryPackage};
    use crate::repository::versioning::VersionScheme;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn package_with_repository(
        strategy: Option<&str>,
        endpoint: Option<String>,
        source_profile: Option<&str>,
        download_url: String,
        checksum: String,
        size: i64,
    ) -> PackageWithRepo {
        let mut repository = Repository::new(
            "dependency-source".to_string(),
            "https://metadata.example.invalid".to_string(),
        );
        repository.default_strategy = strategy.map(str::to_string);
        repository.default_strategy_endpoint = endpoint;
        repository.source_profile = source_profile.map(str::to_string);

        let mut package = RepositoryPackage::new(
            7,
            "dbus-broker".to_string(),
            "37-8.fc44".to_string(),
            VersionScheme::Rpm,
            checksum,
            size,
            download_url,
        );
        package.architecture = Some("x86_64".to_string());
        package.source_profile = source_profile.map(str::to_string);

        PackageWithRepo {
            package,
            repository,
        }
    }

    fn static_plan(index: usize) -> PreparedDependencyAcquisition {
        let mut selected = package_with_repository(
            Some("static"),
            None,
            None,
            format!("https://static.example.invalid/package-{index}.ccs"),
            format!("sha256:checksum-{index}"),
            (index + 1) as i64,
        );
        selected.package.name = format!("package-{index}");
        PreparedDependencyAcquisition {
            input_index: index,
            dependency_name: format!("dependency-{index}"),
            package: selected.package,
            declared_size: (index + 1) as u64,
            strategy: PreparedDependencyStrategy::Static,
        }
    }

    #[test]
    fn native_dependency_still_requires_ecosystem_trust_during_serial_planning() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let keyring = tempfile::tempdir().unwrap();
        let package = package_with_repository(
            Some("binary"),
            None,
            Some("fedora-44"),
            "https://native.example.invalid/dbus-broker.rpm".to_string(),
            "sha256:unused".to_string(),
            1,
        );

        let error = prepare_dependency_acquisitions(
            &conn,
            &[("dbus-broker".to_string(), package)],
            keyring.path(),
        )
        .err()
        .expect("native dependency planning must reject missing trust");

        assert!(
            error
                .to_string()
                .contains("has no ecosystem-native trust policy"),
            "native dependency trust must remain mandatory: {error}"
        );
    }

    #[tokio::test]
    async fn static_dependency_keeps_checksum_verified_local_path() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let cas_root = tempfile::tempdir().unwrap();
        let cas = crate::filesystem::CasStore::new(cas_root.path()).unwrap();
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let keyring = tempfile::tempdir().unwrap();
        let source_path = source.path().join("dependency.ccs");
        let bytes = [0x1f, 0x8b, 0x08, 0x00];
        std::fs::write(&source_path, bytes).unwrap();
        let package = package_with_repository(
            Some("static"),
            None,
            None,
            source_path.to_string_lossy().into_owned(),
            crate::hash::sha256(&bytes),
            bytes.len() as i64,
        );

        let plans = prepare_dependency_acquisitions(
            &conn,
            &[("dbus-broker".to_string(), package)],
            keyring.path(),
        )
        .unwrap();
        let downloaded = acquire_prepared_dependency(&plans[0], destination.path(), &cas, None)
            .await
            .expect("static dependency path must remain checksum verified");

        assert_eq!(std::fs::read(downloaded).unwrap(), bytes);
    }

    #[tokio::test]
    async fn bounded_acquisition_runs_concurrently_and_preserves_input_identity_order() {
        let package_count = MAX_CONCURRENT_DEPENDENCY_ACQUISITIONS + 4;
        let in_flight = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let completion_order = Arc::new(Mutex::new(Vec::new()));
        let observed_identity = Arc::new(Mutex::new(Vec::new()));
        let plans = (0..package_count).map(static_plan).collect::<Vec<_>>();
        let progress = (0..plans.len())
            .map(|_| indicatif::ProgressBar::hidden())
            .collect();

        let outcomes = acquire_dependency_plans_with(
            plans,
            progress,
            MAX_CONCURRENT_DEPENDENCY_ACQUISITIONS,
            {
                let in_flight = Arc::clone(&in_flight);
                let maximum = Arc::clone(&maximum);
                let completion_order = Arc::clone(&completion_order);
                let observed_identity = Arc::clone(&observed_identity);
                move |plan, _| {
                    let in_flight = Arc::clone(&in_flight);
                    let maximum = Arc::clone(&maximum);
                    let completion_order = Arc::clone(&completion_order);
                    let observed_identity = Arc::clone(&observed_identity);
                    async move {
                        let index = plan
                            .dependency_name
                            .strip_prefix("dependency-")
                            .unwrap()
                            .parse::<usize>()
                            .unwrap();
                        observed_identity
                            .lock()
                            .unwrap()
                            .push((plan.dependency_name.clone(), plan.package.name.clone()));
                        let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        maximum.fetch_max(current, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(
                            (package_count - index) as u64 * 5,
                        ))
                        .await;
                        in_flight.fetch_sub(1, Ordering::SeqCst);
                        completion_order.lock().unwrap().push(index);
                        Ok(PathBuf::from(format!("package-{index}.ccs")))
                    }
                }
            },
        )
        .await;

        assert!(maximum.load(Ordering::SeqCst) > 1);
        assert!(maximum.load(Ordering::SeqCst) <= MAX_CONCURRENT_DEPENDENCY_ACQUISITIONS);
        assert_ne!(
            *completion_order.lock().unwrap(),
            (0..package_count).collect::<Vec<_>>()
        );
        assert_eq!(
            outcomes
                .iter()
                .map(|outcome| outcome.dependency_name.as_str())
                .collect::<Vec<_>>(),
            (0..package_count)
                .map(|index| format!("dependency-{index}"))
                .collect::<Vec<_>>()
        );
        let mut observed = observed_identity.lock().unwrap().clone();
        observed.sort();
        let mut expected = (0..package_count)
            .map(|index| (format!("dependency-{index}"), format!("package-{index}")))
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(observed, expected);
    }

    #[tokio::test]
    async fn bounded_acquisition_drains_and_reports_every_failure_in_input_order() {
        let completed = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let plans = (0..6).map(static_plan).collect::<Vec<_>>();
        let progress_bars = (0..plans.len())
            .map(|_| indicatif::ProgressBar::hidden())
            .collect();

        let outcomes = acquire_dependency_plans_with(plans, progress_bars, 2, {
            let completed = Arc::clone(&completed);
            let in_flight = Arc::clone(&in_flight);
            let maximum = Arc::clone(&maximum);
            move |plan, _| {
                let completed = Arc::clone(&completed);
                let in_flight = Arc::clone(&in_flight);
                let maximum = Arc::clone(&maximum);
                async move {
                    let index = plan
                        .dependency_name
                        .strip_prefix("dependency-")
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    completed.fetch_add(1, Ordering::SeqCst);
                    if index == 1 || index == 4 {
                        Err(Error::DownloadError(format!("injected failure {index}")))
                    } else {
                        Ok(PathBuf::from(format!("package-{index}.ccs")))
                    }
                }
            }
        })
        .await;

        assert_eq!(completed.load(Ordering::SeqCst), 6);
        assert!(maximum.load(Ordering::SeqCst) > 1);
        assert!(maximum.load(Ordering::SeqCst) <= 2);
        let error =
            finish_dependency_acquisitions(&DownloadProgress::with_aggregate(6, 21), 6, outcomes)
                .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("2 of 6 dependency downloads failed"));
        assert!(message.contains("injected failure 1"));
        assert!(message.contains("injected failure 4"));
        let first = message.find("dependency-1").unwrap();
        let second = message.find("dependency-4").unwrap();
        assert!(
            first < second,
            "failures must retain input order: {message}"
        );
    }
}
