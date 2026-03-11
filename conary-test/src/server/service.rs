// conary-test/src/server/service.rs
//! Shared business logic for the HTTP and MCP interfaces.
//!
//! Handlers and MCP tools are thin wrappers that delegate to these
//! functions, converting the results into their respective response types.

use anyhow::{bail, Result};
use serde::Serialize;

use crate::config::load_manifest;
use crate::engine::suite::TestSuite;
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
pub async fn start_run(
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
    state.insert_run(run_id, suite).await;

    Ok(StartRunResult {
        run_id,
        suite: suite_name.to_string(),
        distro: distro.to_string(),
        phase,
    })
}

/// Retrieve a run's full report as a JSON value.
pub async fn get_run(state: &AppState, run_id: u64) -> Result<serde_json::Value> {
    let runs = state.runs.read().await;
    match runs.get(&run_id) {
        Some(suite) => to_json_value(suite),
        None => bail!("run {run_id} not found"),
    }
}

/// List runs with summary information, sorted by run ID descending.
pub async fn list_runs(state: &AppState, limit: usize) -> Vec<RunSummary> {
    let runs = state.runs.read().await;
    let mut summaries: Vec<RunSummary> = runs
        .iter()
        .map(|(&id, suite)| RunSummary {
            run_id: id,
            suite: suite.name.clone(),
            phase: suite.phase,
            status: suite.status.as_str().to_string(),
            total: suite.total(),
            passed: suite.passed(),
            failed: suite.failed(),
            skipped: suite.skipped(),
        })
        .collect();

    summaries.sort_by(|a, b| b.run_id.cmp(&a.run_id));
    summaries.truncate(limit);
    summaries
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures;

    #[tokio::test]
    async fn test_start_run_unknown_distro() {
        let state = test_fixtures::test_app_state();
        let result = start_run(&state, "smoke", "nonexistent", 1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_start_run_valid_distro() {
        let state = test_fixtures::test_app_state();
        let result = start_run(&state, "smoke", "fedora43", 1).await.unwrap();
        assert_eq!(result.suite, "smoke");
        assert_eq!(result.distro, "fedora43");

        let runs = state.runs.read().await;
        assert!(runs.contains_key(&result.run_id));
    }

    #[tokio::test]
    async fn test_get_run_not_found() {
        let state = test_fixtures::test_app_state();
        let result = get_run(&state, 9999).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_runs_empty() {
        let state = test_fixtures::test_app_state();
        let runs = list_runs(&state, 20).await;
        assert!(runs.is_empty());
    }

    #[test]
    fn test_list_distros_returns_configured() {
        let state = test_fixtures::test_app_state();
        let distros = list_distros(&state);
        assert_eq!(distros.len(), 1);
        assert_eq!(distros[0].name, "fedora43");
    }
}
