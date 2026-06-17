// apps/conary/src/commands/record_mode/trace.rs

use std::path::PathBuf;

use anyhow::Result;
use conary_core::recipe::recording::{
    ObservedPath, RecordingLimitation, ScopeRoot, SelectedBackend,
};

use super::types::RequestedRecordBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TraceLimitation {
    IncompleteReadEvidence,
    EventLoss,
}

impl TraceLimitation {
    pub(crate) fn to_report_limitation(&self) -> RecordingLimitation {
        match self {
            Self::IncompleteReadEvidence => RecordingLimitation::IncompleteReadEvidence,
            Self::EventLoss => RecordingLimitation::EventLoss,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TraceScope {
    pub(crate) source: ScopeRoot,
    pub(crate) work: ScopeRoot,
    pub(crate) install: ScopeRoot,
}

impl TraceScope {
    pub(crate) fn roots(&self) -> [&ScopeRoot; 3] {
        [&self.source, &self.work, &self.install]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TraceBackendStatus {
    pub(crate) backend: SelectedBackend,
    pub(crate) limitations: Vec<TraceLimitation>,
    pub(crate) unavailable_reason: Option<String>,
}

impl TraceBackendStatus {
    pub(crate) fn selected(backend: SelectedBackend, limitations: Vec<TraceLimitation>) -> Self {
        Self {
            backend,
            limitations,
            unavailable_reason: None,
        }
    }

    pub(crate) fn unavailable(backend: SelectedBackend, reason: impl Into<String>) -> Self {
        Self {
            backend,
            limitations: Vec::new(),
            unavailable_reason: Some(reason.into()),
        }
    }

    pub(crate) fn is_usable(&self) -> bool {
        self.unavailable_reason.is_none()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RawTraceEvent {
    pub(crate) path: PathBuf,
    pub(crate) observed: ObservedPath,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TraceDrain {
    pub(crate) events: Vec<RawTraceEvent>,
    pub(crate) ignored_events: u64,
    pub(crate) event_loss: bool,
}

pub(crate) trait TraceBackend {
    fn probe(
        &self,
        scope: &TraceScope,
        requested: RequestedRecordBackend,
    ) -> Result<TraceBackendStatus>;

    fn start(&self, scope: TraceScope) -> Result<Box<dyn TraceSession>>;
}

pub(crate) trait TraceSession {
    fn drain_events(&mut self) -> Result<TraceDrain>;
    fn finish(&mut self) -> Result<TraceDrain>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_selection_falls_back_to_inotify_with_limitation() {
        let status = TraceBackendStatus::selected(
            conary_core::recipe::recording::SelectedBackend::Inotify,
            vec![TraceLimitation::IncompleteReadEvidence],
        );
        assert!(status.is_usable());
        assert_eq!(
            status.limitations,
            vec![TraceLimitation::IncompleteReadEvidence]
        );
        assert_eq!(
            status.limitations[0].to_report_limitation(),
            RecordingLimitation::IncompleteReadEvidence
        );
    }

    #[test]
    fn backend_status_can_report_unavailable_and_event_loss() {
        let status = TraceBackendStatus::unavailable(
            conary_core::recipe::recording::SelectedBackend::Fanotify,
            "missing capability",
        );

        assert!(!status.is_usable());
        assert_eq!(
            status.unavailable_reason.as_deref(),
            Some("missing capability")
        );
        assert_eq!(
            TraceLimitation::EventLoss.to_report_limitation(),
            RecordingLimitation::EventLoss
        );
    }
}
