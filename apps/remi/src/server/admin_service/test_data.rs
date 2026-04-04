// apps/remi/src/server/admin_service/test_data.rs

//! Test-data DTOs and service operations for Remi admin APIs.

use super::{NotFoundError, ServiceError, blocking_anyhow, test_db_path};
use crate::server::ServerState;
use crate::server::test_db;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Input for pushing a test result with its steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushTestResultData {
    pub test_id: String,
    pub name: String,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub message: Option<String>,
    pub attempt: Option<i32>,
    pub steps: Vec<PushStepData>,
}

/// A single step within a [`PushTestResultData`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushStepData {
    pub step_type: String,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

/// A test run together with all its results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRunDetail {
    pub run: test_db::TestRun,
    pub results: Vec<test_db::TestResult>,
}

/// A single test result together with its steps and logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDetail {
    pub result: test_db::TestResult,
    pub steps: Vec<TestStepWithLogs>,
}

/// A test step paired with its log entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestStepWithLogs {
    pub step: test_db::TestStep,
    pub logs: Vec<test_db::TestLog>,
}

/// Summary returned by [`test_health`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestHealthSummary {
    pub total_runs: u64,
    pub recent_runs: Vec<test_db::TestRun>,
    pub last_status: Option<String>,
}

/// Create a new test run in the test data database.
pub async fn create_test_run(
    state: &Arc<RwLock<ServerState>>,
    suite: String,
    distro: String,
    phase: u32,
    triggered_by: Option<String>,
    source_commit: Option<String>,
) -> Result<test_db::TestRun, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        test_db::TestRun::create(
            &conn,
            &suite,
            &distro,
            i32::try_from(phase).unwrap_or(i32::MAX),
            triggered_by.as_deref(),
            source_commit.as_deref(),
        )
    })
    .await
}

/// Update the status (and optionally the aggregate counts) of a test run.
pub async fn update_test_run_status(
    state: &Arc<RwLock<ServerState>>,
    run_id: i64,
    status: String,
    total: Option<u32>,
    passed: Option<u32>,
    failed: Option<u32>,
    skipped: Option<u32>,
) -> Result<(), ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        test_db::TestRun::update_status(&conn, run_id, &status)?;
        if let Some(t) = total {
            test_db::TestRun::update_counts(
                &conn,
                run_id,
                i32::try_from(t).unwrap_or(i32::MAX),
                i32::try_from(passed.unwrap_or(0)).unwrap_or(0),
                i32::try_from(failed.unwrap_or(0)).unwrap_or(0),
                i32::try_from(skipped.unwrap_or(0)).unwrap_or(0),
            )?;
        }
        Ok(())
    })
    .await
}

/// Push a test result (with steps and logs) into an existing run.
pub async fn push_test_result(
    state: &Arc<RwLock<ServerState>>,
    run_id: i64,
    data: PushTestResultData,
) -> Result<(), ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;

        test_db::TestRun::find_by_id(&conn, run_id)?
            .ok_or_else(|| anyhow::anyhow!("test run {run_id} not found"))?;

        let result = test_db::TestResult::insert(
            &conn,
            &test_db::NewTestResult {
                run_id,
                test_id: &data.test_id,
                name: &data.name,
                status: &data.status,
                duration_ms: data.duration_ms,
                message: data.message.as_deref(),
                attempt: data.attempt.unwrap_or(1),
            },
        )?;

        for (idx, step_data) in data.steps.iter().enumerate() {
            let step = test_db::TestStep::insert(
                &conn,
                result.id,
                i32::try_from(idx).unwrap_or(i32::MAX),
                &step_data.step_type,
                step_data.command.as_deref(),
                step_data.exit_code,
                step_data.duration_ms,
            )?;

            if let Some(ref stdout) = step_data.stdout {
                test_db::TestLog::insert(&conn, step.id, "stdout", stdout)?;
            }
            if let Some(ref stderr) = step_data.stderr {
                test_db::TestLog::insert(&conn, step.id, "stderr", stderr)?;
            }
        }

        Ok(())
    })
    .await
}

/// List test runs with optional filters and cursor-based pagination.
pub async fn list_test_runs(
    state: &Arc<RwLock<ServerState>>,
    limit: u32,
    cursor: Option<i64>,
    suite: Option<String>,
    distro: Option<String>,
    status: Option<String>,
) -> Result<Vec<test_db::TestRun>, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        test_db::TestRun::list_filtered(
            &conn,
            cursor,
            limit,
            suite.as_deref(),
            distro.as_deref(),
            status.as_deref(),
        )
    })
    .await
}

/// Get a test run with all its results.
pub async fn get_test_run_detail(
    state: &Arc<RwLock<ServerState>>,
    run_id: i64,
) -> Result<TestRunDetail, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        let run = test_db::TestRun::find_by_id(&conn, run_id)?
            .ok_or_else(|| NotFoundError(format!("test run {run_id} not found")))?;
        let results = test_db::TestResult::find_by_run(&conn, run_id)?;
        Ok(TestRunDetail { run, results })
    })
    .await
}

/// Get a single test result with its steps and logs.
pub async fn get_test_detail(
    state: &Arc<RwLock<ServerState>>,
    run_id: i64,
    test_id: String,
) -> Result<TestDetail, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        let result = test_db::TestResult::find_by_run_and_test(&conn, run_id, &test_id)?
            .ok_or_else(|| NotFoundError(format!("test {test_id} not found in run {run_id}")))?;

        let steps = test_db::TestStep::find_by_result(&conn, result.id)?;
        let mut steps_with_logs = Vec::with_capacity(steps.len());
        for step in steps {
            let logs = test_db::TestLog::find_by_step(&conn, step.id)?;
            steps_with_logs.push(TestStepWithLogs { step, logs });
        }

        Ok(TestDetail {
            result,
            steps: steps_with_logs,
        })
    })
    .await
}

/// Get log entries for a specific test, optionally filtered by stream or step.
pub async fn get_test_logs(
    state: &Arc<RwLock<ServerState>>,
    run_id: i64,
    test_id: String,
    stream: Option<String>,
    step_index: Option<u32>,
) -> Result<Vec<test_db::TestLog>, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        let result = test_db::TestResult::find_by_run_and_test(&conn, run_id, &test_id)?
            .ok_or_else(|| NotFoundError(format!("test {test_id} not found in run {run_id}")))?;

        let steps = test_db::TestStep::find_by_result(&conn, result.id)?;
        let mut all_logs = Vec::new();

        for step in &steps {
            if let Some(idx) = step_index
                && step.step_index != i32::try_from(idx).unwrap_or(i32::MAX)
            {
                continue;
            }
            all_logs.extend(test_db::TestLog::find_by_step(&conn, step.id)?);
        }

        if let Some(ref stream_name) = stream {
            all_logs.retain(|log| log.stream == *stream_name);
        }

        Ok(all_logs)
    })
    .await
}

/// Return a health summary of recent test activity.
pub async fn test_health(
    state: &Arc<RwLock<ServerState>>,
) -> Result<TestHealthSummary, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        let recent_runs = test_db::TestRun::list(&conn, None, 5)?;
        let total_runs: u64 = conn
            .query_row("SELECT COUNT(*) FROM test_runs", [], |r| {
                r.get::<_, i64>(0).map(|v| v as u64)
            })
            .map_err(|e| anyhow::anyhow!("Failed to count test runs: {e}"))?;
        let last_status = recent_runs.first().map(|run| run.status.clone());

        Ok(TestHealthSummary {
            total_runs,
            recent_runs,
            last_status,
        })
    })
    .await
}

/// Delete test runs older than `older_than_days` days. Returns the number removed.
pub async fn test_gc(
    state: &Arc<RwLock<ServerState>>,
    older_than_days: u32,
) -> Result<u64, ServiceError> {
    let db = test_db_path(state).await?;
    blocking_anyhow(move || {
        let conn = test_db::init(&db)?;
        test_db::gc(&conn, older_than_days)
    })
    .await
}
