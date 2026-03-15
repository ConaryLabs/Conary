// conary-test/src/error_taxonomy.rs
//! Structured error taxonomy for API and MCP responses.
//!
//! Provides machine-parseable error codes, categories, transient flags,
//! and remediation hints so that both humans and LLM agents can
//! programmatically react to failures.

use serde::{Deserialize, Serialize};

/// Structured error response for API and MCP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredError {
    /// Machine-parseable error code (e.g., "test_timeout", "container_failed").
    pub error: String,
    /// Error category.
    pub category: ErrorCategory,
    /// Human-readable message.
    pub message: String,
    /// Whether this error is safe to retry automatically.
    pub transient: bool,
    /// Remediation hint for the caller.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Additional structured details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Error categories that help callers decide how to respond.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// Container/network/OOM failures (usually transient).
    Infrastructure,
    /// Test assertion failure (not transient).
    Assertion,
    /// Bad manifest, missing distro, invalid config (not transient).
    Config,
    /// Build failure, service down (not transient).
    Deployment,
    /// Request validation errors (not transient).
    Validation,
}

// ---------------------------------------------------------------------------
// Builder methods
// ---------------------------------------------------------------------------

impl StructuredError {
    /// Create an infrastructure error (transient by default).
    pub fn infrastructure(error: &str, message: impl Into<String>) -> Self {
        Self {
            error: error.to_string(),
            category: ErrorCategory::Infrastructure,
            message: message.into(),
            transient: true,
            hint: None,
            details: None,
        }
    }

    /// Create an assertion error (not transient).
    pub fn assertion(error: &str, message: impl Into<String>) -> Self {
        Self {
            error: error.to_string(),
            category: ErrorCategory::Assertion,
            message: message.into(),
            transient: false,
            hint: None,
            details: None,
        }
    }

    /// Create a config error (not transient).
    pub fn config(error: &str, message: impl Into<String>) -> Self {
        Self {
            error: error.to_string(),
            category: ErrorCategory::Config,
            message: message.into(),
            transient: false,
            hint: None,
            details: None,
        }
    }

    /// Create a deployment error (not transient).
    pub fn deployment(error: &str, message: impl Into<String>) -> Self {
        Self {
            error: error.to_string(),
            category: ErrorCategory::Deployment,
            message: message.into(),
            transient: false,
            hint: None,
            details: None,
        }
    }

    /// Create a validation error (not transient).
    pub fn validation(error: &str, message: impl Into<String>) -> Self {
        Self {
            error: error.to_string(),
            category: ErrorCategory::Validation,
            message: message.into(),
            transient: false,
            hint: None,
            details: None,
        }
    }

    /// Attach a remediation hint.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Attach additional structured details.
    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

// ---------------------------------------------------------------------------
// Axum integration
// ---------------------------------------------------------------------------

impl axum::response::IntoResponse for StructuredError {
    fn into_response(self) -> axum::response::Response {
        let status = match self.category {
            ErrorCategory::Validation => axum::http::StatusCode::BAD_REQUEST,
            ErrorCategory::Config => axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCategory::Assertion
            | ErrorCategory::Infrastructure
            | ErrorCategory::Deployment => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, axum::Json(self)).into_response()
    }
}

// ---------------------------------------------------------------------------
// Common constructors
// ---------------------------------------------------------------------------

/// Container lifecycle failure.
pub fn container_failed(message: impl Into<String>) -> StructuredError {
    StructuredError::infrastructure("container_failed", message)
        .with_hint("Check Podman status and available disk space")
}

/// Test exceeded its timeout.
pub fn test_timeout(test_id: &str, timeout_secs: u64) -> StructuredError {
    StructuredError::infrastructure(
        "test_timeout",
        format!("Test {test_id} timed out after {timeout_secs}s"),
    )
    .with_hint("Try increasing timeout or reducing concurrency")
    .with_details(serde_json::json!({"test_id": test_id, "timeout_seconds": timeout_secs}))
}

/// Requested distro is not configured.
pub fn unknown_distro(distro: &str) -> StructuredError {
    StructuredError::config(
        "unknown_distro",
        format!("Distro '{distro}' is not configured"),
    )
    .with_hint("Check config.toml [distros] section for available distros")
}

/// Requested suite does not exist.
pub fn unknown_suite(suite: &str) -> StructuredError {
    StructuredError::config("unknown_suite", format!("Suite '{suite}' not found"))
        .with_hint("Run 'conary-test list' to see available suites")
}

/// Run ID not found.
pub fn run_not_found(run_id: u64) -> StructuredError {
    StructuredError::validation("run_not_found", format!("Run {run_id} not found"))
        .with_hint("Use 'list_runs' to see active and completed runs")
}

/// Test ID not found within a run.
pub fn test_not_found(run_id: u64, test_id: &str) -> StructuredError {
    StructuredError::validation(
        "test_not_found",
        format!("Test '{test_id}' not found in run {run_id}"),
    )
    .with_hint("Use 'get_run' to see tests in this run")
}

/// Cargo build failed.
pub fn build_failed(output: &str) -> StructuredError {
    StructuredError::deployment("build_failed", "Cargo build failed")
        .with_details(serde_json::json!({"output": output}))
}

/// Remi server is unreachable.
pub fn remi_unavailable(err: &str) -> StructuredError {
    StructuredError::infrastructure(
        "remi_unavailable",
        format!("Remi server unreachable: {err}"),
    )
    .with_hint("Check REMI_ENDPOINT and REMI_ADMIN_TOKEN environment variables")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infrastructure_error_is_transient() {
        let err = StructuredError::infrastructure("container_oom", "Out of memory");
        assert!(err.transient);
        assert_eq!(err.category, ErrorCategory::Infrastructure);
    }

    #[test]
    fn assertion_error_not_transient() {
        let err = StructuredError::assertion("stdout_mismatch", "Expected 'foo' in stdout");
        assert!(!err.transient);
        assert_eq!(err.category, ErrorCategory::Assertion);
    }

    #[test]
    fn config_error_not_transient() {
        let err = StructuredError::config("bad_toml", "Parse error at line 5");
        assert!(!err.transient);
        assert_eq!(err.category, ErrorCategory::Config);
    }

    #[test]
    fn builder_pattern() {
        let err = test_timeout("T42", 300);
        assert_eq!(err.error, "test_timeout");
        assert!(err.transient);
        assert!(err.hint.is_some());
        assert!(err.details.is_some());

        let details = err.details.as_ref().unwrap();
        assert_eq!(details["test_id"], "T42");
        assert_eq!(details["timeout_seconds"], 300);
    }

    #[test]
    fn serialization_roundtrip() {
        let err = unknown_distro("alpine");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("unknown_distro"));
        assert!(json.contains("config"));
        assert!(json.contains("alpine"));

        let deserialized: StructuredError = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.error, "unknown_distro");
        assert_eq!(deserialized.category, ErrorCategory::Config);
        assert!(!deserialized.transient);
    }

    #[test]
    fn run_not_found_is_validation() {
        let err = run_not_found(42);
        assert_eq!(err.category, ErrorCategory::Validation);
        assert!(!err.transient);
        assert!(err.message.contains("42"));
    }

    #[test]
    fn details_none_skipped_in_json() {
        let err = StructuredError::validation("missing_field", "field 'distro' is required");
        let json = serde_json::to_string(&err).unwrap();
        assert!(!json.contains("details"));
        assert!(!json.contains("hint"));
    }

    #[test]
    fn container_failed_has_hint() {
        let err = container_failed("Podman socket not found");
        assert_eq!(err.error, "container_failed");
        assert!(err.transient);
        assert!(err.hint.as_ref().unwrap().contains("Podman"));
    }

    #[test]
    fn build_failed_includes_output() {
        let err = build_failed("error[E0308]: mismatched types");
        assert_eq!(err.error, "build_failed");
        assert_eq!(err.category, ErrorCategory::Deployment);
        let output = err.details.as_ref().unwrap()["output"].as_str().unwrap();
        assert!(output.contains("E0308"));
    }
}
