// conary-test/src/error.rs

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConaryTestError {
    #[error("container error: {message}")]
    Container {
        message: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("assertion failed in {test_id}: {message}")]
    AssertionFailed { test_id: String, message: String },

    #[error("test timed out after {timeout_secs}s: {test_id}")]
    Timeout { test_id: String, timeout_secs: u64 },

    #[error("config error: {0}")]
    Config(String),

    #[error("manifest error in {file}: {message}")]
    Manifest { file: String, message: String },

    #[error("qemu error: {0}")]
    Qemu(String),

    #[error("mock server error: {0}")]
    MockServer(String),

    #[error("run not found: {0}")]
    RunNotFound(String),

    #[error("test not found: {run_id}/{test_id}")]
    TestNotFound { run_id: String, test_id: String },

    #[error("run cancelled: {0}")]
    Cancelled(String),

    #[error("{0}")]
    Internal(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, ConaryTestError>;

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn container_error_display() {
        let err = ConaryTestError::Container {
            message: "failed to start container".to_string(),
            source: None,
        };
        assert_eq!(err.to_string(), "container error: failed to start container");
    }

    #[test]
    fn assertion_error_includes_test_id() {
        let err = ConaryTestError::AssertionFailed {
            test_id: "T42".to_string(),
            message: "expected foo, got bar".to_string(),
        };
        let display = err.to_string();
        assert!(
            display.contains("T42"),
            "display should contain test_id, got: {display}"
        );
        assert!(
            display.contains("expected foo, got bar"),
            "display should contain message, got: {display}"
        );
    }

    #[test]
    fn internal_wraps_anyhow() {
        let anyhow_err = anyhow!("something went wrong internally");
        let err = ConaryTestError::Internal(anyhow_err);
        assert_eq!(err.to_string(), "something went wrong internally");
    }
}
