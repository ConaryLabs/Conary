// apps/conary/src/commands/diagnostics.rs

#![cfg_attr(not(test), allow(dead_code))]

use std::io::Write;

use anyhow::Result;
use conary_core::diagnostics::{
    DiagnosticEvidence, PackagingArtifact, PackagingCommandOutput, PackagingDiagnostic,
    PackagingSeverity,
};
use conary_core::diagnostics::redaction::{
    redact_command, redact_json_value, redact_log, redact_text,
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
    let dir = match super::operation_records::default_packaging_operations_dir() {
        Ok(dir) => dir,
        Err(error) => {
            tracing::warn!(%error, "failed to resolve packaging operation record directory");
            return;
        }
    };
    let output = redacted_packaging_output(output);
    if let Err(error) =
        super::operation_records::write_packaging_record_unchecked(&dir, &output.operation_id, &output)
    {
        tracing::warn!(%error, "failed to write packaging operation record");
    }
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
        if let Some(message) = &mut event.message {
            let redacted = redact_text(message);
            *message = redacted.value;
        }
        if let Some(artifact) = &mut event.artifact {
            redact_artifact(artifact);
        }
        if let Some(diagnostic) = &mut event.diagnostic {
            redact_diagnostic(diagnostic);
        }
    }
    output
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
        DiagnosticEvidence, PackagingArtifact, PackagingCommandOutput, PackagingDiagnostic,
        PackagingDiagnosticCode, PackagingEvent, PackagingPhase,
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
        let output =
            PackagingCommandOutput::failed("cook-redact", "conary cook", vec![diagnostic]);

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
}
