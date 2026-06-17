// conary-core/src/recipe/recording/mod.rs

pub mod report;

pub use report::{
    CapabilitySuggestion, IgnoredEvent, InstalledFileEvidence, ObservedPath, RecordingLimitation,
    RecordingReport, ScopeRoot, ScopeRootLabel, SelectedBackend, TraceOperation, TraceScope,
};
