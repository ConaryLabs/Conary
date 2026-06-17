// apps/conary/src/commands/diagnostics.rs

#![cfg_attr(not(test), allow(dead_code))]

use std::io::Write;

use anyhow::Result;
use conary_core::diagnostics::redaction::{
    redact_command, redact_json_value, redact_log, redact_text,
};
use conary_core::diagnostics::{
    DiagnosticEvidence, PACKAGING_JSON_SCHEMA_VERSION, PackagingArtifact, PackagingCommandOutput,
    PackagingDiagnostic, PackagingEvent, PackagingEventKind, PackagingPhase, PackagingSeverity,
};

pub(crate) fn render_packaging_json(output: &PackagingCommandOutput) -> Result<String> {
    Ok(serde_json::to_string_pretty(&redacted_packaging_output(
        output,
    ))?)
}

pub(crate) fn render_diagnostics_human(
    diagnostics: &[PackagingDiagnostic],
    output: &mut impl Write,
) -> Result<()> {
    for diagnostic in diagnostics {
        let label = match diagnostic.severity {
            PackagingSeverity::Info => "Info",
            PackagingSeverity::Warning => "Warning",
            PackagingSeverity::Error => "Error",
        };
        writeln!(output, "{label}: {}", diagnostic.message)?;
        for evidence in &diagnostic.evidence {
            writeln!(
                output,
                "  {}: {}",
                evidence.summary,
                evidence.log.as_deref().unwrap_or("")
            )?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn write_packaging_output(
    output: &PackagingCommandOutput,
    json: bool,
    writer: &mut impl Write,
) -> Result<()> {
    let output = redacted_packaging_output(output);
    if json {
        writeln!(writer, "{}", serde_json::to_string_pretty(&output)?)?;
    } else {
        render_diagnostics_human(&output.diagnostics, writer)?;
    }
    Ok(())
}

pub(crate) fn write_packaging_record_if_possible(output: &PackagingCommandOutput) {
    if cfg!(test) && std::env::var_os("CONARY_PACKAGING_OPERATIONS_DIR").is_none() {
        return;
    }

    let dir = match super::operation_records::default_packaging_operations_dir() {
        Ok(dir) => dir,
        Err(error) => {
            tracing::warn!(%error, "failed to resolve packaging operation record directory");
            return;
        }
    };
    let output = redacted_packaging_output(output);
    if let Err(error) = super::operation_records::write_packaging_record_unchecked(
        &dir,
        &output.operation_id,
        &output,
    ) {
        tracing::warn!(%error, "failed to write packaging operation record");
    }
}

pub(crate) fn publish_gate_code_to_diagnostic_code(
    code: conary_core::repository::static_repo::publish_gate::PublishGateFailureCode,
) -> conary_core::diagnostics::PackagingDiagnosticCode {
    match code {
        conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::MissingAttestation
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::BuildAttestationSignatureMismatch
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::PackageSignatureMismatch
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::TomlIntegrityMismatch
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::OutputIdentityMismatch
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::UnacceptedSignerKey
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::RetiredSignerKey
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::AbsentOrUnknownProvenanceClass
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::NonHermeticHardeningLevel
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::StaleOrUnknownPolicy
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::UncleanCommandRiskReport
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::ForeignConversionMissingBoundary
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::ForeignConversionBoundaryHashMismatch
        | conary_core::repository::static_repo::publish_gate::PublishGateFailureCode::RecordedDraftArtifact => {
            conary_core::diagnostics::PackagingDiagnosticCode::PublishGateFailed
        }
    }
}

pub(crate) fn ccs_v2_diagnostic_to_packaging(
    diagnostic: &conary_core::ccs::v2::V2Diagnostic,
) -> conary_core::diagnostics::PackagingDiagnostic {
    use conary_core::ccs::v2::V2DiagnosticCode;
    use conary_core::diagnostics::{
        PackagingDiagnostic, PackagingDiagnosticCode, PackagingPhase, PackagingSuggestion,
    };

    let code = match diagnostic.code {
        V2DiagnosticCode::LegacyV1Package => PackagingDiagnosticCode::CcsV2LegacyRejected,
        _ => PackagingDiagnosticCode::CcsV2ValidationFailed,
    };
    let mut rendered = PackagingDiagnostic::error(
        PackagingPhase::RecipeValidation,
        code,
        diagnostic.message.clone(),
    );
    rendered
        .suggestions
        .push(PackagingSuggestion::new(diagnostic.suggestion.clone()));
    rendered
}

pub(crate) fn redacted_packaging_output(output: &PackagingCommandOutput) -> PackagingCommandOutput {
    let mut output = output.clone();
    if let Some(summary) = &mut output.summary {
        let redacted = redact_text(summary);
        *summary = redacted.value;
    }
    for diagnostic in &mut output.diagnostics {
        redact_diagnostic(diagnostic);
    }
    for artifact in &mut output.artifacts {
        redact_artifact(artifact);
    }
    for event in &mut output.events {
        *event = redacted_packaging_event(event);
    }
    output
}

pub(crate) fn redacted_packaging_event(event: &PackagingEvent) -> PackagingEvent {
    let mut event = event.clone();
    if let Some(message) = &mut event.message {
        let redacted = redact_text(message);
        *message = redacted.value;
    }
    if let Some(diagnostic) = &mut event.diagnostic {
        redact_diagnostic(diagnostic);
    }
    if let Some(artifact) = &mut event.artifact {
        redact_artifact(artifact);
    }
    event
}

pub(crate) fn render_packaging_event_ndjson(event: &PackagingEvent) -> Result<String> {
    let mut rendered = serde_json::to_string(&redacted_packaging_event(event))?;
    rendered.push('\n');
    Ok(rendered)
}

pub(crate) fn bounded_watch_events(
    operation_id: &str,
    events: &[PackagingEvent],
    limit: usize,
) -> Vec<PackagingEvent> {
    if events.len() <= limit {
        return events.to_vec();
    }
    let omitted = events.len() - limit;
    let mut retained = Vec::with_capacity(limit);
    // This synthetic event is only persisted in the final operation record. It
    // is not emitted on the live NDJSON stream, so sequence reuse cannot confuse
    // stream consumers.
    retained.push(PackagingEvent {
        schema_version: PACKAGING_JSON_SCHEMA_VERSION,
        operation_id: operation_id.to_string(),
        sequence: events[omitted].sequence,
        phase: PackagingPhase::OperationRecord,
        kind: PackagingEventKind::WatchRefreshSkipped,
        message: Some(format!(
            "{omitted} older watch events were omitted from this operation record"
        )),
        diagnostic: None,
        artifact: None,
        progress: None,
    });
    retained.extend(events[omitted + 1..].iter().cloned());
    retained
}

fn redact_diagnostic(diagnostic: &mut PackagingDiagnostic) {
    let message = redact_text(&diagnostic.message);
    diagnostic.message = message.value;
    diagnostic.redactions.extend(message.redactions);
    for evidence in &mut diagnostic.evidence {
        redact_evidence(evidence);
    }
}

fn redact_evidence(evidence: &mut DiagnosticEvidence) {
    let summary = redact_text(&evidence.summary);
    evidence.summary = summary.value;
    evidence.redactions.extend(summary.redactions);
    if let Some(path) = &mut evidence.path {
        let redacted = redact_text(path);
        *path = redacted.value;
        evidence.redactions.extend(redacted.redactions);
    }
    if let Some(uri) = &mut evidence.uri {
        let redacted = redact_text(uri);
        *uri = redacted.value;
        evidence.redactions.extend(redacted.redactions);
    }
    if let Some(log) = &mut evidence.log {
        let redacted = redact_log(log);
        *log = redacted.value;
        evidence.redactions.extend(redacted.redactions);
    }
    if let Some(command) = &mut evidence.command {
        let redacted = redact_command(command);
        *command = redacted.value;
        evidence.redactions.extend(redacted.redactions);
    }
    for (key, value) in &mut evidence.metadata {
        evidence
            .redactions
            .extend(redact_json_value(value, &format!("metadata.{key}")));
    }
}

fn redact_artifact(artifact: &mut PackagingArtifact) {
    let redacted = redact_text(&artifact.path);
    artifact.path = redacted.value;
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::diagnostics::{
        DiagnosticEvidence, PACKAGING_JSON_SCHEMA_VERSION, PackagingArtifact,
        PackagingCommandOutput, PackagingDiagnostic, PackagingDiagnosticCode, PackagingEvent,
        PackagingEventKind, PackagingPhase,
    };

    #[test]
    fn json_renderer_includes_schema_version() {
        let output = PackagingCommandOutput::failed(
            "cook-1",
            "conary cook",
            vec![PackagingDiagnostic::error(
                PackagingPhase::RecipeValidation,
                PackagingDiagnosticCode::RecipeValidationFailed,
                "Recipe validation failed",
            )],
        );

        let json = render_packaging_json(&output).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["diagnostics"][0]["code"], "recipe-validation-failed");
    }

    #[test]
    fn human_renderer_uses_same_diagnostic_values() {
        let diagnostic = PackagingDiagnostic::warning(
            PackagingPhase::RecipeValidation,
            PackagingDiagnosticCode::RecipeValidationWarning,
            "unused field",
        )
        .with_evidence(DiagnosticEvidence::log("validator", "ignored key"));
        let mut output = Vec::new();
        render_diagnostics_human(&[diagnostic], &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Warning: unused field"));
        assert!(text.contains("validator"));
    }

    #[test]
    fn json_renderer_redacts_nested_evidence_events_and_artifacts_before_serializing() {
        let diagnostic = PackagingDiagnostic::error(
            PackagingPhase::Build,
            PackagingDiagnosticCode::CookFailed,
            "build command failed",
        )
        .with_evidence(
            DiagnosticEvidence::log(
                "command",
                format!(
                    "API_TOKEN=sk-demo-secret curl -H 'Authorization: Bearer abc.def'\n{}",
                    "x".repeat(20_000)
                ),
            )
            .with_metadata(
                "fetch",
                serde_json::json!({
                    "uri": "https://user:pass@example.invalid/source.tar.gz",
                    "env": ["PRIVATE_KEY=/home/dev/.ssh/id_ed25519"]
                }),
            ),
        );
        let mut output =
            PackagingCommandOutput::failed("cook-1", "conary cook", vec![diagnostic.clone()]);
        output.summary = Some("Bearer summary.secret".to_string());
        output.artifacts.push(PackagingArtifact {
            path: "/home/dev/.ssh/id_rsa".to_string(),
            kind: Some("debug-key".to_string()),
        });
        let mut event = PackagingEvent::diagnostic("cook-1", 1, diagnostic);
        event.message = Some("Bearer event.secret".to_string());
        event.artifact = Some(PackagingArtifact {
            path: "/home/dev/private.key".to_string(),
            kind: Some("artifact".to_string()),
        });
        output.events.push(event);

        let json = render_packaging_json(&output).unwrap();
        assert!(!json.contains("sk-demo-secret"));
        assert!(!json.contains("abc.def"));
        assert!(!json.contains("event.secret"));
        assert!(!json.contains("summary.secret"));
        assert!(!json.contains("user:pass"));
        assert!(!json.contains("/home/dev/.ssh/id_rsa"));
        assert!(!json.contains("/home/dev/private.key"));
        assert!(json.contains("API_TOKEN=[REDACTED]"));
        assert!(json.contains("Bearer [REDACTED]"));
        assert!(json.contains("[TRUNCATED]"));
        assert!(json.contains("\"redactions\""));
    }

    #[test]
    fn packaging_event_ndjson_redacts_diagnostic_before_serializing() {
        let diagnostic = PackagingDiagnostic::error(
            PackagingPhase::Build,
            PackagingDiagnosticCode::WatchCookFailed,
            "failed with API_TOKEN=secret",
        )
        .with_evidence(DiagnosticEvidence::log(
            "build log",
            "Authorization: Bearer abc.def",
        ));
        let event = PackagingEvent::diagnostic("watch-1", 3, diagnostic);

        let line = render_packaging_event_ndjson(&event).unwrap();

        assert!(line.ends_with('\n'));
        assert!(!line.contains("API_TOKEN=secret"), "{line}");
        assert!(!line.contains("abc.def"), "{line}");
        assert!(line.contains("\"schema_version\""), "{line}");
        assert!(line.contains("\"redactions\""), "{line}");
    }

    #[test]
    fn bounded_watch_events_retains_newest_events_and_records_trim_count() {
        let events = (1..=505)
            .map(|sequence| PackagingEvent {
                schema_version: PACKAGING_JSON_SCHEMA_VERSION,
                operation_id: "watch-1".to_string(),
                sequence,
                phase: PackagingPhase::TrySession,
                kind: PackagingEventKind::WatchDebounced,
                message: Some(format!("event {sequence}")),
                diagnostic: None,
                artifact: None,
                progress: None,
            })
            .collect::<Vec<_>>();

        let retained = bounded_watch_events("watch-1", &events, 500);

        assert_eq!(retained.len(), 500);
        assert_eq!(retained[0].sequence, 6);
        assert_eq!(retained[0].kind, PackagingEventKind::WatchRefreshSkipped);
        assert_eq!(
            retained[0].message.as_deref(),
            Some("5 older watch events were omitted from this operation record")
        );
        assert_eq!(retained.last().unwrap().sequence, 505);
    }

    #[test]
    fn write_packaging_record_if_possible_redacts_before_persisting() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();

        let temp = tempfile::TempDir::new().unwrap();
        let previous = std::env::var_os("CONARY_PACKAGING_OPERATIONS_DIR");
        unsafe { std::env::set_var("CONARY_PACKAGING_OPERATIONS_DIR", temp.path()) };

        let diagnostic = PackagingDiagnostic::error(
            PackagingPhase::Build,
            PackagingDiagnosticCode::CookFailed,
            "API_TOKEN=sk-message-secret failed",
        )
        .with_evidence(
            DiagnosticEvidence::log("command", "Bearer record.secret").with_metadata(
                "uri",
                serde_json::json!("https://user:pass@example.invalid/pkg.ccs"),
            ),
        );
        let output = PackagingCommandOutput::failed("cook-redact", "conary cook", vec![diagnostic]);

        write_packaging_record_if_possible(&output);

        match previous {
            Some(value) => unsafe { std::env::set_var("CONARY_PACKAGING_OPERATIONS_DIR", value) },
            None => unsafe { std::env::remove_var("CONARY_PACKAGING_OPERATIONS_DIR") },
        }

        let record_text = std::fs::read_to_string(temp.path().join("cook-redact.json")).unwrap();
        assert!(!record_text.contains("sk-message-secret"));
        assert!(!record_text.contains("record.secret"));
        assert!(!record_text.contains("user:pass"));
        assert!(record_text.contains("[REDACTED]"));
    }

    #[test]
    fn every_publish_gate_failure_code_maps_to_packaging_diagnostic_code() {
        use conary_core::repository::static_repo::publish_gate::PublishGateFailureCode;

        let codes = [
            PublishGateFailureCode::MissingAttestation,
            PublishGateFailureCode::BuildAttestationSignatureMismatch,
            PublishGateFailureCode::PackageSignatureMismatch,
            PublishGateFailureCode::TomlIntegrityMismatch,
            PublishGateFailureCode::OutputIdentityMismatch,
            PublishGateFailureCode::UnacceptedSignerKey,
            PublishGateFailureCode::RetiredSignerKey,
            PublishGateFailureCode::AbsentOrUnknownProvenanceClass,
            PublishGateFailureCode::NonHermeticHardeningLevel,
            PublishGateFailureCode::StaleOrUnknownPolicy,
            PublishGateFailureCode::UncleanCommandRiskReport,
            PublishGateFailureCode::ForeignConversionMissingBoundary,
            PublishGateFailureCode::ForeignConversionBoundaryHashMismatch,
            PublishGateFailureCode::RecordedDraftArtifact,
        ];

        for code in codes {
            assert_eq!(
                publish_gate_code_to_diagnostic_code(code),
                PackagingDiagnosticCode::PublishGateFailed
            );
        }
    }

    #[test]
    fn ccs_v2_diagnostics_map_to_packaging_diagnostics() {
        let diagnostic = conary_core::ccs::v2::V2Diagnostic::error(
            conary_core::ccs::v2::V2DiagnosticCode::LegacyV1Package,
            "legacy package",
            Some("format_version".to_string()),
            "rebuild as v2",
        );
        let rendered = ccs_v2_diagnostic_to_packaging(&diagnostic);
        assert_eq!(
            rendered.code,
            conary_core::diagnostics::PackagingDiagnosticCode::CcsV2LegacyRejected
        );
        assert_eq!(rendered.suggestions[0].message, "rebuild as v2");
    }
}
