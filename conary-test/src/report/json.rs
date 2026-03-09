// conary-test/src/report/json.rs

use crate::engine::suite::TestSuite;
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct JsonReport<'a> {
    suite_name: &'a str,
    phase: u32,
    status: &'a str,
    summary: Summary,
    results: &'a [crate::engine::suite::TestResult],
}

#[derive(Serialize)]
struct Summary {
    total: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
}

/// Serialize a test suite to a JSON string.
pub fn to_json_report(suite: &TestSuite) -> Result<String> {
    let status_str = match suite.status {
        crate::engine::suite::RunStatus::Pending => "pending",
        crate::engine::suite::RunStatus::Running => "running",
        crate::engine::suite::RunStatus::Completed => "completed",
        crate::engine::suite::RunStatus::Cancelled => "cancelled",
    };
    let report = JsonReport {
        suite_name: &suite.name,
        phase: suite.phase,
        status: status_str,
        summary: Summary {
            total: suite.total(),
            passed: suite.passed(),
            failed: suite.failed(),
            skipped: suite.skipped(),
        },
        results: &suite.results,
    };
    Ok(serde_json::to_string_pretty(&report)?)
}

/// Write JSON report to a file.
pub fn write_json_report(suite: &TestSuite, path: &Path) -> Result<()> {
    let json = to_json_report(suite)?;
    std::fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::suite::{TestResult, TestStatus, TestSuite};

    #[test]
    fn test_json_report_format() {
        let mut suite = TestSuite::new("integration", 1);
        suite.record(TestResult {
            id: "T01".to_string(),
            name: "health_check".to_string(),
            status: TestStatus::Passed,
            duration_ms: 42,
            message: None,
            stdout: None,
            stderr: None,
        });
        suite.finish();

        let json = to_json_report(&suite).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["suite_name"], "integration");
        assert_eq!(parsed["phase"], 1);
        assert_eq!(parsed["status"], "completed");
        assert_eq!(parsed["summary"]["total"], 1);
        assert_eq!(parsed["summary"]["passed"], 1);
        assert_eq!(parsed["summary"]["failed"], 0);
        assert_eq!(parsed["summary"]["skipped"], 0);
        assert_eq!(parsed["results"][0]["id"], "T01");
        assert_eq!(parsed["results"][0]["status"], "passed");
    }
}
