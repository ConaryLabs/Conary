// apps/conary/src/commands/record_mode/mod.rs

mod draft;
mod fanotify_backend;
mod inotify_backend;
mod report;
mod runner;
mod trace;
mod types;
mod validation;
mod workspace;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::diagnostics::{
    PackagingArtifact, PackagingCommandOutput, PackagingCommandStatus, PackagingDiagnostic,
    PackagingDiagnosticCode, PackagingEventKind,
};
use conary_core::recipe::recording::{
    IgnoredEvent, RecordingLimitation, ScopeRoot, SelectedBackend, TraceScope as ReportTraceScope,
};

pub(crate) use report::reconcile_installed_scan_with_trace;
pub(crate) use types::{RecordCliRequest, RequestedRecordBackend};

use draft::DraftMaterialization;
use fanotify_backend::FanotifyTraceBackend;
use inotify_backend::InotifyTraceBackend;
use runner::RecordCommandRequest;
use trace::{TraceBackend, TraceDrain, TraceScope, TraceSession};
use workspace::RecordWorkspace;

pub(crate) async fn cmd_cook_record(request: RecordCliRequest) -> Result<()> {
    validate_record_request(&request)?;
    let operation_id = new_record_operation_id();
    let mut early_diagnostics = Vec::new();
    if let Err(error) = workspace::cleanup_stale_workspaces(&std::env::temp_dir()) {
        early_diagnostics.push(report::record_error(
            PackagingDiagnosticCode::RecordCleanupFailed,
            format!("failed to clean stale record workspaces: {error}"),
        ));
    }

    let result =
        match RecordWorkspace::create(&request.source, &request.output_dir, request.keep_raw_trace)
        {
            Ok(workspace) => {
                let result =
                    run_record_operation(&request, &operation_id, &workspace, early_diagnostics);
                let cleanup = workspace.cleanup();
                match (result, cleanup) {
                    (Ok(output), Ok(())) => Ok(output),
                    (Ok(mut output), Err(error)) => {
                        output.status = PackagingCommandStatus::Failed;
                        output.diagnostics.push(report::record_error(
                            PackagingDiagnosticCode::RecordCleanupFailed,
                            format!("failed to clean record workspace: {error}"),
                        ));
                        Ok(output)
                    }
                    (Err(error), Ok(())) => Err(error),
                    (Err(error), Err(cleanup_error)) => Err(error.context(format!(
                        "record workspace cleanup also failed: {cleanup_error}"
                    ))),
                }
            }
            Err(error) => Ok(record_failed_output(
                &operation_id,
                PackagingDiagnosticCode::RecordTraceFailed,
                format!("failed to create record workspace: {error}"),
            )),
        }?;

    super::diagnostics::write_packaging_record_if_possible(&result);
    if request.json {
        let mut stdout = std::io::stdout();
        super::diagnostics::write_packaging_output(&result, true, &mut stdout)?;
    } else {
        print_human_record_output(&request.output_dir, &result);
    }

    if result.status == PackagingCommandStatus::Failed {
        bail!(
            "{}",
            result
                .summary
                .clone()
                .unwrap_or_else(|| "record mode failed".to_string())
        );
    }
    Ok(())
}

fn validate_record_request(request: &RecordCliRequest) -> Result<()> {
    if request.command.is_empty() {
        bail!("record mode requires a command after `--`");
    }
    if request.allow_network {
        bail!("--record-allow-network is reserved for a later record-mode slice");
    }
    Ok(())
}

fn run_record_operation(
    request: &RecordCliRequest,
    operation_id: &str,
    workspace: &RecordWorkspace,
    mut diagnostics: Vec<PackagingDiagnostic>,
) -> Result<PackagingCommandOutput> {
    let mut events = Vec::new();
    let mut sequence = 0;
    push_record_event(
        &mut events,
        operation_id,
        &mut sequence,
        PackagingEventKind::RecordStarted,
        "Record mode started",
    );

    let trace_scope = trace_scope_for_workspace(workspace)?;
    let fanotify = FanotifyTraceBackend::new();
    let inotify = InotifyTraceBackend::new();
    let backend_status = trace::select_backend(request.backend, &fanotify, &inotify, &trace_scope)?;
    let backend = backend_status.backend;
    push_record_event(
        &mut events,
        operation_id,
        &mut sequence,
        PackagingEventKind::RecordBackendSelected,
        format!("Selected {backend:?} trace backend"),
    );

    let mut limitations = backend_status
        .limitations
        .iter()
        .map(|limitation| limitation.to_report_limitation())
        .collect::<Vec<_>>();
    let mut trace_session = start_trace_session(backend, trace_scope, &fanotify, &inotify)
        .context("failed to start record trace backend")?;

    if request.unsafe_host {
        eprintln!("WARNING: executing record command directly on the host without sandboxing.");
        push_limitation(&mut limitations, RecordingLimitation::UnsafeHost);
    }

    push_record_event(
        &mut events,
        operation_id,
        &mut sequence,
        PackagingEventKind::RecordCommandStarted,
        "Record command started",
    );
    let command_outcome = runner::run_record_command(&RecordCommandRequest {
        source_root: workspace.source_root.clone(),
        work_root: workspace.work_root.clone(),
        install_root: workspace.install_root.clone(),
        command: request.command.clone(),
        unsafe_host: request.unsafe_host,
    })?;
    push_record_event(
        &mut events,
        operation_id,
        &mut sequence,
        PackagingEventKind::RecordCommandFinished,
        format!("Record command exited with {}", command_outcome.exit_code),
    );

    let trace_drain = match trace_session.finish() {
        Ok(drain) => drain,
        Err(error) => {
            push_limitation(&mut limitations, RecordingLimitation::EventLoss);
            diagnostics.push(report::record_error(
                PackagingDiagnosticCode::RecordTraceFailed,
                format!("failed to finish trace collection: {error}"),
            ));
            TraceDrain::default()
        }
    };
    push_record_event(
        &mut events,
        operation_id,
        &mut sequence,
        PackagingEventKind::RecordTraceFinished,
        "Trace collection finished",
    );

    let _raw_trace_path_count = trace_drain
        .events
        .iter()
        .filter(|event| event.path.is_absolute())
        .count();
    let observed_paths = trace_drain
        .events
        .into_iter()
        .map(|event| event.observed)
        .collect::<Vec<_>>();
    let mut ignored_events = Vec::new();
    if trace_drain.ignored_events > 0 {
        ignored_events.push(IgnoredEvent {
            reason: "trace-backend-ignored-events".to_string(),
            count: trace_drain.ignored_events,
        });
    }
    if trace_drain.event_loss {
        push_limitation(
            &mut limitations,
            trace::TraceLimitation::EventLoss.to_report_limitation(),
        );
    }

    workspace.publish_source_snapshot()?;
    let installed_files = report::installed_file_evidence(&workspace.install_root)?;
    let (scan_limitations, scan_ignored) =
        reconcile_installed_scan_with_trace(&installed_files, &observed_paths);
    for limitation in scan_limitations {
        push_limitation(&mut limitations, limitation);
    }
    ignored_events.extend(scan_ignored);

    if command_outcome.exit_code != 0 {
        push_limitation(&mut limitations, RecordingLimitation::CommandFailed);
        diagnostics.push(report::record_error(
            PackagingDiagnosticCode::RecordCommandFailed,
            format!("record command exited with {}", command_outcome.exit_code),
        ));
    }

    let recipe_path = draft::materialize_draft_recipe(DraftMaterialization {
        output_dir: workspace.output_dir.clone(),
        package_name: package_name_for_source(&request.source),
        package_version: "0.1.0-recorded".to_string(),
        command: request.command.clone(),
        recording_destdir: workspace.install_root.to_string_lossy().to_string(),
        installed_files: installed_files.clone(),
        network_likely: false,
    })?;
    diagnostics.push(PackagingDiagnostic::info(
        conary_core::diagnostics::PackagingPhase::RecordMode,
        PackagingDiagnosticCode::RecordDraftGenerated,
        format!("draft recipe written to {}", recipe_path.display()),
    ));
    push_record_event(
        &mut events,
        operation_id,
        &mut sequence,
        PackagingEventKind::RecordDraftGenerated,
        "Draft recipe generated",
    );

    let mut validation_failed = false;
    if request.validate {
        push_record_event(
            &mut events,
            operation_id,
            &mut sequence,
            PackagingEventKind::RecordValidationStarted,
            "Recorded draft validation started",
        );
        match validation::validate_recorded_draft(&workspace.output_dir, operation_id) {
            Ok(validation_output)
                if validation_output.status == PackagingCommandStatus::Succeeded =>
            {
                events.extend(validation_output.events);
            }
            Ok(validation_output) => {
                validation_failed = true;
                push_limitation(&mut limitations, RecordingLimitation::ValidationFailed);
                diagnostics.push(report::record_error(
                    PackagingDiagnosticCode::RecordValidationFailed,
                    validation_output
                        .summary
                        .unwrap_or_else(|| "recorded draft validation failed".to_string()),
                ));
                events.extend(validation_output.events);
            }
            Err(error) => {
                validation_failed = true;
                push_limitation(&mut limitations, RecordingLimitation::ValidationFailed);
                diagnostics.push(report::record_error(
                    PackagingDiagnosticCode::RecordValidationFailed,
                    format!("recorded draft validation failed: {error}"),
                ));
            }
        }
        push_record_event(
            &mut events,
            operation_id,
            &mut sequence,
            PackagingEventKind::RecordValidationFinished,
            "Recorded draft validation finished",
        );
    } else {
        push_limitation(&mut limitations, RecordingLimitation::ValidationSkipped);
    }

    let recording_report = report::build_recording_report(report::ReportInput {
        operation_id: operation_id.to_string(),
        backend,
        command: request.command.clone(),
        command_exit: Some(command_outcome.exit_code),
        observed_paths,
        installed_files,
        limitations,
        ignored_events,
        private_prefixes: vec![workspace.private_root.clone()],
    })?;
    report::write_report_files(&workspace.output_dir, &recording_report)?;

    let success = command_outcome.exit_code == 0 && !validation_failed;
    push_record_event(
        &mut events,
        operation_id,
        &mut sequence,
        PackagingEventKind::RecordFinished,
        if success {
            "Record mode finished"
        } else {
            "Record mode finished with failures"
        },
    );
    let mut output = report::record_command_output(
        operation_id,
        success,
        diagnostics,
        events,
        if success {
            format!(
                "recorded draft written to {}",
                workspace.output_dir.display()
            )
        } else {
            "record mode failed".to_string()
        },
    );
    output.artifacts.push(PackagingArtifact {
        path: workspace
            .output_dir
            .join("recipe.toml")
            .display()
            .to_string(),
        kind: Some("recipe".to_string()),
    });
    output.artifacts.push(PackagingArtifact {
        path: workspace
            .output_dir
            .join("trace-report.json")
            .display()
            .to_string(),
        kind: Some("trace-report".to_string()),
    });
    Ok(output)
}

fn trace_scope_for_workspace(workspace: &RecordWorkspace) -> Result<TraceScope> {
    Ok(TraceScope {
        source: ScopeRoot::new(ReportTraceScope::Source, &workspace.source_root)?,
        work: ScopeRoot::new(ReportTraceScope::Work, &workspace.work_root)?,
        install: ScopeRoot::new(ReportTraceScope::Install, &workspace.install_root)?,
    })
}

fn start_trace_session(
    backend: SelectedBackend,
    scope: TraceScope,
    fanotify: &FanotifyTraceBackend,
    inotify: &InotifyTraceBackend,
) -> Result<Box<dyn TraceSession>> {
    match backend {
        SelectedBackend::Fanotify => fanotify.start(scope),
        SelectedBackend::Inotify => inotify.start(scope),
        SelectedBackend::FanotifyInotify => Ok(Box::new(CombinedTraceSession {
            sessions: vec![fanotify.start(scope.clone())?, inotify.start(scope)?],
        })),
    }
}

struct CombinedTraceSession {
    sessions: Vec<Box<dyn TraceSession>>,
}

impl TraceSession for CombinedTraceSession {
    fn drain_events(&mut self) -> Result<TraceDrain> {
        let mut combined = TraceDrain::default();
        for session in &mut self.sessions {
            merge_trace_drain(&mut combined, session.drain_events()?);
        }
        Ok(combined)
    }

    fn finish(&mut self) -> Result<TraceDrain> {
        let mut combined = TraceDrain::default();
        for session in &mut self.sessions {
            merge_trace_drain(&mut combined, session.finish()?);
        }
        Ok(combined)
    }
}

fn merge_trace_drain(target: &mut TraceDrain, drain: TraceDrain) {
    target.events.extend(drain.events);
    target.ignored_events += drain.ignored_events;
    target.event_loss |= drain.event_loss;
}

fn push_record_event(
    events: &mut Vec<conary_core::diagnostics::PackagingEvent>,
    operation_id: &str,
    sequence: &mut u64,
    kind: PackagingEventKind,
    message: impl Into<String>,
) {
    events.push(report::record_event(operation_id, *sequence, kind, message));
    *sequence += 1;
}

fn push_limitation(limitations: &mut Vec<RecordingLimitation>, limitation: RecordingLimitation) {
    if !limitations.contains(&limitation) {
        limitations.push(limitation);
    }
}

fn record_failed_output(
    operation_id: &str,
    code: PackagingDiagnosticCode,
    message: String,
) -> PackagingCommandOutput {
    let diagnostic = report::record_error(code, message.clone());
    report::record_command_output(
        operation_id,
        false,
        vec![diagnostic],
        vec![report::record_event(
            operation_id,
            0,
            PackagingEventKind::RecordFinished,
            "Record mode failed",
        )],
        message,
    )
}

fn print_human_record_output(output_dir: &Path, output: &PackagingCommandOutput) {
    match output.status {
        PackagingCommandStatus::Succeeded => println!("Recorded draft: {}", output_dir.display()),
        PackagingCommandStatus::Failed => println!("Record mode failed: {}", output_dir.display()),
    }
    println!("Recipe: {}", output_dir.join("recipe.toml").display());
    println!(
        "Trace report: {}",
        output_dir.join("trace-report.json").display()
    );
}

fn new_record_operation_id() -> String {
    crate::commands::operation_records::new_operation_id("record")
}

pub(crate) fn default_record_output_dir(source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && *value != ".")
        .unwrap_or("source");
    PathBuf::from("recorded").join(name)
}

fn package_name_for_source(source: &Path) -> String {
    source
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && *value != ".")
        .unwrap_or("recorded")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command: Vec<String>) -> RecordCliRequest {
        RecordCliRequest {
            source: ".".into(),
            output_dir: "recorded/demo".into(),
            backend: RequestedRecordBackend::Auto,
            validate: false,
            keep_raw_trace: false,
            unsafe_host: false,
            allow_network: false,
            json: false,
            command,
        }
    }

    #[test]
    fn record_request_rejects_missing_command() {
        let error = validate_record_request(&request(Vec::new())).unwrap_err();
        assert!(error.to_string().contains("requires a command"));
    }

    #[test]
    fn record_request_rejects_reserved_network_flag() {
        let mut request = request(vec!["make".to_string()]);
        request.allow_network = true;
        let error = validate_record_request(&request).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }

    #[test]
    fn operation_id_uses_record_prefix() {
        let id = new_record_operation_id();
        assert!(id.starts_with("record-"));
    }

    #[test]
    fn default_output_dir_uses_source_directory_name() {
        assert_eq!(
            default_record_output_dir(std::path::Path::new("/tmp/demo")),
            std::path::PathBuf::from("recorded/demo")
        );
        assert_eq!(
            default_record_output_dir(std::path::Path::new(".")),
            std::path::PathBuf::from("recorded/source")
        );
    }

    #[test]
    fn installed_scan_reconciliation_marks_unobserved_installed_path() {
        use conary_core::recipe::recording::{
            InstalledFileEvidence, ObservedPath, RecordingLimitation, TraceOperation, TraceScope,
        };

        let installed = vec![InstalledFileEvidence {
            path: "usr/bin/demo".to_string(),
            file_type: "file".to_string(),
            executable: true,
            size: 3,
            link_target: None,
        }];
        let observed = vec![ObservedPath {
            scope: TraceScope::Install,
            operation: TraceOperation::InstallCreate,
            path: "usr/bin/other".to_string(),
        }];

        let (limitations, ignored) = reconcile_installed_scan_with_trace(&installed, &observed);

        assert!(limitations.contains(&RecordingLimitation::EventLoss));
        assert!(ignored.iter().any(|event| {
            event.reason == "installed-scan-reconciled-missing-watch-event" && event.count == 1
        }));
    }
}
