// conary-test/src/report/stream.rs

use serde::Serialize;

/// Server-Sent Event types for live streaming.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum TestEvent {
    #[serde(rename = "suite_started")]
    SuiteStarted {
        run_id: u64,
        suite: String,
        phase: u32,
        total: usize,
    },

    #[serde(rename = "test_started")]
    TestStarted {
        run_id: u64,
        test_id: String,
        name: String,
    },

    #[serde(rename = "test_passed")]
    TestPassed {
        run_id: u64,
        test_id: String,
        duration_ms: u64,
    },

    #[serde(rename = "test_failed")]
    TestFailed {
        run_id: u64,
        test_id: String,
        message: String,
        stdout: Option<String>,
    },

    #[serde(rename = "test_skipped")]
    TestSkipped {
        run_id: u64,
        test_id: String,
        message: String,
    },

    #[serde(rename = "step_output")]
    StepOutput {
        run_id: u64,
        test_id: String,
        step: usize,
        line: String,
    },

    #[serde(rename = "run_complete")]
    RunComplete {
        run_id: u64,
        passed: usize,
        failed: usize,
        skipped: usize,
    },
}

impl TestEvent {
    /// The run ID associated with this event.
    pub fn run_id(&self) -> u64 {
        match self {
            Self::SuiteStarted { run_id, .. }
            | Self::TestStarted { run_id, .. }
            | Self::TestPassed { run_id, .. }
            | Self::TestFailed { run_id, .. }
            | Self::TestSkipped { run_id, .. }
            | Self::StepOutput { run_id, .. }
            | Self::RunComplete { run_id, .. } => *run_id,
        }
    }

    /// SSE event type name for this event variant.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::SuiteStarted { .. } => "suite_started",
            Self::TestStarted { .. } => "test_started",
            Self::TestPassed { .. } => "test_passed",
            Self::TestFailed { .. } => "test_failed",
            Self::TestSkipped { .. } => "test_skipped",
            Self::StepOutput { .. } => "step_output",
            Self::RunComplete { .. } => "run_complete",
        }
    }

    /// Format as SSE text.
    pub fn to_sse(&self) -> String {
        let data = serde_json::to_string(self)
            .unwrap_or_else(|e| format!("{{\"error\":\"serialization failed: {e}\"}}"));
        format!("event: {}\ndata: {data}\n\n", self.event_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_serialization() {
        let event = TestEvent::TestPassed {
            run_id: 1,
            test_id: "T01".to_string(),
            duration_ms: 100,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("test_id"));
        assert!(json.contains("T01"));
        assert!(json.contains("test_passed"));
    }

    #[test]
    fn suite_started_event_serializes() {
        let event = TestEvent::SuiteStarted {
            run_id: 5,
            suite: "phase1-core".to_string(),
            phase: 1,
            total: 10,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("suite_started"));
        assert!(json.contains("phase1-core"));
        assert!(json.contains(r#""total":10"#));
    }

    #[test]
    fn step_output_event_serializes() {
        let event = TestEvent::StepOutput {
            run_id: 3,
            test_id: "T05".to_string(),
            step: 2,
            line: "conary: installed vim-9.1".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("step_output"));
        assert!(json.contains("T05"));
        assert!(json.contains("conary: installed vim-9.1"));
    }

    #[test]
    fn to_sse_format_suite_started() {
        let event = TestEvent::SuiteStarted {
            run_id: 1,
            suite: "smoke".to_string(),
            phase: 1,
            total: 5,
        };
        let sse = event.to_sse();
        assert!(sse.starts_with("event: suite_started\n"));
        assert!(sse.contains("data: "));
        assert!(sse.ends_with("\n\n"));
    }

    #[test]
    fn to_sse_format_step_output() {
        let event = TestEvent::StepOutput {
            run_id: 1,
            test_id: "T01".to_string(),
            step: 0,
            line: "hello".to_string(),
        };
        let sse = event.to_sse();
        assert!(sse.starts_with("event: step_output\n"));
    }

    #[test]
    fn run_id_accessor() {
        let event = TestEvent::RunComplete {
            run_id: 42,
            passed: 10,
            failed: 1,
            skipped: 2,
        };
        assert_eq!(event.run_id(), 42);
    }

    #[test]
    fn run_id_accessor_all_variants() {
        let events: Vec<TestEvent> = vec![
            TestEvent::SuiteStarted {
                run_id: 1,
                suite: "s".to_string(),
                phase: 1,
                total: 0,
            },
            TestEvent::TestStarted {
                run_id: 2,
                test_id: "T01".to_string(),
                name: "n".to_string(),
            },
            TestEvent::TestPassed {
                run_id: 3,
                test_id: "T01".to_string(),
                duration_ms: 0,
            },
            TestEvent::TestFailed {
                run_id: 4,
                test_id: "T01".to_string(),
                message: "m".to_string(),
                stdout: None,
            },
            TestEvent::TestSkipped {
                run_id: 5,
                test_id: "T01".to_string(),
                message: "m".to_string(),
            },
            TestEvent::StepOutput {
                run_id: 6,
                test_id: "T01".to_string(),
                step: 0,
                line: "l".to_string(),
            },
            TestEvent::RunComplete {
                run_id: 7,
                passed: 0,
                failed: 0,
                skipped: 0,
            },
        ];
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.run_id(), (i + 1) as u64);
        }
    }
}
