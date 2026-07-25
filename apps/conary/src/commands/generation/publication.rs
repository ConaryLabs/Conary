// apps/conary/src/commands/generation/publication.rs

use anyhow::{Result, anyhow};
use conary_core::config_transaction::GenerationConfigTransaction;
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
    eprintln!(
        "WARNING: package mutation committed, but generation publication is pending for changeset {changeset_id}.\nRun: {retry}"
    );
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
) -> Result<PublicationOutcome> {
    publish_pending_debt(conn, db_path, summary, debt)
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
    publish_pending_debt(conn, db_path, summary, debt)
}

fn publish_pending_debt(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    debt: GenerationPublication,
) -> Result<PublicationOutcome> {
    let runtime_root = ConaryRuntimeRoot::from_db_path(db_path);
    let high_water = GenerationPublication::applied_high_water_changeset_id(conn)?;
    let recoverable_before = GenerationPublication::pending_recoverable(conn)?;
    let config_transactions = recoverable_before
        .iter()
        .map(|publication| publication.config_transaction.clone())
        .collect::<Vec<_>>();

    let publish_result = (|| -> Result<BuiltForPublication> {
        debt.set_phase(
            conn,
            GenerationPublicationPhase::Building,
            GenerationPublicationStatus::Running,
            None,
            None,
        )?;
        let captured = super::selected_root::load_publication_candidate(&runtime_root, &debt)?;
        let built = crate::commands::composefs_ops::build_captured_generation_for_publication(
            conn,
            db_path,
            summary,
            &config_transactions,
            captured,
        )?;
        if built.state_number != built.generation_number {
            return Err(anyhow!(
                "generation builder returned mismatched state/generation numbers: state={} generation={}",
                built.state_number,
                built.generation_number
            ));
        }
        debt.set_phase(
            conn,
            GenerationPublicationPhase::ArtifactReady,
            GenerationPublicationStatus::Running,
            Some(built.state_number),
            Some(built.generation_number),
        )?;
        crate::commands::composefs_ops::publish_generation_link(db_path, built.generation_number)?;
        debt.set_phase(
            conn,
            GenerationPublicationPhase::CurrentPublished,
            GenerationPublicationStatus::Running,
            Some(built.state_number),
            Some(built.generation_number),
        )?;
        crate::commands::generation::config_transaction::project_persisted_statuses(
            conn,
            &config_transactions,
        )?;
        crate::commands::composefs_ops::mark_generation_state_active(
            conn,
            built.generation_number,
        )?;
        conary_core::db::backup::create_generation_db_backup(
            db_path,
            runtime_root.generation_path(built.generation_number),
            built.generation_number,
            built.state_number,
        )
        .map_err(|error| anyhow!("failed to write generation DB backup: {error}"))?;
        Ok(BuiltForPublication {
            state_number: built.state_number,
            generation_number: built.generation_number,
        })
    })();

    match publish_result {
        Ok(built) => {
            let completed = GenerationPublication::mark_complete_through(
                conn,
                high_water,
                built.state_number,
                built.generation_number,
            )?;
            super::selected_root::remove_terminal_publication_candidates(
                conn,
                &runtime_root,
                &recoverable_before,
            )?;
            Ok(PublicationOutcome {
                generation_number: Some(built.generation_number),
                state_number: Some(built.state_number),
                needs_publication: false,
                retry_command: None,
                completed_debts: completed,
            })
        }
        Err(error) => {
            debt.mark_failed(conn, &error.to_string())?;
            Ok(PublicationOutcome {
                generation_number: None,
                state_number: None,
                needs_publication: true,
                retry_command: Some(PublicationOutcome::default_retry_command()),
                completed_debts: 0,
            })
        }
    }
}

#[derive(Debug)]
struct BuiltForPublication {
    state_number: i64,
    generation_number: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_command_uses_parameterless_publish() {
        assert_eq!(
            PublicationOutcome::default_retry_command(),
            "conary system generation publish --yes"
        );
    }

    #[test]
    fn successful_publication_completion_sweeps_prior_debts() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        conary_core::db::init(temp.path()).unwrap();
        let conn = conary_core::db::open(temp.path()).unwrap();
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES ('A', 'applied')",
            [],
        )
        .unwrap();
        let cs_a = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES ('B', 'applied')",
            [],
        )
        .unwrap();
        let cs_b = conn.last_insert_rowid();

        let first = GenerationPublication::create_pending(
            &conn,
            Some(cs_a),
            None,
            "/tmp/conary.db",
            "/tmp/conary",
            "A",
            &Default::default(),
        )
        .unwrap();
        first.mark_failed(&conn, "forced").unwrap();
        GenerationPublication::create_pending(
            &conn,
            Some(cs_b),
            None,
            "/tmp/conary.db",
            "/tmp/conary",
            "B",
            &Default::default(),
        )
        .unwrap();

        let completed =
            GenerationPublication::mark_complete_through(&conn, Some(cs_b), 2, 2).unwrap();
        assert_eq!(completed, 2);
        assert!(
            GenerationPublication::pending_recoverable(&conn)
                .unwrap()
                .is_empty()
        );
    }
}
