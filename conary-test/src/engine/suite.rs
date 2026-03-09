// conary-test/src/engine/suite.rs

use chrono::{DateTime, Utc};
use serde::Serialize;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
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
        if result.status == TestStatus::Failed {
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

    pub fn total(&self) -> usize {
        self.results.len()
    }

    pub fn finish(&mut self) {
        self.status = RunStatus::Completed;
        self.finished_at = Some(Utc::now());
    }
}
