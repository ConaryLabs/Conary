// conary-test/src/server/service/tests.rs

use super::*;
use crate::server::wal::Wal;
use crate::test_fixtures;
use chrono::{TimeZone, Utc};
use std::sync::{Arc, Mutex};

#[test]
fn test_start_run_unknown_distro() {
    let state = test_fixtures::test_app_state();
    let result = start_run(&state, "smoke", "nonexistent", 1);
    assert!(result.is_err());
}

#[test]
fn test_start_run_valid_distro() {
    let state = test_fixtures::test_app_state();
    let result = start_run(&state, "smoke", "fedora44", 1).unwrap();
    assert_eq!(result.suite, "smoke");
    assert_eq!(result.distro, "fedora44");

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
    assert_eq!(distros[0].name, "fedora44");
}

#[test]
fn test_deployment_status_reports_binary_runtime_and_service_sections() {
    let mut state = test_fixtures::test_app_state();
    state.start_time = Utc.with_ymd_and_hms(2026, 4, 9, 0, 0, 0).unwrap();

    let wal = Wal::open(":memory:").unwrap();
    wal.buffer(1, r#"{"test_id":"T01"}"#).unwrap();
    state.wal = Some(Arc::new(Mutex::new(wal)));

    let now = Utc.with_ymd_and_hms(2026, 4, 9, 1, 1, 1).unwrap();
    let status = deployment_status_at(&state, now).unwrap();

    assert_eq!(
        status.binary.version,
        crate::build_info::BuildInfo::current().version
    );
    assert_eq!(
        status.binary.git_commit,
        crate::build_info::BuildInfo::current().git_commit
    );
    assert_eq!(status.runtime.started_at, "2026-04-09T00:00:00+00:00");
    assert_eq!(status.runtime.uptime_seconds, 3661);
    assert_eq!(status.runtime.uptime_human, "0d 1h 1m 1s");
    assert_eq!(status.runtime.wal_pending, 1);
    assert_eq!(status.runtime.active_runs, 0);
    assert_eq!(status.service.status, "running");
}

#[test]
fn test_deployment_status_counts_only_pending_and_running_runs() {
    let state = test_fixtures::test_app_state();

    let pending = crate::engine::suite::TestSuite::new("pending", 1);

    let mut running = crate::engine::suite::TestSuite::new("running", 1);
    running.status = RunStatus::Running;

    let mut completed = crate::engine::suite::TestSuite::new("completed", 1);
    completed.status = RunStatus::Completed;

    let mut cancelled = crate::engine::suite::TestSuite::new("cancelled", 1);
    cancelled.status = RunStatus::Cancelled;

    state.insert_run(1, pending);
    state.insert_run(2, running);
    state.insert_run(3, completed);
    state.insert_run(4, cancelled);

    let status =
        deployment_status_at(&state, Utc.with_ymd_and_hms(2026, 4, 9, 0, 0, 1).unwrap()).unwrap();

    assert_eq!(status.runtime.active_runs, 2);
}

#[test]
fn test_deployment_status_degrades_when_wal_lock_is_poisoned() {
    let mut state = test_fixtures::test_app_state();
    let wal = Arc::new(Mutex::new(Wal::open(":memory:").unwrap()));
    let wal_for_poison = Arc::clone(&wal);

    let _ = std::thread::spawn(move || {
        let _guard = wal_for_poison.lock().unwrap();
        panic!("poison wal");
    })
    .join();

    state.wal = Some(wal);

    let status =
        deployment_status_at(&state, Utc.with_ymd_and_hms(2026, 4, 9, 0, 0, 1).unwrap()).unwrap();

    assert_eq!(status.runtime.wal_pending, 0);
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
    let run = start_run(&state, "smoke", "fedora44", 1).unwrap();
    // start_run already registers the cancel flag.

    cancel_run(&state, run.run_id).unwrap();

    let entry = state.runs.get(&run.run_id).unwrap();
    assert_eq!(entry.status, RunStatus::Cancelled);
}

#[test]
fn test_cancel_run_finished_preserves_status() {
    use crate::engine::suite::RunStatus;

    let state = test_fixtures::test_app_state();
    let run = start_run(&state, "smoke", "fedora44", 1).unwrap();
    // Simulate a finished run by removing the cancel flag (as
    // execute_run does via remove_cancel_flag on completion).
    state.remove_cancel_flag(run.run_id);

    // cancel_run should succeed but NOT rewrite the status.
    cancel_run(&state, run.run_id).unwrap();
    let entry = state.runs.get(&run.run_id).unwrap();
    // Status remains Pending (unchanged) rather than being overwritten to Cancelled.
    assert_eq!(entry.status, RunStatus::Pending);
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
    let run = start_run(&state, "smoke", "fedora44", 1).unwrap();
    let result = rerun_test(&state, run.run_id, "T99");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[test]
fn test_rerun_test_creates_new_run() {
    use crate::engine::suite::TestResult;

    let state = test_fixtures::test_app_state();
    let run = start_run(&state, "smoke", "fedora44", 1).unwrap();

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
    let run = start_run(&state, "smoke", "fedora44", 1).unwrap();

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
    let run = start_run(&state, "smoke", "fedora44", 1).unwrap();

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
    let run = start_run(&state, "smoke", "fedora44", 1).unwrap();

    let artifacts = get_run_artifacts(&state, run.run_id).unwrap();
    assert_eq!(artifacts.run_id, run.run_id);
    assert_eq!(artifacts.status, "pending");
    assert_eq!(artifacts.summary.suite, "smoke");
}
