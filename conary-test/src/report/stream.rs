// conary-test/src/report/stream.rs

use serde::Serialize;

/// Server-Sent Event types for live streaming.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum TestEvent {
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

    #[serde(rename = "run_complete")]
    RunComplete {
        run_id: u64,
        passed: usize,
        failed: usize,
        skipped: usize,
    },
}

impl TestEvent {
    /// Format as SSE text.
    pub fn to_sse(&self) -> String {
        let event_name = match self {
            Self::TestStarted { .. } => "test_started",
            Self::TestPassed { .. } => "test_passed",
            Self::TestFailed { .. } => "test_failed",
            Self::TestSkipped { .. } => "test_skipped",
            Self::RunComplete { .. } => "run_complete",
        };
        let data = serde_json::to_string(self).unwrap_or_else(|e| {
            format!("{{\"error\":\"serialization failed: {e}\"}}")
        });
        format!("event: {event_name}\ndata: {data}\n\n")
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
}
