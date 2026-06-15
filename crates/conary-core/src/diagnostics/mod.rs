// crates/conary-core/src/diagnostics/mod.rs

pub mod redaction;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const PACKAGING_JSON_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackagingCommandStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackagingPhase {
    Inference,
    RecipeValidation,
    SourceFetch,
    Build,
    TrySession,
    Publish,
    OperationRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackagingSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackagingDiagnosticCode {
    InferenceTrace,
    RecipeValidationWarning,
    RecipeValidationFailed,
    SourceCacheMiss,
    BuildNetworkAccess,
    UnpinnedDependency,
    CommandRiskEvidence,
    CookFailed,
    PublishGateFailed,
    ProjectPublishPreflightFailed,
    PublishJsonUnsupported,
    OperationRecordWriteFailed,
    RedactionFailed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticEvidenceKind {
    Command,
    Path,
    Uri,
    Log,
    Check,
    Artifact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionMarker {
    pub field: String,
    pub reason: String,
}

impl RedactionMarker {
    pub fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticEvidence {
    pub kind: DiagnosticEvidenceKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redactions: Vec<RedactionMarker>,
}

impl DiagnosticEvidence {
    pub fn log(summary: impl Into<String>, log: impl Into<String>) -> Self {
        Self {
            kind: DiagnosticEvidenceKind::Log,
            summary: summary.into(),
            path: None,
            uri: None,
            command: None,
            log: Some(log.into()),
            metadata: BTreeMap::new(),
            redactions: Vec::new(),
        }
    }

    pub fn artifact(summary: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            kind: DiagnosticEvidenceKind::Artifact,
            summary: summary.into(),
            path: Some(path.into()),
            uri: None,
            command: None,
            log: None,
            metadata: BTreeMap::new(),
            redactions: Vec::new(),
        }
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    pub fn with_redaction(mut self, redaction: RedactionMarker) -> Self {
        self.redactions.push(redaction);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagingSuggestion {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

impl PackagingSuggestion {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            command: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackagingDiagnostic {
    pub phase: PackagingPhase,
    pub code: PackagingDiagnosticCode,
    pub severity: PackagingSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<DiagnosticEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<PackagingSuggestion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redactions: Vec<RedactionMarker>,
}

impl PackagingDiagnostic {
    pub fn new(
        phase: PackagingPhase,
        code: PackagingDiagnosticCode,
        severity: PackagingSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            phase,
            code,
            severity,
            message: message.into(),
            evidence: Vec::new(),
            suggestions: Vec::new(),
            redactions: Vec::new(),
        }
    }

    pub fn info(
        phase: PackagingPhase,
        code: PackagingDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self::new(phase, code, PackagingSeverity::Info, message)
    }

    pub fn warning(
        phase: PackagingPhase,
        code: PackagingDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self::new(phase, code, PackagingSeverity::Warning, message)
    }

    pub fn error(
        phase: PackagingPhase,
        code: PackagingDiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self::new(phase, code, PackagingSeverity::Error, message)
    }

    pub fn with_evidence(mut self, evidence: DiagnosticEvidence) -> Self {
        self.evidence.push(evidence);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagingArtifact {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagingProgress {
    pub current: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackagingEventKind {
    OperationStarted,
    PhaseStarted,
    PhaseFinished,
    CommandStarted,
    CommandFailed,
    DiagnosticEmitted,
    ArtifactCreated,
    OperationFinished,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackagingEvent {
    pub schema_version: u16,
    pub operation_id: String,
    pub sequence: u64,
    pub phase: PackagingPhase,
    pub kind: PackagingEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<PackagingDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<PackagingArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<PackagingProgress>,
}

impl PackagingEvent {
    pub fn diagnostic(
        operation_id: impl Into<String>,
        sequence: u64,
        diagnostic: PackagingDiagnostic,
    ) -> Self {
        Self {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: operation_id.into(),
            sequence,
            phase: diagnostic.phase,
            kind: PackagingEventKind::DiagnosticEmitted,
            message: Some(diagnostic.message.clone()),
            diagnostic: Some(diagnostic),
            artifact: None,
            progress: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackagingCommandOutput {
    pub schema_version: u16,
    pub operation_id: String,
    pub command: String,
    pub status: PackagingCommandStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<PackagingDiagnostic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<PackagingEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<PackagingArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl PackagingCommandOutput {
    pub fn succeeded(operation_id: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: operation_id.into(),
            command: command.into(),
            status: PackagingCommandStatus::Succeeded,
            diagnostics: Vec::new(),
            events: Vec::new(),
            artifacts: Vec::new(),
            summary: None,
        }
    }

    pub fn failed(
        operation_id: impl Into<String>,
        command: impl Into<String>,
        diagnostics: Vec<PackagingDiagnostic>,
    ) -> Self {
        Self {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: operation_id.into(),
            command: command.into(),
            status: PackagingCommandStatus::Failed,
            diagnostics,
            events: Vec::new(),
            artifacts: Vec::new(),
            summary: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_output_serializes_schema_version_and_diagnostic_code() {
        let diagnostic = PackagingDiagnostic::error(
            PackagingPhase::RecipeValidation,
            PackagingDiagnosticCode::RecipeValidationFailed,
            "Recipe validation failed",
        )
        .with_evidence(DiagnosticEvidence::log("validation", "missing [package] section"));

        let output = PackagingCommandOutput::failed("op-1", "conary cook", vec![diagnostic]);

        let value = serde_json::to_value(&output).expect("serialize command output");
        assert_eq!(value["schema_version"], PACKAGING_JSON_SCHEMA_VERSION);
        assert_eq!(value["status"], "failed");
        assert_eq!(value["diagnostics"][0]["code"], "recipe-validation-failed");
        assert_eq!(value["diagnostics"][0]["phase"], "recipe-validation");
    }

    #[test]
    fn event_serializes_sequence_and_optional_diagnostic() {
        let event = PackagingEvent::diagnostic(
            "cook-1",
            7,
            PackagingDiagnostic::warning(
                PackagingPhase::Inference,
                PackagingDiagnosticCode::InferenceTrace,
                "Inference used Cargo metadata",
            ),
        );

        let value = serde_json::to_value(&event).expect("serialize event");
        assert_eq!(value["schema_version"], PACKAGING_JSON_SCHEMA_VERSION);
        assert_eq!(value["operation_id"], "cook-1");
        assert_eq!(value["sequence"], 7);
        assert_eq!(value["kind"], "diagnostic-emitted");
        assert_eq!(value["diagnostic"]["code"], "inference-trace");
    }

    #[test]
    fn diagnostic_evidence_can_store_redaction_markers() {
        let evidence = DiagnosticEvidence::log("command", "Bearer [REDACTED]")
            .with_metadata("source", serde_json::json!("scriptlet"))
            .with_redaction(RedactionMarker::new("log", "bearer-token"));
        assert_eq!(evidence.redactions[0].field, "log");
        assert_eq!(evidence.metadata["source"], "scriptlet");
    }
}
