// conary-test/src/server/service.rs
//! Shared business logic for the HTTP and MCP interfaces.
//!
//! Handlers and MCP tools are thin wrappers that delegate to these
//! functions, converting the results into their respective response types.

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::config::load_manifest;
use crate::container::{ContainerBackend, ImageInfo};
use crate::engine::suite::{RunStatus, TestSuite};
use crate::report::json::to_json_value;
use crate::server::state::AppState;

// ---------------------------------------------------------------------------
// Return types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SuiteInfo {
    pub name: String,
    pub phase: u32,
    pub test_count: usize,
}

#[derive(Debug, Serialize)]
pub struct StartRunResult {
    pub run_id: u64,
    pub suite: String,
    pub distro: String,
    pub phase: u32,
}

#[derive(Debug)]
pub struct RerunResult {
    pub run_id: u64,
    pub suite_name: String,
    pub distro: String,
    pub phase: u32,
}

#[derive(Debug, Serialize)]
pub struct RunSummary {
    pub run_id: u64,
    pub suite: String,
    pub phase: u32,
    pub status: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Debug, Serialize)]
pub struct DistroInfo {
    pub name: String,
    pub remi_distro: String,
    pub repo_name: String,
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

/// List all TOML manifests in the manifest directory.
pub fn list_suites(state: &AppState) -> Result<Vec<SuiteInfo>> {
    let manifest_dir = std::path::Path::new(&state.manifest_dir);
    let entries = std::fs::read_dir(manifest_dir)?;

    let mut suites = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml")
            && let Ok(manifest) = load_manifest(&path)
        {
            suites.push(SuiteInfo {
                name: manifest.suite.name,
                phase: manifest.suite.phase,
                test_count: manifest.test.len(),
            });
        }
    }

    suites.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(suites)
}

/// Start a new test run after validating the distro name.
pub fn start_run(
    state: &AppState,
    suite_name: &str,
    distro: &str,
    phase: u32,
) -> Result<StartRunResult> {
    if !state.config.distros.contains_key(distro) {
        bail!("unknown distro: {distro}");
    }

    let run_id = AppState::next_run_id();
    let suite = TestSuite::new(suite_name, phase);
    state.insert_run(run_id, suite);
    state.run_meta.insert(
        run_id,
        crate::server::state::RunMeta {
            suite_name: suite_name.to_string(),
            distro: distro.to_string(),
            phase,
        },
    );

    Ok(StartRunResult {
        run_id,
        suite: suite_name.to_string(),
        distro: distro.to_string(),
        phase,
    })
}

/// Spawn a background task that actually executes a test run.
///
/// This handles the full lifecycle: build image, create container,
/// initialize conary state, load manifest, run tests, update state,
/// and clean up the container.
pub fn spawn_run(state: &AppState, run_id: u64, suite_name: &str, distro: &str, phase: u32) {
    let state = state.clone();
    let suite_name = suite_name.to_string();
    let distro = distro.to_string();

    tokio::spawn(async move {
        // Wait for a permit -- limits concurrent test execution to prevent
        // resource exhaustion on memory-constrained hosts.
        let _permit = state
            .run_semaphore
            .acquire()
            .await
            .expect("run semaphore closed");
        tracing::info!(run_id, "acquired run permit");

        if let Err(e) = execute_run(&state, run_id, &suite_name, &distro, phase).await {
            tracing::error!(run_id, error = %e, "test run failed");
            if let Some(mut entry) = state.runs.get_mut(&run_id) {
                entry.status = RunStatus::Cancelled;
            }
            state.remove_cancel_flag(run_id);
        }
        // _permit drops here, releasing the semaphore slot.
    });
}

/// Inner async function that executes a test run end-to-end.
async fn execute_run(
    state: &AppState,
    run_id: u64,
    suite_name: &str,
    distro: &str,
    phase: u32,
) -> Result<()> {
    use crate::container::{BollardBackend, ContainerBackend, ContainerConfig, VolumeMount};
    use crate::engine::runner::TestRunner;

    tracing::info!(run_id, suite_name, distro, phase, "starting test run");

    // Mark as running.
    if let Some(mut entry) = state.runs.get_mut(&run_id) {
        entry.status = RunStatus::Running;
    }

    // Register cancellation flag.
    let cancel_flag = state.register_cancel_flag(run_id);

    // Build the image (serialized per-distro to avoid Podman contention).
    let image_tag = {
        let lock = state.image_lock(distro);
        let mut cached = lock.lock().await;
        if let Some(ref tag) = *cached {
            tracing::info!(run_id, image = %tag, "reusing cached image");
            tag.clone()
        } else {
            tracing::info!(run_id, distro, "building image (first run for this distro)");
            let tag = build_image(state, distro).await?;
            *cached = Some(tag.clone());
            tag
        }
    };
    tracing::info!(run_id, image = %image_tag, "image ready");

    // Create and start the container.
    let backend = BollardBackend::new()?;
    let results_dir = state.config.paths.results_dir.clone();
    let host_results_dir = std::env::current_dir()
        .unwrap_or_default()
        .join("tests/integration/remi/results");
    std::fs::create_dir_all(&host_results_dir).ok();

    let container_config = ContainerConfig {
        image: image_tag,
        privileged: true,
        volumes: vec![VolumeMount {
            host_path: host_results_dir.display().to_string(),
            container_path: results_dir,
            read_only: false,
        }],
        ..Default::default()
    };
    let container_id = backend.create(container_config.clone()).await?;
    tracing::info!(run_id, id = %container_id, "container created");
    backend.start(&container_id).await?;

    // Initialize conary state inside the container.
    initialize_container(state, distro, phase, &backend, &container_id).await?;

    // Load the manifest.
    let manifest_path =
        std::path::PathBuf::from(&state.manifest_dir).join(format!("{suite_name}.toml"));
    let manifest = crate::config::load_manifest(&manifest_path)?;

    // Run the tests.
    let mut runner = TestRunner::new(state.config.clone(), distro.to_string());
    let suite = runner
        .run_with_cancel(
            &manifest,
            &backend,
            &container_id,
            Some(&container_config),
            Some(cancel_flag),
            Some((run_id, state.event_tx.clone())),
        )
        .await?;

    // Update the suite in state with results.
    if let Some(mut entry) = state.runs.get_mut(&run_id) {
        entry.status = suite.status;
        entry.results = suite.results;
        entry.finished_at = suite.finished_at;
    }

    // Cleanup.
    state.remove_cancel_flag(run_id);
    if let Err(e) = backend.stop(&container_id).await {
        tracing::warn!(run_id, error = %e, "failed to stop container");
    }
    if let Err(e) = backend.remove(&container_id).await {
        tracing::warn!(run_id, error = %e, "failed to remove container");
    }

    tracing::info!(run_id, "test run complete");
    Ok(())
}

/// Initialize conary database and repos inside a test container.
///
/// Mirrors the logic from `cli.rs::initialize_container_state`.
async fn initialize_container(
    state: &AppState,
    distro: &str,
    _phase: u32,
    backend: &dyn ContainerBackend,
    container_id: &crate::container::ContainerId,
) -> Result<()> {
    use std::time::Duration;

    let config = &state.config;
    let db_parent = std::path::Path::new(&config.paths.db)
        .parent()
        .context("db path has no parent directory")?
        .display()
        .to_string();
    let init_cmd = format!(
        "mkdir -p {db_parent} && {} system init --db-path {}",
        config.paths.conary_bin, config.paths.db
    );
    let init_result = backend
        .exec(
            container_id,
            &["sh", "-c", &init_cmd],
            Duration::from_secs(120),
        )
        .await?;
    if init_result.exit_code != 0 {
        bail!(
            "failed to initialize conary database: {}{}",
            init_result.stdout,
            init_result.stderr
        );
    }

    for repo in &config.setup.remove_default_repos {
        let remove_cmd = format!(
            "{} repo remove {} --db-path {} >/dev/null 2>&1 || true",
            config.paths.conary_bin, repo, config.paths.db
        );
        backend
            .exec(
                container_id,
                &["sh", "-c", &remove_cmd],
                Duration::from_secs(30),
            )
            .await?;
    }

    {
        let distro_config = config
            .distros
            .get(distro)
            .with_context(|| format!("unknown distro: {distro}"))?;
        let add_repo_cmd = format!(
            "{} repo add {} {} --default-strategy remi --remi-endpoint {} --remi-distro {} --no-gpg-check --db-path {} >/dev/null 2>&1 || true",
            config.paths.conary_bin,
            distro_config.repo_name,
            config.remi.endpoint,
            config.remi.endpoint,
            distro_config.remi_distro,
            config.paths.db
        );
        backend
            .exec(
                container_id,
                &["sh", "-c", &add_repo_cmd],
                Duration::from_secs(60),
            )
            .await?;
    }

    Ok(())
}

/// Retrieve a run's full report as a JSON value.
pub fn get_run(state: &AppState, run_id: u64) -> Result<serde_json::Value> {
    match state.runs.get(&run_id) {
        Some(entry) => to_json_value(&entry),
        None => bail!("run {run_id} not found"),
    }
}

/// List runs with summary information, sorted by run ID descending.
pub fn list_runs(state: &AppState, limit: usize) -> Vec<RunSummary> {
    let mut summaries: Vec<RunSummary> = state
        .runs
        .iter()
        .map(|entry| {
            let id = *entry.key();
            let suite = entry.value();
            RunSummary {
                run_id: id,
                suite: suite.name.clone(),
                phase: suite.phase,
                status: suite.status.as_str().to_string(),
                total: suite.total(),
                passed: suite.passed(),
                failed: suite.failed(),
                skipped: suite.skipped(),
            }
        })
        .collect();

    summaries.sort_by(|a, b| b.run_id.cmp(&a.run_id));
    summaries.truncate(limit);
    summaries
}

/// Logs from a single test attempt.
#[derive(Debug, Serialize)]
pub struct AttemptLogs {
    pub attempt: u32,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

/// Aggregated logs for all attempts of a test.
#[derive(Debug, Serialize)]
pub struct TestLogs {
    pub test_id: String,
    pub attempts: Vec<AttemptLogs>,
}

/// Summary and artifact paths for a completed run.
#[derive(Debug, Serialize)]
pub struct RunArtifacts {
    pub run_id: u64,
    pub status: String,
    pub report_path: Option<String>,
    pub summary: RunSummary,
}

/// Result of a container cleanup operation.
#[derive(Debug, Serialize)]
pub struct CleanupResult {
    pub removed: usize,
    pub errors: Vec<String>,
}

/// List all configured distros.
pub fn list_distros(state: &AppState) -> Vec<DistroInfo> {
    let mut distros: Vec<DistroInfo> = state
        .config
        .distros
        .iter()
        .map(|(name, cfg)| DistroInfo {
            name: name.clone(),
            remi_distro: cfg.remi_distro.clone(),
            repo_name: cfg.repo_name.clone(),
        })
        .collect();

    distros.sort_by(|a, b| a.name.cmp(&b.name));
    distros
}

// ---------------------------------------------------------------------------
// Cancel / Rerun
// ---------------------------------------------------------------------------

/// Cancel a running test run. Sets the cancellation flag and marks the
/// suite as cancelled.
pub fn cancel_run(state: &AppState, run_id: u64) -> Result<()> {
    if !state.cancel_run(run_id) {
        // No cancellation flag found -- check if the run exists at all.
        if state.runs.contains_key(&run_id) {
            // Run exists but has no flag (already finished). Still mark it.
            if let Some(mut entry) = state.runs.get_mut(&run_id) {
                entry.status = RunStatus::Cancelled;
            }
            return Ok(());
        }
        bail!("run {run_id} not found");
    }

    // Also mark the suite status.
    if let Some(mut entry) = state.runs.get_mut(&run_id) {
        entry.status = RunStatus::Cancelled;
    }
    Ok(())
}

/// Re-run a single test from an existing run. Creates a new single-test
/// pending run and returns its ID along with the original suite/distro
/// for the caller to spawn execution.
pub fn rerun_test(state: &AppState, run_id: u64, test_id: &str) -> Result<RerunResult> {
    let entry = state
        .runs
        .get(&run_id)
        .ok_or_else(|| anyhow::anyhow!("run {run_id} not found"))?;

    let _test = entry
        .results
        .iter()
        .find(|r| r.id == test_id)
        .ok_or_else(|| anyhow::anyhow!("test '{test_id}' not found in run {run_id}"))?;

    let phase = entry.phase;
    drop(entry);

    // Get the original run's metadata for distro and suite info.
    let meta = state
        .run_meta
        .get(&run_id)
        .ok_or_else(|| anyhow::anyhow!("metadata for run {run_id} not found"))?;
    let distro = meta.distro.clone();
    let original_suite = meta.suite_name.clone();
    drop(meta);

    let suite_name = format!("rerun-{test_id}");
    let new_run_id = AppState::next_run_id();
    let suite = TestSuite::new(&suite_name, phase);
    state.insert_run(new_run_id, suite);
    state.run_meta.insert(
        new_run_id,
        crate::server::state::RunMeta {
            suite_name: original_suite.clone(),
            distro: distro.clone(),
            phase,
        },
    );

    Ok(RerunResult {
        run_id: new_run_id,
        suite_name: original_suite,
        distro,
        phase,
    })
}

// ---------------------------------------------------------------------------
// Logs / Artifacts
// ---------------------------------------------------------------------------

/// Extract stdout/stderr from all attempts of a test within a run.
pub fn get_test_logs(state: &AppState, run_id: u64, test_id: &str) -> Result<TestLogs> {
    let entry = state
        .runs
        .get(&run_id)
        .ok_or_else(|| anyhow::anyhow!("run {run_id} not found"))?;

    let test = entry
        .results
        .iter()
        .find(|r| r.id == test_id)
        .ok_or_else(|| anyhow::anyhow!("test '{test_id}' not found in run {run_id}"))?;

    let mut attempts: Vec<AttemptLogs> = test
        .attempts
        .iter()
        .map(|a| AttemptLogs {
            attempt: a.attempt,
            stdout: a.stdout.clone(),
            stderr: a.stderr.clone(),
        })
        .collect();

    // If there are no explicit attempts, use the top-level stdout/stderr.
    if attempts.is_empty() {
        attempts.push(AttemptLogs {
            attempt: 1,
            stdout: test.stdout.clone(),
            stderr: test.stderr.clone(),
        });
    }

    Ok(TestLogs {
        test_id: test_id.to_string(),
        attempts,
    })
}

// ---------------------------------------------------------------------------
// Image management / Cleanup
// ---------------------------------------------------------------------------

/// Build a container image for a distro. Returns the image tag.
pub async fn build_image(state: &AppState, distro: &str) -> Result<String> {
    if !state.config.distros.contains_key(distro) {
        bail!("unknown distro: {distro}");
    }

    let backend = crate::container::BollardBackend::new()?;

    let default_name = format!("Containerfile.{distro}");
    let dc = state.config.distros.get(distro).unwrap();
    let filename = dc.containerfile.as_deref().unwrap_or(&default_name);
    let containerfile =
        std::path::PathBuf::from("tests/integration/remi/containers").join(filename);

    crate::container::build_distro_image(&backend, &containerfile, distro).await
}

/// List all available container images.
pub async fn list_images(_state: &AppState) -> Result<Vec<ImageInfo>> {
    let backend = crate::container::BollardBackend::new()?;
    backend.list_images().await
}

/// Clean up stopped conary-test containers.
pub async fn cleanup_containers(_state: &AppState) -> Result<CleanupResult> {
    let docker = bollard::Docker::connect_with_local_defaults()?;

    let mut filters = std::collections::HashMap::new();
    filters.insert("label", vec!["conary-test"]);
    filters.insert("status", vec!["exited", "dead"]);

    let containers = docker
        .list_containers(Some(bollard::container::ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        }))
        .await?;

    let mut removed = 0;
    let mut errors = Vec::new();

    for container in containers {
        let id = match container.id {
            Some(ref id) => id.clone(),
            None => continue,
        };
        match docker
            .remove_container(
                &id,
                Some(bollard::container::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(()) => removed += 1,
            Err(e) => errors.push(format!("failed to remove {}: {e}", &id[..12.min(id.len())])),
        }
    }

    Ok(CleanupResult { removed, errors })
}

/// Return artifact information for a completed run.
pub fn get_run_artifacts(state: &AppState, run_id: u64) -> Result<RunArtifacts> {
    let entry = state
        .runs
        .get(&run_id)
        .ok_or_else(|| anyhow::anyhow!("run {run_id} not found"))?;

    let summary = RunSummary {
        run_id,
        suite: entry.name.clone(),
        phase: entry.phase,
        status: entry.status.as_str().to_string(),
        total: entry.total(),
        passed: entry.passed(),
        failed: entry.failed(),
        skipped: entry.skipped(),
    };

    Ok(RunArtifacts {
        run_id,
        status: entry.status.as_str().to_string(),
        report_path: None, // Reports are generated on demand, not persisted.
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures;

    #[test]
    fn test_start_run_unknown_distro() {
        let state = test_fixtures::test_app_state();
        let result = start_run(&state, "smoke", "nonexistent", 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_start_run_valid_distro() {
        let state = test_fixtures::test_app_state();
        let result = start_run(&state, "smoke", "fedora43", 1).unwrap();
        assert_eq!(result.suite, "smoke");
        assert_eq!(result.distro, "fedora43");

        assert!(state.runs.contains_key(&result.run_id));
    }

    #[test]
    fn test_get_run_not_found() {
        let state = test_fixtures::test_app_state();
        let result = get_run(&state, 9999);
        assert!(result.is_err());
    }

    #[test]
    fn test_list_runs_empty() {
        let state = test_fixtures::test_app_state();
        let runs = list_runs(&state, 20);
        assert!(runs.is_empty());
    }

    #[test]
    fn test_list_distros_returns_configured() {
        let state = test_fixtures::test_app_state();
        let distros = list_distros(&state);
        assert_eq!(distros.len(), 1);
        assert_eq!(distros[0].name, "fedora43");
    }

    #[test]
    fn test_cancel_run_not_found() {
        let state = test_fixtures::test_app_state();
        let result = cancel_run(&state, 9999);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_cancel_run_sets_flag() {
        let state = test_fixtures::test_app_state();
        let run = start_run(&state, "smoke", "fedora43", 1).unwrap();
        let _flag = state.register_cancel_flag(run.run_id);

        cancel_run(&state, run.run_id).unwrap();

        let entry = state.runs.get(&run.run_id).unwrap();
        assert_eq!(entry.status, RunStatus::Cancelled);
    }

    #[test]
    fn test_cancel_run_finished_still_marks_cancelled() {
        use crate::engine::suite::RunStatus;

        let state = test_fixtures::test_app_state();
        let run = start_run(&state, "smoke", "fedora43", 1).unwrap();
        // Don't register a cancel flag (simulates finished run).
        cancel_run(&state, run.run_id).unwrap();
        let entry = state.runs.get(&run.run_id).unwrap();
        assert_eq!(entry.status, RunStatus::Cancelled);
    }

    #[test]
    fn test_rerun_test_not_found_run() {
        let state = test_fixtures::test_app_state();
        let result = rerun_test(&state, 9999, "T01");
        assert!(result.is_err());
    }

    #[test]
    fn test_rerun_test_not_found_test() {
        let state = test_fixtures::test_app_state();
        let run = start_run(&state, "smoke", "fedora43", 1).unwrap();
        let result = rerun_test(&state, run.run_id, "T99");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_rerun_test_creates_new_run() {
        use crate::engine::suite::TestResult;

        let state = test_fixtures::test_app_state();
        let run = start_run(&state, "smoke", "fedora43", 1).unwrap();

        // Record a test result in the original run.
        state.runs.get_mut(&run.run_id).unwrap().record(TestResult {
            id: "T01".to_string(),
            name: "health check".to_string(),
            status: crate::engine::suite::TestStatus::Passed,
            duration_ms: 42,
            message: None,
            stdout: None,
            stderr: None,
            attempts: Vec::new(),
        });

        let rerun = rerun_test(&state, run.run_id, "T01").unwrap();
        assert_ne!(rerun.run_id, run.run_id);
        assert!(state.runs.contains_key(&rerun.run_id));

        let new_suite = state.runs.get(&rerun.run_id).unwrap();
        assert_eq!(new_suite.name, "rerun-T01");
    }

    #[test]
    fn test_get_test_logs_not_found() {
        let state = test_fixtures::test_app_state();
        assert!(get_test_logs(&state, 9999, "T01").is_err());
    }

    #[test]
    fn test_get_test_logs_from_top_level() {
        use crate::engine::suite::TestResult;

        let state = test_fixtures::test_app_state();
        let run = start_run(&state, "smoke", "fedora43", 1).unwrap();

        state.runs.get_mut(&run.run_id).unwrap().record(TestResult {
            id: "T01".to_string(),
            name: "health".to_string(),
            status: crate::engine::suite::TestStatus::Passed,
            duration_ms: 10,
            message: None,
            stdout: Some("hello".to_string()),
            stderr: Some("warn".to_string()),
            attempts: Vec::new(),
        });

        let logs = get_test_logs(&state, run.run_id, "T01").unwrap();
        assert_eq!(logs.test_id, "T01");
        assert_eq!(logs.attempts.len(), 1);
        assert_eq!(logs.attempts[0].stdout.as_deref(), Some("hello"));
        assert_eq!(logs.attempts[0].stderr.as_deref(), Some("warn"));
    }

    #[test]
    fn test_get_test_logs_from_attempts() {
        use crate::engine::suite::{AttemptResult, TestResult, TestStatus};

        let state = test_fixtures::test_app_state();
        let run = start_run(&state, "smoke", "fedora43", 1).unwrap();

        state.runs.get_mut(&run.run_id).unwrap().record(TestResult {
            id: "T01".to_string(),
            name: "flaky".to_string(),
            status: TestStatus::Passed,
            duration_ms: 200,
            message: None,
            stdout: None,
            stderr: None,
            attempts: vec![
                AttemptResult {
                    attempt: 1,
                    status: TestStatus::Failed,
                    message: Some("timeout".to_string()),
                    stdout: Some("attempt1-out".to_string()),
                    stderr: Some("attempt1-err".to_string()),
                    duration_ms: 100,
                },
                AttemptResult {
                    attempt: 2,
                    status: TestStatus::Passed,
                    message: None,
                    stdout: Some("attempt2-out".to_string()),
                    stderr: None,
                    duration_ms: 100,
                },
            ],
        });

        let logs = get_test_logs(&state, run.run_id, "T01").unwrap();
        assert_eq!(logs.attempts.len(), 2);
        assert_eq!(logs.attempts[0].stdout.as_deref(), Some("attempt1-out"));
        assert_eq!(logs.attempts[1].stdout.as_deref(), Some("attempt2-out"));
    }

    #[test]
    fn test_get_run_artifacts_not_found() {
        let state = test_fixtures::test_app_state();
        assert!(get_run_artifacts(&state, 9999).is_err());
    }

    #[test]
    fn test_get_run_artifacts_returns_summary() {
        let state = test_fixtures::test_app_state();
        let run = start_run(&state, "smoke", "fedora43", 1).unwrap();

        let artifacts = get_run_artifacts(&state, run.run_id).unwrap();
        assert_eq!(artifacts.run_id, run.run_id);
        assert_eq!(artifacts.status, "pending");
        assert_eq!(artifacts.summary.suite, "smoke");
    }
}
