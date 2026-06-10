// apps/conary/src/commands/remove/scriptlets.rs

use std::path::Path;

use anyhow::Result;
use conary_core::scriptlet::{
    ExecutionMode, SandboxMode, ScriptletExecutor, ScriptletFailureKind, ScriptletFailureOutcome,
    ScriptletOutcome,
};
use tracing::{info, warn};

use super::legacy_replay::{
    build_legacy_replay_audit_for_remove, execute_legacy_remove_replay_plan_entries,
};
use super::types::RemoveInnerResult;
use crate::commands::ScriptletWarning;
use crate::commands::progress::{RemovePhase, RemoveProgress};

pub(super) fn run_post_remove_scriptlet(
    conn: &rusqlite::Connection,
    remove_result: &RemoveInnerResult,
    root: &str,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    progress: &RemoveProgress,
) -> Result<()> {
    // Execute post-remove scriptlet (best effort - warn on failure, don't abort)
    if no_scripts {
        return Ok(());
    }

    let has_legacy_post = remove_result
        .planned_post_remove
        .as_ref()
        .is_some_and(|plan| !plan.lifecycle_entries.is_empty());
    let post = remove_result
        .stored_scriptlets
        .iter()
        .find(|s| s.phase == "post-remove");
    if !has_legacy_post && post.is_none() {
        return Ok(());
    }

    progress.set_phase(RemovePhase::PostScript);

    let legacy_post_outcomes = execute_legacy_remove_replay_plan_entries(
        Path::new(root),
        &remove_result.trove.name,
        &remove_result.trove.version,
        remove_result.legacy_bundle.as_ref(),
        remove_result.planned_post_remove.as_ref(),
        sandbox_mode,
    )?;
    let legacy_post_warnings =
        legacy_post_replay_warnings(&remove_result.trove.name, &legacy_post_outcomes)?;
    if !legacy_post_warnings.is_empty() {
        crate::commands::append_scriptlet_warning_metadata(
            conn,
            remove_result.changeset_id,
            legacy_post_warnings,
        )?;
    }
    if let Some(audit) = build_legacy_replay_audit_for_remove(
        remove_result,
        &remove_result.legacy_pre_outcomes,
        &legacy_post_outcomes,
    ) {
        crate::commands::append_legacy_replay_audit_metadata(
            conn,
            remove_result.changeset_id,
            audit,
        )?;
    }

    let executor = ScriptletExecutor::new(
        Path::new(root),
        &remove_result.trove.name,
        &remove_result.trove.version,
        remove_result.scriptlet_format,
    )
    .with_sandbox_mode(sandbox_mode);

    if let Some(post) = post {
        info!("Running post-remove scriptlet...");
        match executor.execute_entry_with_outcome(post, &ExecutionMode::Remove) {
            ScriptletOutcome::Success { .. } | ScriptletOutcome::Skipped { .. } => {}
            ScriptletOutcome::Failure(failure)
                if failure.failure_kind == ScriptletFailureKind::ScriptExited =>
            {
                warn!(
                    "Post-remove scriptlet failed: {}. Package files already removed.",
                    failure.message
                );
                eprintln!("WARNING: Post-remove scriptlet failed: {}", failure.message);
                crate::commands::append_scriptlet_warning_metadata(
                    conn,
                    remove_result.changeset_id,
                    vec![scriptlet_warning_from_failure(
                        &remove_result.trove.name,
                        failure,
                        "post-remove scriptlet failed after package files were removed",
                    )],
                )?;
            }
            ScriptletOutcome::Failure(failure) => {
                return Err(anyhow::anyhow!(
                    "Post-remove scriptlet failed after commit with a non-degradable failure: {}",
                    failure.message
                ));
            }
        }
    }

    Ok(())
}

fn scriptlet_warning_from_failure(
    package: &str,
    failure: ScriptletFailureOutcome,
    context: &str,
) -> ScriptletWarning {
    ScriptletWarning::new(
        failure.phase,
        package,
        failure.failure_kind.as_str(),
        failure.requested_sandbox_mode.as_str(),
        failure.effective_sandbox.as_str(),
        format!("{context}: {}", failure.message),
    )
}

fn legacy_post_replay_warnings(
    package_name: &str,
    outcomes: &[ScriptletOutcome],
) -> Result<Vec<ScriptletWarning>> {
    let mut warnings = Vec::new();

    for outcome in outcomes {
        match outcome {
            ScriptletOutcome::Success { .. } | ScriptletOutcome::Skipped { .. } => {}
            ScriptletOutcome::Failure(failure)
                if failure.failure_kind == ScriptletFailureKind::ScriptExited =>
            {
                warnings.push(scriptlet_warning_from_failure(
                    package_name,
                    failure.clone(),
                    "legacy post-remove scriptlet failed after package files were removed",
                ));
            }
            ScriptletOutcome::Failure(failure) => {
                return Err(anyhow::anyhow!(
                    "legacy post-remove scriptlet failed after commit with a non-degradable failure: {}",
                    failure.message
                ));
            }
        }
    }

    Ok(warnings)
}
