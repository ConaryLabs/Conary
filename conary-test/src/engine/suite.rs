// conary-test/src/engine/suite.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Pending,
    Running,
    Completed,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
    Cancelled,
}

/// Result of a single test attempt (for retry tracking).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptResult {
    pub attempt: u32,
    pub status: TestStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub id: String,
    pub name: String,
    pub status: TestStatus,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<AttemptResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestSuite {
    pub name: String,
    pub phase: u32,
    pub status: RunStatus,
    pub results: Vec<TestResult>,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    failed_ids: HashSet<String>,
}

impl TestSuite {
    pub fn new(name: &str, phase: u32) -> Self {
        Self {
            name: name.to_string(),
            phase,
            status: RunStatus::Pending,
            results: Vec::new(),
            started_at: Utc::now(),
            finished_at: None,
            failed_ids: HashSet::new(),
        }
    }

    pub fn record(&mut self, result: TestResult) {
        if matches!(result.status, TestStatus::Failed | TestStatus::Skipped) {
            self.failed_ids.insert(result.id.clone());
        }
        self.results.push(result);
    }

    pub fn has_failed(&self, id: &str) -> bool {
        self.failed_ids.contains(id)
    }

    pub fn should_skip(&self, depends_on: &Option<Vec<String>>) -> bool {
        match depends_on {
            None => false,
            Some(deps) => deps.iter().any(|dep| self.has_failed(dep)),
        }
    }

    pub fn passed(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.status == TestStatus::Passed)
            .count()
    }

    pub fn failed(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.status == TestStatus::Failed)
            .count()
    }

    pub fn skipped(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.status == TestStatus::Skipped)
            .count()
    }

    pub fn cancelled(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.status == TestStatus::Cancelled)
            .count()
    }

    pub fn total(&self) -> usize {
        self.results.len()
    }

    pub fn finish(&mut self) {
        self.status = RunStatus::Completed;
        self.finished_at = Some(Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_status_serializes() {
        let json = serde_json::to_string(&TestStatus::Cancelled).unwrap();
        assert_eq!(json, r#""cancelled""#);
    }

    #[test]
    fn cancelled_status_deserializes() {
        let status: TestStatus = serde_json::from_str(r#""cancelled""#).unwrap();
        assert_eq!(status, TestStatus::Cancelled);
    }

    #[test]
    fn test_status_round_trips() {
        for status in [
            TestStatus::Passed,
            TestStatus::Failed,
            TestStatus::Skipped,
            TestStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: TestStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn attempt_result_round_trips() {
        let attempt = AttemptResult {
            attempt: 2,
            status: TestStatus::Failed,
            message: Some("connection timeout".to_string()),
            stdout: Some("partial output".to_string()),
            stderr: None,
            duration_ms: 1500,
        };

        let json = serde_json::to_string(&attempt).unwrap();
        let back: AttemptResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.attempt, 2);
        assert_eq!(back.status, TestStatus::Failed);
        assert_eq!(back.message.as_deref(), Some("connection timeout"));
        assert_eq!(back.stdout.as_deref(), Some("partial output"));
        assert!(back.stderr.is_none());
        assert_eq!(back.duration_ms, 1500);
    }

    #[test]
    fn attempt_result_skips_none_fields() {
        let attempt = AttemptResult {
            attempt: 1,
            status: TestStatus::Passed,
            message: None,
            stdout: None,
            stderr: None,
            duration_ms: 50,
        };
        let json = serde_json::to_string(&attempt).unwrap();
        assert!(!json.contains("message"));
        assert!(!json.contains("stdout"));
        assert!(!json.contains("stderr"));
    }

    #[test]
    fn cancelled_count_works() {
        let mut suite = TestSuite::new("cancel-test", 1);
        suite.record(TestResult {
            id: "T01".to_string(),
            name: "passed".to_string(),
            status: TestStatus::Passed,
            duration_ms: 10,
            message: None,
            stdout: None,
            stderr: None,
            attempts: Vec::new(),
        });
        suite.record(TestResult {
            id: "T02".to_string(),
            name: "cancelled".to_string(),
            status: TestStatus::Cancelled,
            duration_ms: 0,
            message: Some("cancelled by user".to_string()),
            stdout: None,
            stderr: None,
            attempts: Vec::new(),
        });
        assert_eq!(suite.cancelled(), 1);
        assert_eq!(suite.passed(), 1);
        assert_eq!(suite.total(), 2);
    }

    #[test]
    fn test_result_with_attempts_serializes() {
        let result = TestResult {
            id: "T01".to_string(),
            name: "flaky".to_string(),
            status: TestStatus::Passed,
            duration_ms: 300,
            message: None,
            stdout: None,
            stderr: None,
            attempts: vec![
                AttemptResult {
                    attempt: 1,
                    status: TestStatus::Failed,
                    message: Some("timeout".to_string()),
                    stdout: None,
                    stderr: None,
                    duration_ms: 200,
                },
                AttemptResult {
                    attempt: 2,
                    status: TestStatus::Passed,
                    message: None,
                    stdout: None,
                    stderr: None,
                    duration_ms: 100,
                },
            ],
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("attempts"));
        assert!(json.contains(r#""attempt":1"#));
        assert!(json.contains(r#""attempt":2"#));
    }

    #[test]
    fn test_result_empty_attempts_not_serialized() {
        let result = TestResult {
            id: "T01".to_string(),
            name: "simple".to_string(),
            status: TestStatus::Passed,
            duration_ms: 42,
            message: None,
            stdout: None,
            stderr: None,
            attempts: Vec::new(),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("attempts"));
    }
}
