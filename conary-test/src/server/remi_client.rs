// conary-test/src/server/remi_client.rs
//! HTTP client for pushing test results to the Remi admin API.
//!
//! Configured via environment variables:
//! - `REMI_ADMIN_ENDPOINT` -- base URL for the admin REST API
//! - `REMI_ADMIN_TOKEN` -- bearer token for the admin API

use anyhow::{Context, Result, bail};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// HTTP request timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Async HTTP client for the Remi admin API.
///
/// Handles creating test runs, pushing results with steps/logs, updating
/// run status, and proxying read queries. All requests use bearer token
/// authentication.
#[derive(Debug)]
pub struct RemiClient {
    client: reqwest::Client,
    base_url: String,
    #[allow(dead_code)]
    // Injected into default headers at construction; stored for potential re-auth
    token: String,
}

impl RemiClient {
    /// Construct a `RemiClient` from environment variables.
    ///
    /// Reads `REMI_ADMIN_ENDPOINT` and `REMI_ADMIN_TOKEN`.
    pub fn from_env() -> Result<Self> {
        let token = std::env::var("REMI_ADMIN_TOKEN")
            .context("REMI_ADMIN_TOKEN environment variable is required")?;
        let base_url = std::env::var("REMI_ADMIN_ENDPOINT").context(
            "REMI_ADMIN_ENDPOINT environment variable is required for Remi admin API access",
        )?;
        Ok(Self::new(base_url, token))
    }

    /// Construct a `RemiClient` with explicit configuration.
    pub fn new(base_url: String, token: String) -> Self {
        let mut headers = HeaderMap::new();
        // Safe: token is ASCII (bearer tokens are always ASCII-safe).
        if let Ok(val) = HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(AUTHORIZATION, val);
        }

        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .default_headers(headers)
            .build()
            .expect("failed to build reqwest client");

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
        }
    }

    // -------------------------------------------------------------------
    // Write operations
    // -------------------------------------------------------------------

    /// Create a new test run on Remi. Returns the run ID.
    pub async fn create_run(
        &self,
        suite: &str,
        distro: &str,
        phase: u32,
        triggered_by: Option<&str>,
        source_commit: Option<&str>,
    ) -> Result<i64> {
        let url = format!("{}/v1/admin/test-runs", self.base_url);
        let body = serde_json::json!({
            "suite": suite,
            "distro": distro,
            "phase": phase,
            "triggered_by": triggered_by,
            "source_commit": source_commit,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("failed to POST /v1/admin/test-runs")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("create_run failed (HTTP {status}): {text}");
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .context("failed to parse create_run response")?;

        data["id"]
            .as_i64()
            .context("create_run response missing 'id' field")
    }

    /// Update run status and aggregate counts.
    pub async fn update_run(
        &self,
        run_id: i64,
        status: &str,
        total: u32,
        passed: u32,
        failed: u32,
        skipped: u32,
    ) -> Result<()> {
        let url = format!("{}/v1/admin/test-runs/{run_id}", self.base_url);
        let body = serde_json::json!({
            "status": status,
            "total": total,
            "passed": passed,
            "failed": failed,
            "skipped": skipped,
        });

        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("failed to PUT /v1/admin/test-runs/{run_id}"))?;

        let status_code = resp.status();
        if !status_code.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("update_run failed (HTTP {status_code}): {text}");
        }

        Ok(())
    }

    /// Push a test result with steps and logs.
    pub async fn push_result(&self, run_id: i64, data: &PushResultData) -> Result<()> {
        let url = format!("{}/v1/admin/test-runs/{run_id}/results", self.base_url);

        let resp = self
            .client
            .post(&url)
            .json(data)
            .send()
            .await
            .with_context(|| format!("failed to POST /v1/admin/test-runs/{run_id}/results"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("push_result failed (HTTP {status}): {text}");
        }

        Ok(())
    }

    // -------------------------------------------------------------------
    // Read (proxy) operations
    // -------------------------------------------------------------------

    /// List test runs with optional filters and cursor-based pagination.
    pub async fn list_runs(
        &self,
        limit: u32,
        cursor: Option<i64>,
        suite: Option<&str>,
        distro: Option<&str>,
        status: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut url = format!("{}/v1/admin/test-runs?limit={limit}", self.base_url);
        if let Some(c) = cursor {
            url.push_str(&format!("&cursor={c}"));
        }
        if let Some(s) = suite {
            url.push_str(&format!("&suite={s}"));
        }
        if let Some(d) = distro {
            url.push_str(&format!("&distro={d}"));
        }
        if let Some(st) = status {
            url.push_str(&format!("&status={st}"));
        }

        self.get_json(&url).await
    }

    /// Get a test run with all its results.
    pub async fn get_run(&self, run_id: i64) -> Result<serde_json::Value> {
        let url = format!("{}/v1/admin/test-runs/{run_id}", self.base_url);
        self.get_json(&url).await
    }

    /// Get a single test result with its steps and logs.
    pub async fn get_test(&self, run_id: i64, test_id: &str) -> Result<serde_json::Value> {
        let url = format!(
            "{}/v1/admin/test-runs/{run_id}/tests/{test_id}",
            self.base_url
        );
        self.get_json(&url).await
    }

    /// Get log entries for a specific test, optionally filtered by stream
    /// or step index.
    pub async fn get_logs(
        &self,
        run_id: i64,
        test_id: &str,
        stream: Option<&str>,
        step_index: Option<u32>,
    ) -> Result<serde_json::Value> {
        let mut url = format!(
            "{}/v1/admin/test-runs/{run_id}/tests/{test_id}/logs",
            self.base_url
        );

        let mut has_query = false;
        if let Some(s) = stream {
            url.push_str(&format!("?stream={s}"));
            has_query = true;
        }
        if let Some(idx) = step_index {
            let sep = if has_query { '&' } else { '?' };
            url.push_str(&format!("{sep}step_index={idx}"));
        }

        self.get_json(&url).await
    }

    /// Get a health summary of recent test activity.
    pub async fn health(&self) -> Result<serde_json::Value> {
        let url = format!("{}/v1/admin/test-health", self.base_url);
        self.get_json(&url).await
    }

    // -------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------

    /// Send a GET request and parse the JSON response.
    async fn get_json(&self, url: &str) -> Result<serde_json::Value> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to GET {url}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("GET {url} failed (HTTP {status}): {text}");
        }

        resp.json()
            .await
            .with_context(|| format!("failed to parse JSON from {url}"))
    }
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Data for pushing a test result to Remi.
///
/// Matches the `PushTestResultData` type on the Remi server side
/// (see `conary-server/src/server/admin_service.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushResultData {
    pub test_id: String,
    pub name: String,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub message: Option<String>,
    pub attempt: Option<i32>,
    pub steps: Vec<PushStepData>,
}

/// A single step within a test result.
///
/// Matches the `PushStepData` type on the Remi server side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushStepData {
    pub step_type: String,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{LazyLock, Mutex, MutexGuard};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct EnvVarGuard {
        _lock: MutexGuard<'static, ()>,
        admin_token: Option<OsString>,
        admin_endpoint: Option<OsString>,
        legacy_endpoint: Option<OsString>,
    }

    impl EnvVarGuard {
        fn new() -> Self {
            Self {
                _lock: ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
                admin_token: std::env::var_os("REMI_ADMIN_TOKEN"),
                admin_endpoint: std::env::var_os("REMI_ADMIN_ENDPOINT"),
                legacy_endpoint: std::env::var_os("REMI_ENDPOINT"),
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.admin_token {
                    Some(value) => std::env::set_var("REMI_ADMIN_TOKEN", value),
                    None => std::env::remove_var("REMI_ADMIN_TOKEN"),
                }
                match &self.admin_endpoint {
                    Some(value) => std::env::set_var("REMI_ADMIN_ENDPOINT", value),
                    None => std::env::remove_var("REMI_ADMIN_ENDPOINT"),
                }
                match &self.legacy_endpoint {
                    Some(value) => std::env::set_var("REMI_ENDPOINT", value),
                    None => std::env::remove_var("REMI_ENDPOINT"),
                }
            }
        }
    }

    #[test]
    fn new_trims_trailing_slash() {
        let client = RemiClient::new("https://example.com/".to_string(), "tok".to_string());
        assert_eq!(client.base_url, "https://example.com");
    }

    #[test]
    fn new_preserves_clean_url() {
        let client = RemiClient::new("https://example.com".to_string(), "tok".to_string());
        assert_eq!(client.base_url, "https://example.com");
    }

    #[test]
    fn from_env_requires_token() {
        let _env_guard = EnvVarGuard::new();
        // Clear the env var to ensure it is not set.
        unsafe {
            std::env::remove_var("REMI_ADMIN_TOKEN");
            std::env::remove_var("REMI_ADMIN_ENDPOINT");
        }
        let result = RemiClient::from_env();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("REMI_ADMIN_TOKEN"),);
    }

    #[test]
    fn from_env_requires_admin_endpoint() {
        let _env_guard = EnvVarGuard::new();
        unsafe {
            std::env::set_var("REMI_ADMIN_TOKEN", "tok");
            std::env::remove_var("REMI_ADMIN_ENDPOINT");
            std::env::remove_var("REMI_ENDPOINT");
        }
        let result = RemiClient::from_env();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("REMI_ADMIN_ENDPOINT")
        );
    }

    #[test]
    fn from_env_uses_admin_endpoint() {
        let _env_guard = EnvVarGuard::new();
        unsafe {
            std::env::set_var("REMI_ADMIN_TOKEN", "tok");
            std::env::set_var("REMI_ADMIN_ENDPOINT", "https://admin.example.com/");
            std::env::remove_var("REMI_ENDPOINT");
        }
        let client = RemiClient::from_env().unwrap();
        assert_eq!(client.base_url, "https://admin.example.com");
    }

    #[test]
    fn push_result_data_serializes() {
        let data = PushResultData {
            test_id: "T01".to_string(),
            name: "health_check".to_string(),
            status: "passed".to_string(),
            duration_ms: Some(150),
            message: None,
            attempt: Some(1),
            steps: vec![PushStepData {
                step_type: "exec".to_string(),
                command: Some("conary --version".to_string()),
                exit_code: Some(0),
                duration_ms: Some(42),
                stdout: Some("conary 0.5.0".to_string()),
                stderr: None,
            }],
        };

        let json = serde_json::to_value(&data).unwrap();
        assert_eq!(json["test_id"], "T01");
        assert_eq!(json["steps"][0]["step_type"], "exec");
        assert_eq!(json["steps"][0]["exit_code"], 0);
    }

    #[test]
    fn push_result_data_deserializes() {
        let json = serde_json::json!({
            "test_id": "T05",
            "name": "install_test",
            "status": "failed",
            "duration_ms": 3000,
            "message": "exit code 1",
            "attempt": 2,
            "steps": [{
                "step_type": "exec",
                "command": "conary install foo",
                "exit_code": 1,
                "duration_ms": 2900,
                "stdout": "",
                "stderr": "package not found"
            }]
        });

        let data: PushResultData = serde_json::from_value(json).unwrap();
        assert_eq!(data.test_id, "T05");
        assert_eq!(data.status, "failed");
        assert_eq!(data.attempt, Some(2));
        assert_eq!(data.steps.len(), 1);
        assert_eq!(data.steps[0].exit_code, Some(1));
    }

    #[test]
    fn push_step_data_optional_fields() {
        let step = PushStepData {
            step_type: "assert".to_string(),
            command: None,
            exit_code: None,
            duration_ms: None,
            stdout: None,
            stderr: None,
        };

        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json["step_type"], "assert");
        assert!(json["command"].is_null());
    }
}
