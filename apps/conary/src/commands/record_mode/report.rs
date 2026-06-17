// apps/conary/src/commands/record_mode/report.rs

use std::path::{Path, PathBuf};

use anyhow::Result;
use conary_core::diagnostics::redaction::redact_command;
use conary_core::diagnostics::{
    PACKAGING_JSON_SCHEMA_VERSION, PackagingCommandOutput, PackagingCommandStatus,
    PackagingDiagnostic, PackagingDiagnosticCode, PackagingEvent, PackagingEventKind,
    PackagingPhase,
};
use conary_core::recipe::recording::{
    IgnoredEvent, InstalledFileEvidence, ObservedPath, RecordingLimitation, RecordingReport,
    ScopeRootLabel, SelectedBackend,
};

pub(crate) struct ReportInput {
    pub(crate) operation_id: String,
    pub(crate) backend: SelectedBackend,
    pub(crate) command: Vec<String>,
    pub(crate) command_exit: Option<i32>,
    pub(crate) observed_paths: Vec<ObservedPath>,
    pub(crate) installed_files: Vec<InstalledFileEvidence>,
    pub(crate) limitations: Vec<RecordingLimitation>,
    pub(crate) ignored_events: Vec<IgnoredEvent>,
    pub(crate) private_prefixes: Vec<PathBuf>,
}

pub(crate) fn build_recording_report(input: ReportInput) -> Result<RecordingReport> {
    let command = redact_command(&input.command);
    let command_summary = redact_private_prefixes(command.value, &input.private_prefixes);
    let mut redactions = command
        .redactions
        .into_iter()
        .map(|marker| marker.reason)
        .collect::<Vec<_>>();
    if command_summary
        .iter()
        .any(|value| value.contains("[PRIVATE-PATH]"))
    {
        redactions.push("private-path".to_string());
    }
    redactions.sort();
    redactions.dedup();

    Ok(RecordingReport {
        schema_version: 1,
        operation_id: input.operation_id,
        backend: input.backend,
        scope_roots: vec![
            ScopeRootLabel::Source,
            ScopeRootLabel::Work,
            ScopeRootLabel::Install,
        ],
        command_summary,
        command_exit: input.command_exit,
        observed_paths: input.observed_paths,
        installed_files: input.installed_files,
        inferred_build_steps: Vec::new(),
        inferred_install_steps: Vec::new(),
        capability_suggestions: Vec::new(),
        ignored_events: input.ignored_events,
        redactions,
        limitations: input.limitations,
    })
}

fn redact_private_prefixes(values: Vec<String>, private_prefixes: &[PathBuf]) -> Vec<String> {
    values
        .into_iter()
        .map(|value| {
            private_prefixes.iter().fold(value, |redacted, prefix| {
                let prefix = prefix.to_string_lossy();
                redacted.replace(prefix.as_ref(), "[PRIVATE-PATH]")
            })
        })
        .collect()
}

pub(crate) fn write_report_files(output_dir: &Path, report: &RecordingReport) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;
    let json = serde_json::to_string_pretty(report)?;
    std::fs::write(output_dir.join("trace-report.json"), json)?;
    std::fs::write(
        output_dir.join("trace-report.txt"),
        format!(
            "Recording backend: {:?}\nCommand exit: {:?}\nObserved paths: {}\n",
            report.backend,
            report.command_exit,
            report.observed_paths.len()
        ),
    )?;
    Ok(())
}

pub(crate) fn record_command_output(
    operation_id: &str,
    success: bool,
    diagnostics: Vec<PackagingDiagnostic>,
    events: Vec<PackagingEvent>,
    summary: impl Into<String>,
) -> PackagingCommandOutput {
    PackagingCommandOutput {
        schema_version: PACKAGING_JSON_SCHEMA_VERSION,
        operation_id: operation_id.to_string(),
        command: "conary cook --record".to_string(),
        status: if success {
            PackagingCommandStatus::Succeeded
        } else {
            PackagingCommandStatus::Failed
        },
        diagnostics,
        events,
        artifacts: Vec::new(),
        summary: Some(summary.into()),
    }
}

pub(crate) fn record_event(
    operation_id: &str,
    sequence: u64,
    kind: PackagingEventKind,
    message: impl Into<String>,
) -> PackagingEvent {
    PackagingEvent {
        schema_version: PACKAGING_JSON_SCHEMA_VERSION,
        operation_id: operation_id.to_string(),
        sequence,
        phase: PackagingPhase::RecordMode,
        kind,
        message: Some(message.into()),
        diagnostic: None,
        artifact: None,
        progress: None,
    }
}

pub(crate) fn record_error(
    code: PackagingDiagnosticCode,
    message: impl Into<String>,
) -> PackagingDiagnostic {
    PackagingDiagnostic::error(PackagingPhase::RecordMode, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::diagnostics::PackagingSeverity;
    use conary_core::recipe::recording::{TraceOperation, TraceScope};

    #[test]
    fn report_writer_redacts_command_and_private_paths() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("recorded/demo");
        let private_root = temp.path().join("conary-record-private");
        let report = build_recording_report(ReportInput {
            operation_id: "record-1".to_string(),
            backend: SelectedBackend::Inotify,
            command: vec![
                "curl".to_string(),
                "-H".to_string(),
                "Authorization: Bearer secret-token".to_string(),
                private_root
                    .join("destdir/usr/bin/demo")
                    .to_string_lossy()
                    .to_string(),
            ],
            command_exit: Some(0),
            observed_paths: vec![ObservedPath {
                scope: TraceScope::Install,
                operation: TraceOperation::InstallCreate,
                path: "usr/bin/demo".to_string(),
            }],
            installed_files: Vec::new(),
            limitations: vec![RecordingLimitation::IncompleteReadEvidence],
            ignored_events: Vec::new(),
            private_prefixes: vec![private_root.clone()],
        })
        .unwrap();

        write_report_files(&output_dir, &report).unwrap();
        let text = std::fs::read_to_string(output_dir.join("trace-report.json")).unwrap();
        assert!(text.contains("Bearer [REDACTED]"));
        assert!(!text.contains("secret-token"));
        assert!(!text.contains(private_root.to_str().unwrap()));
        assert!(text.contains("[PRIVATE-PATH]"));
        assert!(report.redactions.contains(&"bearer-token".to_string()));
        assert!(report.redactions.contains(&"private-path".to_string()));
    }

    #[test]
    fn command_output_finalizer_sets_record_status_and_events() {
        let diagnostic = record_error(
            PackagingDiagnosticCode::RecordCommandFailed,
            "command failed",
        );
        let event = record_event(
            "record-1",
            1,
            PackagingEventKind::RecordCommandFinished,
            "finished",
        );

        let output = record_command_output(
            "record-1",
            false,
            vec![diagnostic.clone()],
            vec![event.clone()],
            "recording failed",
        );

        assert_eq!(output.status, PackagingCommandStatus::Failed);
        assert_eq!(output.operation_id, "record-1");
        assert_eq!(output.diagnostics[0], diagnostic);
        assert_eq!(output.diagnostics[0].severity, PackagingSeverity::Error);
        assert_eq!(output.events[0], event);
        assert_eq!(output.summary.as_deref(), Some("recording failed"));
    }
}
