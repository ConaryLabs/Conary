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
use conary_core::recipe::recording::{
    TraceOperation, TraceScope, suggest_capabilities_from_evidence,
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
    let capability_suggestions = suggest_capabilities_from_evidence(&input.installed_files);

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
        capability_suggestions,
        ignored_events: input.ignored_events,
        redactions,
        limitations: input.limitations,
    })
}

pub(crate) fn installed_file_evidence(install_root: &Path) -> Result<Vec<InstalledFileEvidence>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(install_root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }
        let relative = entry.path().strip_prefix(install_root)?;
        if entry.file_type().is_symlink() {
            let link_target = std::fs::read_link(entry.path())?;
            files.push(InstalledFileEvidence {
                path: relative.to_string_lossy().to_string(),
                file_type: "symlink".to_string(),
                executable: false,
                size: 0,
                link_target: Some(link_target.to_string_lossy().to_string()),
            });
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let metadata = entry.metadata()?;
        files.push(InstalledFileEvidence {
            path: relative.to_string_lossy().to_string(),
            file_type: "file".to_string(),
            executable: executable_bit(&metadata),
            size: metadata.len(),
            link_target: None,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn executable_bit(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

pub(crate) fn reconcile_installed_scan_with_trace(
    installed_files: &[InstalledFileEvidence],
    observed_paths: &[ObservedPath],
) -> (Vec<RecordingLimitation>, Vec<IgnoredEvent>) {
    use std::collections::HashSet;

    let installed = installed_files
        .iter()
        .filter(|file| matches!(file.file_type.as_str(), "file" | "symlink"))
        .map(|file| file.path.clone())
        .collect::<HashSet<_>>();
    let observed_writes = observed_paths
        .iter()
        .filter(|path| {
            path.scope == TraceScope::Install
                && matches!(
                    path.operation,
                    TraceOperation::InstallCreate | TraceOperation::InstallModify
                )
        })
        .map(|path| path.path.clone())
        .collect::<HashSet<_>>();
    let missing = installed.difference(&observed_writes).count();
    if missing == 0 {
        return (Vec::new(), Vec::new());
    }
    (
        vec![RecordingLimitation::EventLoss],
        vec![IgnoredEvent {
            reason: "installed-scan-reconciled-missing-watch-event".to_string(),
            count: missing as u64,
        }],
    )
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

    #[test]
    fn installed_scan_records_files_symlinks_and_executable_bits() {
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("destdir");
        std::fs::create_dir_all(install.join("usr/bin")).unwrap();
        let bin = install.join("usr/bin/demo");
        std::fs::write(&bin, "bin").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&bin).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&bin, permissions).unwrap();
            std::os::unix::fs::symlink("demo", install.join("usr/bin/demo-link")).unwrap();
        }

        let files = installed_file_evidence(&install).unwrap();

        assert!(
            files
                .iter()
                .any(|file| file.path == "usr/bin/demo" && file.executable)
        );
        #[cfg(unix)]
        assert!(files.iter().any(|file| file.path == "usr/bin/demo-link"
            && file.file_type == "symlink"
            && file.link_target.as_deref() == Some("demo")));
    }
}
