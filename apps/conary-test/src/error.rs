// conary-test/src/error.rs

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConaryTestError {
    #[error("container error: {message}")]
    Container {
        message: String,
        #[source]
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
        assert_eq!(
            err.to_string(),
            "container error: failed to start container"
        );
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

    #[test]
    fn run_not_found_display() {
        let err = ConaryTestError::RunNotFound("42".to_string());
        assert_eq!(err.to_string(), "run not found: 42");
    }

    #[test]
    fn timeout_display() {
        let err = ConaryTestError::Timeout {
            test_id: "T05".to_string(),
            timeout_secs: 30,
        };
        let display = err.to_string();
        assert!(
            display.contains("T05"),
            "should contain test_id, got: {display}"
        );
        assert!(
            display.contains("30"),
            "should contain timeout_secs, got: {display}"
        );
        assert_eq!(display, "test timed out after 30s: T05");
    }

    #[test]
    fn cancelled_display() {
        let err = ConaryTestError::Cancelled("user requested stop".to_string());
        assert_eq!(err.to_string(), "run cancelled: user requested stop");
    }

    #[test]
    fn test_not_found_display() {
        let err = ConaryTestError::TestNotFound {
            run_id: "7".to_string(),
            test_id: "T99".to_string(),
        };
        assert_eq!(err.to_string(), "test not found: 7/T99");
    }

    #[test]
    fn manifest_error_display() {
        let err = ConaryTestError::Manifest {
            file: "phase1-core.toml".to_string(),
            message: "missing suite section".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains("phase1-core.toml"));
        assert!(display.contains("missing suite section"));
    }

    #[test]
    fn config_error_display() {
        let err = ConaryTestError::Config("bad endpoint URL".to_string());
        assert_eq!(err.to_string(), "config error: bad endpoint URL");
    }

    #[test]
    fn container_error_with_source() {
        let source = std::io::Error::new(std::io::ErrorKind::NotFound, "socket missing");
        let err = ConaryTestError::Container {
            message: "connection refused".to_string(),
            source: Some(Box::new(source)),
        };
        assert_eq!(err.to_string(), "container error: connection refused");
    }
}
