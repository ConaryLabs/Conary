// apps/conary/src/commands/packaging_mcp/projection.rs
//! Projection from M3a packaging output into agent operation envelopes.

#![allow(dead_code)] // Consumed by the publish plan/apply slice after read tools land.

use conary_agent_contract::{
    AgentError, AgentErrorKind, EvidenceItem, EvidenceKind, EvidenceRedaction, OperationEnvelope,
    OperationStatus, ResourceRef, RiskLevel,
};
use conary_core::diagnostics::{
    DiagnosticEvidence, DiagnosticEvidenceKind, PackagingCommandOutput, PackagingCommandStatus,
    PackagingDiagnosticCode, PackagingSeverity,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentProjectionMode {
    Inspect,
    Plan,
    Apply,
    Explain,
}

pub(crate) fn project_packaging_output(
    operation: &str,
    output: &PackagingCommandOutput,
    risk: RiskLevel,
    mode: AgentProjectionMode,
    subject: Option<ResourceRef>,
) -> OperationEnvelope {
    let output = super::super::diagnostics::redacted_packaging_output(output);
    let status = match (output.status, mode) {
        (PackagingCommandStatus::Succeeded, AgentProjectionMode::Plan) => OperationStatus::Planned,
        (PackagingCommandStatus::Succeeded, _) => OperationStatus::Ok,
        (PackagingCommandStatus::Failed, _) => OperationStatus::Failed,
    };
    let summary = output
        .summary
        .clone()
        .unwrap_or_else(|| format!("Packaging operation {}", status_summary(status)));
    let mut envelope = OperationEnvelope::new(operation, status, risk, summary);
    envelope.subject = subject;
    envelope.evidence.push(EvidenceItem {
        kind: EvidenceKind::Check,
        summary: "M3a packaging operation output".to_string(),
        uri: None,
        path: None,
        id: Some(output.operation_id.clone()),
        command: None,
        exit_code: None,
        metadata: std::collections::BTreeMap::from([
            (
                "schema_version".to_string(),
                serde_json::json!(output.schema_version),
            ),
            ("status".to_string(), serde_json::json!(output.status)),
        ]),
        redactions: Vec::new(),
    });

    for diagnostic in &output.diagnostics {
        if diagnostic.severity == PackagingSeverity::Warning {
            envelope.warnings.push(diagnostic.message.clone());
        }
        for evidence in &diagnostic.evidence {
            envelope.evidence.push(project_evidence(evidence));
        }
    }

    if let Some(error_diagnostic) = output
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.severity == PackagingSeverity::Error)
    {
        envelope.error = Some(AgentError {
            kind: diagnostic_code_to_error_kind(error_diagnostic.code),
            message: error_diagnostic.message.clone(),
            remediation: error_diagnostic
                .suggestions
                .first()
                .map(|suggestion| suggestion.message.clone()),
        });
    }

    envelope
}

fn status_summary(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Ok => "succeeded",
        OperationStatus::Planned => "planned",
        OperationStatus::Running => "running",
        OperationStatus::Unavailable => "unavailable",
        OperationStatus::Failed => "failed",
        OperationStatus::Partial => "partially completed",
    }
}

fn diagnostic_code_to_error_kind(code: PackagingDiagnosticCode) -> AgentErrorKind {
    match code {
        PackagingDiagnosticCode::InferenceTrace
        | PackagingDiagnosticCode::RecipeValidationWarning => AgentErrorKind::PartialFailure,
        PackagingDiagnosticCode::RecipeValidationFailed
        | PackagingDiagnosticCode::BuildNetworkAccess
        | PackagingDiagnosticCode::UnpinnedDependency
        | PackagingDiagnosticCode::CommandRiskEvidence
        | PackagingDiagnosticCode::PublishGateFailed
        | PackagingDiagnosticCode::ProjectPublishPreflightFailed => {
            AgentErrorKind::ValidationFailed
        }
        PackagingDiagnosticCode::SourceCacheMiss
        | PackagingDiagnosticCode::WatchSourceIdentityFailed => AgentErrorKind::MissingPrerequisite,
        PackagingDiagnosticCode::PublishJsonUnsupported
        | PackagingDiagnosticCode::TryWatchUnsupported => AgentErrorKind::NotSupported,
        PackagingDiagnosticCode::CookFailed
        | PackagingDiagnosticCode::WatchCookFailed
        | PackagingDiagnosticCode::WatchTryRefreshFailed
        | PackagingDiagnosticCode::WatchCleanupFailed
        | PackagingDiagnosticCode::RecordBackendUnavailable
        | PackagingDiagnosticCode::RecordTraceFailed
        | PackagingDiagnosticCode::RecordCommandFailed
        | PackagingDiagnosticCode::RecordDraftGenerated
        | PackagingDiagnosticCode::RecordValidationFailed
        | PackagingDiagnosticCode::RecordRedactionFailed
        | PackagingDiagnosticCode::RecordCleanupFailed
        | PackagingDiagnosticCode::OperationRecordWriteFailed
        | PackagingDiagnosticCode::RedactionFailed
        | PackagingDiagnosticCode::Unknown => AgentErrorKind::PartialFailure,
    }
}

fn project_evidence(evidence: &DiagnosticEvidence) -> EvidenceItem {
    EvidenceItem {
        kind: match evidence.kind {
            DiagnosticEvidenceKind::Command => EvidenceKind::Command,
            DiagnosticEvidenceKind::Path | DiagnosticEvidenceKind::Uri => EvidenceKind::Resource,
            DiagnosticEvidenceKind::Log => EvidenceKind::Log,
            DiagnosticEvidenceKind::Check => EvidenceKind::Check,
            DiagnosticEvidenceKind::Artifact => EvidenceKind::Artifact,
        },
        summary: evidence.summary.clone(),
        uri: evidence.uri.clone(),
        path: evidence.path.clone(),
        id: None,
        command: evidence.command.clone(),
        exit_code: None,
        metadata: evidence.metadata.clone(),
        redactions: evidence
            .redactions
            .iter()
            .map(|redaction| EvidenceRedaction {
                field: redaction.field.clone(),
                reason: redaction.reason.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_agent_contract::{AgentErrorKind, EvidenceKind, OperationStatus, RiskLevel};
    use conary_core::diagnostics::{
        DiagnosticEvidence, PackagingCommandOutput, PackagingDiagnostic, PackagingDiagnosticCode,
        PackagingPhase,
    };

    #[test]
    fn failed_publish_gate_projects_validation_error_and_evidence() {
        let diagnostic = PackagingDiagnostic::error(
            PackagingPhase::Publish,
            PackagingDiagnosticCode::PublishGateFailed,
            "Static artifact publish gate failed",
        )
        .with_evidence(DiagnosticEvidence::log(
            "publish-gate",
            "RecordedDraftArtifact",
        ));
        let output =
            PackagingCommandOutput::failed("publish-1", "conary publish", vec![diagnostic]);

        let envelope = project_packaging_output(
            "conary.packaging.publish.apply",
            &output,
            RiskLevel::High,
            AgentProjectionMode::Apply,
            None,
        );

        assert_eq!(envelope.status, OperationStatus::Failed);
        assert_eq!(
            envelope.error.unwrap().kind,
            AgentErrorKind::ValidationFailed
        );
        assert!(
            envelope
                .evidence
                .iter()
                .any(|item| item.kind == EvidenceKind::Log)
        );
    }

    #[test]
    fn succeeded_plan_projects_planned_status() {
        let output = PackagingCommandOutput::succeeded("plan-1", "conary publish");
        let envelope = project_packaging_output(
            "conary.packaging.publish.plan",
            &output,
            RiskLevel::High,
            AgentProjectionMode::Plan,
            None,
        );

        assert_eq!(envelope.status, OperationStatus::Planned);
        assert!(envelope.error.is_none());
    }

    #[test]
    fn projection_redacts_path_uri_command_and_metadata_before_agent_output() {
        let diagnostic = PackagingDiagnostic::error(
            PackagingPhase::Publish,
            PackagingDiagnosticCode::PublishGateFailed,
            "failed for /home/alice/.conary/keys/root.pem",
        )
        .with_evidence(
            DiagnosticEvidence::artifact("private-key", "/home/alice/.conary/keys/root.pem")
                .with_metadata(
                    "url",
                    serde_json::json!("https://user:secret@example.invalid/pkg"),
                ),
        );
        let output = PackagingCommandOutput::failed(
            "publish-redact",
            "conary publish --bearer-token secret",
            vec![diagnostic],
        );

        let envelope = project_packaging_output(
            "conary.packaging.publish.apply",
            &output,
            RiskLevel::High,
            AgentProjectionMode::Apply,
            None,
        );
        let rendered = serde_json::to_string(&envelope).unwrap();

        assert!(!rendered.contains("/home/alice"));
        assert!(!rendered.contains("secret"));
        assert!(rendered.contains("redactions"));
    }
}
