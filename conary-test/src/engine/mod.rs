// conary-test/src/engine/mod.rs

pub mod assertions;
pub mod runner;
pub mod suite;

pub use assertions::evaluate_assertion;
pub use runner::TestRunner;
pub use suite::{RunStatus, TestResult, TestStatus, TestSuite};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::manifest::Assertion;

    fn make_result(id: &str, status: TestStatus) -> TestResult {
        TestResult {
            id: id.to_string(),
            name: format!("test_{id}"),
            status,
            duration_ms: 10,
            message: None,
            stdout: None,
            stderr: None,
        }
    }

    fn empty_assertion() -> Assertion {
        Assertion {
            exit_code: None,
            exit_code_not: None,
            stdout_contains: None,
            stdout_not_contains: None,
            stderr_contains: None,
            file_exists: None,
            file_not_exists: None,
            file_checksum: None,
        }
    }

    #[test]
    fn test_suite_tracks_results() {
        let mut suite = TestSuite::new("tracker", 1);
        suite.record(make_result("T01", TestStatus::Passed));
        suite.record(make_result("T02", TestStatus::Failed));

        assert_eq!(suite.passed(), 1);
        assert_eq!(suite.failed(), 1);
        assert_eq!(suite.skipped(), 0);
        assert_eq!(suite.total(), 2);
        assert!(!suite.has_failed("T01"));
        assert!(suite.has_failed("T02"));
    }

    #[test]
    fn test_suite_dependency_check() {
        let mut suite = TestSuite::new("deps", 1);
        suite.record(make_result("T01", TestStatus::Failed));

        // Failed dep causes skip.
        let deps = Some(vec!["T01".to_string()]);
        assert!(suite.should_skip(&deps));

        // No deps never skips.
        assert!(!suite.should_skip(&None));

        // Unknown dep does not skip.
        let unknown = Some(vec!["T99".to_string()]);
        assert!(!suite.should_skip(&unknown));
    }

    #[test]
    fn test_assertion_exit_code() {
        let mut a = empty_assertion();
        a.exit_code = Some(0);

        assert!(evaluate_assertion(&a, 0, "", "").is_ok());
        assert!(evaluate_assertion(&a, 1, "", "").is_err());
    }

    #[test]
    fn test_assertion_stdout_contains() {
        let mut a = empty_assertion();
        a.stdout_contains = Some("hello".to_string());

        assert!(evaluate_assertion(&a, 0, "say hello world", "").is_ok());
        assert!(evaluate_assertion(&a, 0, "goodbye", "").is_err());
    }

    #[test]
    fn test_assertion_combined() {
        let mut a = empty_assertion();
        a.exit_code = Some(0);
        a.stdout_contains = Some("ok".to_string());
        a.stdout_not_contains = Some("error".to_string());

        // All conditions met.
        assert!(evaluate_assertion(&a, 0, "status: ok", "").is_ok());

        // Wrong exit code.
        assert!(evaluate_assertion(&a, 1, "status: ok", "").is_err());

        // Missing stdout needle.
        assert!(evaluate_assertion(&a, 0, "status: fine", "").is_err());

        // Contains forbidden string.
        assert!(evaluate_assertion(&a, 0, "ok but error", "").is_err());
    }
}
