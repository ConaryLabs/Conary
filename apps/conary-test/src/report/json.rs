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
    #[serde(skip_serializing_if = "Option::is_none")]
    corpus: Option<crate::report::corpus::CorpusReport<'a>>,
}

#[derive(Serialize)]
struct Summary {
    total: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
    cancelled: usize,
}

/// Serialize a test suite to a JSON string.
pub fn to_json_report(suite: &TestSuite) -> Result<String> {
    let value = to_json_value(suite)?;
    Ok(serde_json::to_string_pretty(&value)?)
}

/// Serialize a test suite to a [`serde_json::Value`] without the
/// intermediate string round-trip.
pub fn to_json_value(suite: &TestSuite) -> Result<serde_json::Value> {
    let report = JsonReport {
        suite_name: &suite.name,
        phase: suite.phase,
        status: suite.status.as_str(),
        summary: Summary {
            total: suite.total(),
            passed: suite.passed(),
            failed: suite.failed(),
            skipped: suite.skipped(),
            cancelled: suite.cancelled(),
        },
        results: &suite.results,
        corpus: crate::report::corpus::CorpusReport::from_cases(
            &suite.corpus_cases,
            suite.corpus_expected(),
        ),
    };
    Ok(serde_json::to_value(report)?)
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
    use conary_core::corpus::{
        ConversionStage, CorpusCaseResult, SourceArtifactDigestSource, SourceArtifactIdentity,
        SourceArtifactRole, SourceProfileIdentity, StageResult, TargetCapabilitySnapshot,
    };

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
            attempts: Vec::new(),
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
        assert_eq!(parsed["summary"]["cancelled"], 0);
        assert_eq!(parsed["results"][0]["id"], "T01");
        assert_eq!(parsed["results"][0]["status"], "passed");
        assert!(parsed.get("corpus").is_none());
    }

    #[test]
    fn corpus_report_is_attributable_and_fail_closed() {
        let mut suite = TestSuite::new("corpus", 4);
        suite.expect_corpus_cases(1);
        suite.record_corpus(CorpusCaseResult::from_stages(
            "TC-RPM-FEDORA",
            SourceProfileIdentity {
                profile: "fedora-44".into(),
                format: "rpm".into(),
            },
            vec![SourceArtifactIdentity {
                role: SourceArtifactRole::InstallRequest,
                digest_source: SourceArtifactDigestSource::FixtureBuildManifest,
                name: "fixture".into(),
                version: "1.0-1".into(),
                architecture: Some("x86_64".into()),
                digest: "a".repeat(64),
            }],
            TargetCapabilitySnapshot {
                profile: "fedora44".into(),
                architecture: "x86_64".into(),
                init_system: "systemd".into(),
                capabilities: vec!["native_lifecycle".into()],
            },
            vec![StageResult::passed(ConversionStage::Installation)],
        ));

        let parsed: serde_json::Value =
            serde_json::from_str(&to_json_report(&suite).unwrap()).unwrap();
        assert_eq!(parsed["corpus"]["schema_version"], 1);
        assert_eq!(parsed["corpus"]["expected_cases"], 1);
        assert_eq!(parsed["corpus"]["aggregate"]["cases"], 1);
        assert_eq!(parsed["corpus"]["aggregate"]["completed"], 1);
        assert_eq!(parsed["corpus"]["cases"][0]["case_id"], "TC-RPM-FEDORA");
        assert_eq!(
            parsed["corpus"]["cases"][0]["source_artifacts"][0]["role"],
            "install_request"
        );
        assert!(suite.corpus_all_completed());
    }

    #[test]
    fn missing_declared_corpus_case_is_published_and_blocks_success() {
        let mut suite = TestSuite::new("corpus", 4);
        suite.expect_corpus_cases(1);

        let parsed: serde_json::Value =
            serde_json::from_str(&to_json_report(&suite).unwrap()).unwrap();
        assert_eq!(parsed["corpus"]["expected_cases"], 1);
        assert_eq!(parsed["corpus"]["aggregate"]["cases"], 0);
        assert!(!suite.corpus_all_completed());
    }
}
