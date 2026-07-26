// apps/conary/src/commands/try_session/watch/reporting.rs
//! Event rendering, failure reporting, and watch-session finalization.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::diagnostics::{
    DiagnosticEvidence, PackagingCommandStatus, PackagingDiagnostic, PackagingDiagnosticCode,
    PackagingEvent, PackagingEventKind, PackagingPhase,
};

use super::super::rollback_active_try_session;
use super::{WatchEvents, WatchLoopConfig};

pub(super) fn emit_push(
    events: &mut WatchEvents,
    phase: PackagingPhase,
    kind: PackagingEventKind,
    message: impl Into<String>,
    json: bool,
    output: &mut impl Write,
) -> Result<()> {
    let event = events.push(phase, kind, message);
    write_event(&event, json, output)
}

pub(super) fn emit_diagnostic(
    events: &mut WatchEvents,
    diagnostic: PackagingDiagnostic,
    json: bool,
    output: &mut impl Write,
) -> Result<()> {
    let event = events.diagnostic(diagnostic);
    write_event(&event, json, output)
}

pub(super) struct WatchFailure<'a> {
    pub(super) phase: PackagingPhase,
    pub(super) code: PackagingDiagnosticCode,
    pub(super) message: &'a str,
    pub(super) error: &'a anyhow::Error,
}

pub(super) fn emit_non_destructive_failure(
    events: &mut WatchEvents,
    config: &WatchLoopConfig,
    failure: WatchFailure<'_>,
    json: bool,
    output: &mut impl Write,
) -> Result<()> {
    emit_diagnostic(
        events,
        watch_error(failure.phase, failure.code, failure.message, failure.error),
        json,
        output,
    )?;
    emit_push(
        events,
        failure.phase,
        PackagingEventKind::WatchRefreshFailed,
        failure.message,
        json,
        output,
    )?;
    write_optional_file(&config.failure_file, &failure.error.to_string())
}

pub(super) fn watch_error(
    phase: PackagingPhase,
    code: PackagingDiagnosticCode,
    message: impl Into<String>,
    error: &anyhow::Error,
) -> PackagingDiagnostic {
    PackagingDiagnostic::error(phase, code, message)
        .with_evidence(DiagnosticEvidence::log("error", format!("{error:#}")))
}

pub(super) fn fail_before_session(
    mut events: WatchEvents,
    phase: PackagingPhase,
    code: PackagingDiagnosticCode,
    message: &'static str,
    error: anyhow::Error,
    json: bool,
    output: &mut impl Write,
) -> Result<()> {
    emit_diagnostic(
        &mut events,
        watch_error(phase, code, message, &error),
        json,
        output,
    )?;
    finish_watch(
        events,
        PackagingCommandStatus::Failed,
        "try watch failed before startup completed",
        json,
        output,
    )?;
    Err(error)
}

pub(super) fn rollback_or_fail(
    db_path: &str,
    events: &mut WatchEvents,
    success_message: &str,
    json: bool,
    output: &mut impl Write,
) -> Result<()> {
    match rollback_active_try_session(db_path) {
        Ok(()) => {
            emit_push(
                events,
                PackagingPhase::TrySession,
                PackagingEventKind::WatchCancelled,
                success_message,
                json,
                output,
            )?;
            Ok(())
        }
        Err(error) => {
            emit_diagnostic(
                events,
                PackagingDiagnostic::error(
                    PackagingPhase::TrySession,
                    PackagingDiagnosticCode::WatchCleanupFailed,
                    "try watch rollback failed; run `conary try status` and `conary try rollback`",
                )
                .with_evidence(DiagnosticEvidence::log(
                    "rollback error",
                    format!("{error:#}"),
                )),
                json,
                output,
            )?;
            Err(error)
        }
    }
}

pub(super) fn finish_watch(
    mut events: WatchEvents,
    status: PackagingCommandStatus,
    summary: impl Into<String>,
    json: bool,
    output: &mut impl Write,
) -> Result<()> {
    let summary = summary.into();
    let event = events.operation_finished(summary.clone());
    write_event(&event, json, output)?;
    let record = events.into_command_output(status, summary);
    crate::commands::diagnostics::write_packaging_record_if_possible(&record);
    Ok(())
}

pub(super) fn finish_current_events(
    events: &mut WatchEvents,
    status: PackagingCommandStatus,
    summary: impl Into<String>,
    json: bool,
    output: &mut impl Write,
) -> Result<()> {
    let summary = summary.into();
    let event = events.operation_finished(summary.clone());
    write_event(&event, json, output)?;
    let record = WatchEvents {
        operation_id: events.operation_id.clone(),
        next_sequence: events.next_sequence,
        events: events.events.clone(),
        diagnostics: events.diagnostics.clone(),
    }
    .into_command_output(status, summary);
    crate::commands::diagnostics::write_packaging_record_if_possible(&record);
    Ok(())
}

fn write_event(event: &PackagingEvent, json: bool, output: &mut impl Write) -> Result<()> {
    if json {
        output.write_all(
            crate::commands::diagnostics::render_packaging_event_ndjson(event)?.as_bytes(),
        )?;
    } else if let Some(message) = &event.message {
        writeln!(output, "{message}")?;
    }
    Ok(())
}

pub(super) fn write_optional_file(path: &Option<PathBuf>, contents: &str) -> Result<()> {
    if let Some(path) = path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}
