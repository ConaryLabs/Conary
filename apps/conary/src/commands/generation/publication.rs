// apps/conary/src/commands/generation/publication.rs

use anyhow::{Result, anyhow};
use conary_core::config_transaction::GenerationConfigTransaction;
use conary_core::db::generation_delta::{GenerationDbDeltaCapture, GenerationDbDeltaRecorder};
use conary_core::db::models::{
    GenerationPublication, GenerationPublicationPhase, GenerationPublicationStatus,
};
use conary_core::runtime_root::ConaryRuntimeRoot;
use rusqlite::Connection;

#[derive(Debug, Clone)]
pub(crate) struct PublicationRequest<'a> {
    pub db_path: &'a str,
    pub summary: &'a str,
    pub trigger_changeset_id: Option<i64>,
    pub tx_uuid: Option<&'a str>,
    pub config_transaction: GenerationConfigTransaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublicationOutcome {
    pub generation_number: Option<i64>,
    pub state_number: Option<i64>,
    pub needs_publication: bool,
    pub retry_command: Option<String>,
    pub failure_reason: Option<String>,
    pub completed_debts: usize,
}

pub(crate) const DEFAULT_PUBLICATION_RETRY_COMMAND: &str = "conary system generation publish --yes";

impl PublicationOutcome {
    pub(crate) fn default_retry_command() -> String {
        DEFAULT_PUBLICATION_RETRY_COMMAND.to_string()
    }
}

pub(crate) fn warn_if_publication_pending(changeset_id: i64, outcome: &PublicationOutcome) {
    if !outcome.needs_publication {
        return;
    }
    let retry = outcome
        .retry_command
        .as_deref()
        .unwrap_or(DEFAULT_PUBLICATION_RETRY_COMMAND);
    tracing::warn!(
        changeset_id,
        retry,
        "Package mutation committed, but generation publication is pending"
    );
    crate::ui::warn(&format!(
        "Package mutation committed, but generation publication is pending for changeset {changeset_id}."
    ));
    eprintln!("Run: {retry}");
}

/// Persist exact selected-root publication authority in the caller's
/// transaction.
pub(crate) fn record_selected_root_state(
    conn: &Connection,
    request: &PublicationRequest<'_>,
) -> Result<GenerationPublication> {
    let runtime_root = ConaryRuntimeRoot::from_db_path(request.db_path);
    let runtime_root_display = runtime_root.root().display().to_string();
    GenerationPublication::create_pending(
        conn,
        request.trigger_changeset_id,
        request.tx_uuid,
        request.db_path,
        &runtime_root_display,
        request.summary,
        &request.config_transaction,
    )
    .map_err(Into::into)
}

pub(crate) fn publish_recorded_selected_root(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    debt: GenerationPublication,
    delta_recorder: Option<&mut GenerationDbDeltaRecorder<'_>>,
) -> Result<PublicationOutcome> {
    publish_pending_debt(conn, db_path, summary, debt, delta_recorder)
}

pub(crate) fn retry_pending_publication(
    conn: &Connection,
    db_path: &str,
    summary: &str,
) -> Result<PublicationOutcome> {
    let debt = GenerationPublication::pending_recoverable(conn)?
        .into_iter()
        .last()
        .ok_or_else(|| anyhow!("no pending generation publication debt"))?;
    publish_pending_debt(conn, db_path, summary, debt, None)
}

fn publish_pending_debt(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    debt: GenerationPublication,
    delta_recorder: Option<&mut GenerationDbDeltaRecorder<'_>>,
) -> Result<PublicationOutcome> {
    publish_pending_debt_with_hook(conn, db_path, summary, debt, delta_recorder, |_| Ok(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationReplayCheckpoint {
    CurrentLinkSwappedBeforePhase,
    ActiveStateMarkedBeforeDatabaseBackup,
    DatabaseBackedUpBeforeTerminalState,
}

fn publish_pending_debt_with_hook(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    debt: GenerationPublication,
    mut delta_recorder: Option<&mut GenerationDbDeltaRecorder<'_>>,
    mut checkpoint: impl FnMut(PublicationReplayCheckpoint) -> Result<()>,
) -> Result<PublicationOutcome> {
    let runtime_root = ConaryRuntimeRoot::from_db_path(db_path);
    let high_water = GenerationPublication::applied_high_water_changeset_id(conn)?;
    let recoverable_before = GenerationPublication::pending_recoverable(conn)?;
    let config_transactions = recoverable_before
        .iter()
        .map(|publication| publication.config_transaction.clone())
        .collect::<Vec<_>>();

    let publish_result = replay_publication(
        conn,
        PublicationReplayRequest {
            db_path,
            summary,
            runtime_root: &runtime_root,
            debt: &debt,
            config_transactions: &config_transactions,
        },
        delta_recorder.as_deref_mut(),
        &mut checkpoint,
    )
    .and_then(|built| {
        checkpoint(PublicationReplayCheckpoint::DatabaseBackedUpBeforeTerminalState)?;
        let completed = debt.mark_complete_through(
            conn,
            high_water,
            built.state_number,
            built.generation_number,
        )?;
        if let Some(recorder) = delta_recorder {
            write_generation_db_backup(conn, db_path, &runtime_root, &built, Some(recorder))?;
        }
        Ok((built, completed))
    });

    match publish_result {
        Ok((built, completed)) => Ok(PublicationOutcome {
            generation_number: Some(built.generation_number),
            state_number: Some(built.state_number),
            needs_publication: false,
            retry_command: None,
            failure_reason: None,
            completed_debts: completed,
        }),
        Err(error) => {
            let failure_reason = error.to_string();
            debt.mark_failed(conn, &failure_reason)?;
            Ok(PublicationOutcome {
                generation_number: None,
                state_number: None,
                needs_publication: true,
                retry_command: Some(PublicationOutcome::default_retry_command()),
                failure_reason: Some(failure_reason),
                completed_debts: 0,
            })
        }
    }
}

struct PublicationReplayRequest<'a> {
    db_path: &'a str,
    summary: &'a str,
    runtime_root: &'a ConaryRuntimeRoot,
    debt: &'a GenerationPublication,
    config_transactions: &'a [GenerationConfigTransaction],
}

fn replay_publication(
    conn: &Connection,
    request: PublicationReplayRequest<'_>,
    mut delta_recorder: Option<&mut GenerationDbDeltaRecorder<'_>>,
    checkpoint: &mut impl FnMut(PublicationReplayCheckpoint) -> Result<()>,
) -> Result<BuiltForPublication> {
    let PublicationReplayRequest {
        db_path,
        summary,
        runtime_root,
        debt,
        config_transactions,
    } = request;
    let mut phase = debt.phase;
    let mut built = recorded_publication_numbers(debt)?;

    loop {
        match phase {
            GenerationPublicationPhase::PendingBuild | GenerationPublicationPhase::Building => {
                debt.set_phase(
                    conn,
                    GenerationPublicationPhase::Building,
                    GenerationPublicationStatus::Running,
                    None,
                    None,
                )?;
                let captured = super::selected_root::load_publication_selected_root(conn, debt)?;
                let result =
                    crate::commands::composefs_ops::build_captured_generation_for_publication(
                        conn,
                        db_path,
                        summary,
                        config_transactions,
                        captured,
                    )?;
                if result.state_number != result.generation_number {
                    return Err(anyhow!(
                        "generation builder returned mismatched state/generation numbers: state={} generation={}",
                        result.state_number,
                        result.generation_number
                    ));
                }
                built = Some(BuiltForPublication {
                    state_number: result.state_number,
                    generation_number: result.generation_number,
                });
                set_replay_phase(
                    conn,
                    debt,
                    GenerationPublicationPhase::ArtifactReady,
                    built.as_ref(),
                )?;
                phase = GenerationPublicationPhase::ArtifactReady;
            }
            GenerationPublicationPhase::ArtifactReady => {
                let built = required_publication_numbers(built.as_ref(), phase)?;
                crate::commands::composefs_ops::publish_generation_link(
                    db_path,
                    built.generation_number,
                )?;
                checkpoint(PublicationReplayCheckpoint::CurrentLinkSwappedBeforePhase)?;
                set_replay_phase(
                    conn,
                    debt,
                    GenerationPublicationPhase::CurrentPublished,
                    Some(built),
                )?;
                phase = GenerationPublicationPhase::CurrentPublished;
            }
            GenerationPublicationPhase::CurrentPublished => {
                let built = required_publication_numbers(built.as_ref(), phase)?;
                crate::commands::composefs_ops::publish_generation_link(
                    db_path,
                    built.generation_number,
                )?;
                crate::commands::generation::config_transaction::project_persisted_statuses(
                    conn,
                    config_transactions,
                )?;
                set_replay_phase(
                    conn,
                    debt,
                    GenerationPublicationPhase::ConfigurationProjected,
                    Some(built),
                )?;
                phase = GenerationPublicationPhase::ConfigurationProjected;
            }
            GenerationPublicationPhase::ConfigurationProjected => {
                let built = required_publication_numbers(built.as_ref(), phase)?;
                crate::commands::composefs_ops::publish_generation_link(
                    db_path,
                    built.generation_number,
                )?;
                crate::commands::composefs_ops::mark_generation_state_active(
                    conn,
                    built.generation_number,
                )?;
                set_replay_phase(
                    conn,
                    debt,
                    GenerationPublicationPhase::ActiveMarked,
                    Some(built),
                )?;
                phase = GenerationPublicationPhase::ActiveMarked;
            }
            GenerationPublicationPhase::ActiveMarked => {
                let built = required_publication_numbers(built.as_ref(), phase)?;
                crate::commands::composefs_ops::publish_generation_link(
                    db_path,
                    built.generation_number,
                )?;
                set_replay_phase(
                    conn,
                    debt,
                    GenerationPublicationPhase::ActiveMarked,
                    Some(built),
                )?;
                checkpoint(PublicationReplayCheckpoint::ActiveStateMarkedBeforeDatabaseBackup)?;
                write_generation_db_backup(
                    conn,
                    db_path,
                    runtime_root,
                    built,
                    delta_recorder.as_deref_mut(),
                )
                .map_err(|error| anyhow!("failed to write generation DB backup: {error}"))?;
                set_replay_phase(
                    conn,
                    debt,
                    GenerationPublicationPhase::DatabaseBackedUp,
                    Some(built),
                )?;
                return Ok(*built);
            }
            GenerationPublicationPhase::DatabaseBackedUp => {
                let built = required_publication_numbers(built.as_ref(), phase)?;
                crate::commands::composefs_ops::publish_generation_link(
                    db_path,
                    built.generation_number,
                )?;
                conary_core::db::backup::verify_generation_db_backup(
                    runtime_root.generation_path(built.generation_number),
                    Some(runtime_root.root()),
                )
                .map_err(|error| anyhow!("generation DB backup verification failed: {error}"))?;
                set_replay_phase(
                    conn,
                    debt,
                    GenerationPublicationPhase::DatabaseBackedUp,
                    Some(built),
                )?;
                return Ok(*built);
            }
        }
    }
}

fn write_generation_db_backup(
    conn: &Connection,
    db_path: &str,
    runtime_root: &ConaryRuntimeRoot,
    built: &BuiltForPublication,
    delta_recorder: Option<&mut GenerationDbDeltaRecorder<'_>>,
) -> Result<()> {
    let generation_dir = runtime_root.generation_path(built.generation_number);
    if let Some(recorder) = delta_recorder {
        match recorder.capture()? {
            GenerationDbDeltaCapture::Captured(delta) => {
                let prior =
                    conary_core::db::generation_backup_chain::find_generation_db_chain_by_baseline(
                        runtime_root.generations_dir(),
                        built.generation_number,
                        delta.source_baseline(),
                    )?;
                if let Some(prior_generation_dir) = prior {
                    conary_core::db::backup::create_generation_db_backup_from_delta(
                        conn,
                        db_path,
                        prior_generation_dir,
                        &generation_dir,
                        built.generation_number,
                        built.state_number,
                        &delta,
                    )?;
                    return Ok(());
                }
            }
            GenerationDbDeltaCapture::Fallback(reason) => {
                tracing::info!(
                    fallback_reason = reason.as_str(),
                    "Generation database delta capture requires a new full base"
                );
            }
        }
    }
    conary_core::db::backup::create_generation_db_backup(
        conn,
        db_path,
        generation_dir,
        built.generation_number,
        built.state_number,
    )?;
    Ok(())
}

fn set_replay_phase(
    conn: &Connection,
    debt: &GenerationPublication,
    phase: GenerationPublicationPhase,
    built: Option<&BuiltForPublication>,
) -> Result<()> {
    debt.set_phase(
        conn,
        phase,
        GenerationPublicationStatus::Running,
        built.map(|built| built.state_number),
        built.map(|built| built.generation_number),
    )
    .map_err(Into::into)
}

fn recorded_publication_numbers(
    debt: &GenerationPublication,
) -> Result<Option<BuiltForPublication>> {
    match (debt.state_number, debt.generation_number) {
        (None, None) => Ok(None),
        (Some(state_number), Some(generation_number)) if state_number == generation_number => {
            Ok(Some(BuiltForPublication {
                state_number,
                generation_number,
            }))
        }
        (state_number, generation_number) => Err(anyhow!(
            "publication debt has incomplete or mismatched state/generation identity: state={state_number:?} generation={generation_number:?}"
        )),
    }
}

fn required_publication_numbers(
    built: Option<&BuiltForPublication>,
    phase: GenerationPublicationPhase,
) -> Result<&BuiltForPublication> {
    built.ok_or_else(|| {
        anyhow!(
            "publication phase {} has no exact state/generation identity",
            phase.as_str()
        )
    })
}

#[derive(Debug, Clone, Copy)]
struct BuiltForPublication {
    state_number: i64,
    generation_number: i64,
}

#[cfg(test)]
#[path = "publication/tests.rs"]
mod tests;
