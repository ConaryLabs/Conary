pub mod capabilities;
pub mod draft;
pub mod report;

pub use capabilities::suggest_capabilities_from_evidence;
pub use draft::{
    DraftRecipeInput, derive_draft_recipe, installed_file_paths_from_evidence,
    render_recorded_command,
};
pub use report::{
    CapabilitySuggestion, IgnoredEvent, InstalledFileEvidence, ObservedPath, RecordingLimitation,
    RecordingReport, ScopeRoot, ScopeRootLabel, SelectedBackend, TraceOperation, TraceScope,
};
