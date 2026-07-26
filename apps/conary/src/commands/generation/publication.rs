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
    publish_pending_debt_with_hook(conn, db_path, summary, debt, |_| Ok(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationReplayCheckpoint {
    CurrentLinkSwappedBeforePhase,
    ActiveStateMarkedBeforeDatabaseBackup,
    CandidatesRemovedBeforeTerminalState,
}

fn publish_pending_debt_with_hook(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    debt: GenerationPublication,
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
        db_path,
        summary,
        &runtime_root,
        &debt,
        &config_transactions,
        &mut checkpoint,
    )
    .and_then(|built| {
        super::selected_root::remove_backed_up_publication_candidates(
            &runtime_root,
            &recoverable_before,
        )?;
        checkpoint(PublicationReplayCheckpoint::CandidatesRemovedBeforeTerminalState)?;
        let completed = debt.mark_complete_through(
            conn,
            high_water,
            built.state_number,
            built.generation_number,
        )?;
        Ok((built, completed))
    });

    match publish_result {
        Ok((built, completed)) => Ok(PublicationOutcome {
            generation_number: Some(built.generation_number),
            state_number: Some(built.state_number),
            needs_publication: false,
            retry_command: None,
            completed_debts: completed,
        }),
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

fn replay_publication(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
    config_transactions: &[GenerationConfigTransaction],
    checkpoint: &mut impl FnMut(PublicationReplayCheckpoint) -> Result<()>,
) -> Result<BuiltForPublication> {
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
                let captured =
                    super::selected_root::load_publication_candidate(runtime_root, debt)?;
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
                conary_core::db::backup::create_generation_db_backup(
                    db_path,
                    runtime_root.generation_path(built.generation_number),
                    built.generation_number,
                    built.state_number,
                )
                .map_err(|error| anyhow!("failed to write generation DB backup: {error}"))?;
                set_replay_phase(
                    conn,
                    debt,
                    GenerationPublicationPhase::DatabaseBackedUp,
                    Some(built),
                )?;
                phase = GenerationPublicationPhase::DatabaseBackedUp;
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
mod tests {
    use super::*;
    use conary_core::config_transaction::{
        ConfigArtifact, ConfigPathTransaction, ConfigTransactionOperation,
    };
    use conary_core::db::models::{ConfigFile, ConfigStatus, Trove};
    use conary_core::payload::ResolvedPayloadNode;
    use std::path::Path;

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
        let second = GenerationPublication::create_pending(
            &conn,
            Some(cs_b),
            None,
            "/tmp/conary.db",
            "/tmp/conary",
            "B",
            &Default::default(),
        )
        .unwrap();
        second
            .set_phase(
                &conn,
                GenerationPublicationPhase::DatabaseBackedUp,
                GenerationPublicationStatus::Running,
                Some(2),
                Some(2),
            )
            .unwrap();

        let completed = second
            .mark_complete_through(&conn, Some(cs_b), 2, 2)
            .unwrap();
        assert_eq!(completed, 2);
        assert!(
            GenerationPublication::pending_recoverable(&conn)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn recovery_replays_projection_and_backup_after_link_swap_before_phase() {
        let fixture = PublicationCrashFixture::new();
        let mut interrupted = false;
        let outcome = publish_pending_debt_with_hook(
            &fixture.conn,
            &fixture.db_path,
            "link-before-projection crash",
            fixture.debt.clone(),
            |checkpoint| {
                if checkpoint == PublicationReplayCheckpoint::CurrentLinkSwappedBeforePhase
                    && !interrupted
                {
                    interrupted = true;
                    return Err(anyhow!("simulated death after current link swap"));
                }
                Ok(())
            },
        )
        .unwrap();

        assert!(outcome.needs_publication);
        let interrupted_debt = fixture.reload_debt();
        assert_eq!(
            interrupted_debt.phase,
            GenerationPublicationPhase::ArtifactReady
        );
        assert_eq!(
            fixture.current_generation(),
            interrupted_debt.generation_number.unwrap()
        );
        assert_eq!(fixture.config_status(), ConfigStatus::Modified);
        assert!(!fixture.generation_backup_exists(&interrupted_debt));

        let recovered = retry_pending_publication(
            &fixture.conn,
            &fixture.db_path,
            "recover link-before-projection crash",
        )
        .unwrap();

        assert!(
            !recovered.needs_publication,
            "recovery debt remained pending: {:?}",
            fixture.reload_debt()
        );
        fixture.assert_terminal_publication();
    }

    #[test]
    fn recovery_replays_backup_after_link_swap_and_active_state_mark() {
        let fixture = PublicationCrashFixture::new();
        let mut interrupted = false;
        let outcome = publish_pending_debt_with_hook(
            &fixture.conn,
            &fixture.db_path,
            "active-before-backup crash",
            fixture.debt.clone(),
            |checkpoint| {
                if checkpoint == PublicationReplayCheckpoint::ActiveStateMarkedBeforeDatabaseBackup
                    && !interrupted
                {
                    interrupted = true;
                    return Err(anyhow!("simulated death before generation DB backup"));
                }
                Ok(())
            },
        )
        .unwrap();

        assert!(outcome.needs_publication);
        let interrupted_debt = fixture.reload_debt();
        assert_eq!(
            interrupted_debt.phase,
            GenerationPublicationPhase::ActiveMarked
        );
        assert_eq!(
            fixture.current_generation(),
            interrupted_debt.generation_number.unwrap()
        );
        assert_eq!(fixture.config_status(), ConfigStatus::Pristine);
        assert!(!fixture.generation_backup_exists(&interrupted_debt));

        let recovered = retry_pending_publication(
            &fixture.conn,
            &fixture.db_path,
            "recover active-before-backup crash",
        )
        .unwrap();

        assert!(
            !recovered.needs_publication,
            "recovery debt remained pending: {:?}",
            fixture.reload_debt()
        );
        fixture.assert_terminal_publication();
    }

    #[test]
    fn recovery_completes_after_candidates_are_removed_before_terminal_state() {
        let fixture = PublicationCrashFixture::new();
        let mut interrupted = false;
        let outcome = publish_pending_debt_with_hook(
            &fixture.conn,
            &fixture.db_path,
            "candidate-cleanup crash",
            fixture.debt.clone(),
            |checkpoint| {
                if checkpoint == PublicationReplayCheckpoint::CandidatesRemovedBeforeTerminalState
                    && !interrupted
                {
                    interrupted = true;
                    return Err(anyhow!(
                        "simulated death after candidate cleanup before terminal state"
                    ));
                }
                Ok(())
            },
        )
        .unwrap();

        assert!(outcome.needs_publication);
        let interrupted_debt = fixture.reload_debt();
        assert_eq!(
            interrupted_debt.phase,
            GenerationPublicationPhase::DatabaseBackedUp
        );
        assert_eq!(interrupted_debt.status, GenerationPublicationStatus::Failed);
        assert!(interrupted_debt.recoverable);
        assert!(!fixture.candidate_exists());
        assert!(fixture.generation_backup_exists(&interrupted_debt));

        let recovered = retry_pending_publication(
            &fixture.conn,
            &fixture.db_path,
            "recover candidate-cleanup crash",
        )
        .unwrap();

        assert!(
            !recovered.needs_publication,
            "recovery debt remained pending: {:?}",
            fixture.reload_debt()
        );
        fixture.assert_terminal_publication();
    }

    struct PublicationCrashFixture {
        _temp: tempfile::TempDir,
        _mount_guard: crate::commands::composefs_ops::TestMountSkipGuard,
        db_path: String,
        conn: Connection,
        debt: GenerationPublication,
        runtime_root: ConaryRuntimeRoot,
    }

    impl PublicationCrashFixture {
        fn new() -> Self {
            let mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
            let (temp, db_path) = crate::commands::test_helpers::setup_command_test_db();
            crate::commands::test_helpers::create_active_test_generation(Path::new(&db_path), 1);
            let conn = conary_core::db::open(&db_path).unwrap();
            let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);

            let artifact = config_artifact(&runtime_root, b"projected configuration");
            let nginx = Trove::find_one_by_name(&conn, "nginx").unwrap().unwrap();
            let mut config = ConfigFile::new(
                "/etc/nginx/nginx.conf".to_string(),
                nginx.id.unwrap(),
                artifact.sha256().to_string(),
            );
            config.status = ConfigStatus::Modified;
            config.current_hash = Some(conary_core::hash::sha256(b"local configuration"));
            config.insert(&conn).unwrap();

            let config_transaction = GenerationConfigTransaction {
                entries: vec![ConfigPathTransaction {
                    path: config.path.clone(),
                    operation: ConfigTransactionOperation::Restore,
                    before: None,
                    current: Some(artifact),
                    after: None,
                    auxiliaries: Vec::new(),
                }],
                ..Default::default()
            };
            let mut session = super::super::selected_root::SelectedRootSession::begin(
                &conn,
                &db_path,
                "publication crash fixture",
            )
            .unwrap();
            let debt = GenerationPublication::create_pending(
                &conn,
                None,
                None,
                &db_path,
                &runtime_root.root().display().to_string(),
                "publication crash fixture",
                &config_transaction,
            )
            .unwrap();
            session
                .persist_for_publication(&runtime_root, &debt)
                .unwrap();
            drop(session);

            Self {
                _temp: temp,
                _mount_guard: mount_guard,
                db_path,
                conn,
                debt,
                runtime_root,
            }
        }

        fn reload_debt(&self) -> GenerationPublication {
            GenerationPublication::find_by_id(&self.conn, self.debt.id.unwrap())
                .unwrap()
                .unwrap()
        }

        fn current_generation(&self) -> i64 {
            conary_core::generation::mount::current_generation(self.runtime_root.root())
                .unwrap()
                .unwrap()
        }

        fn config_status(&self) -> ConfigStatus {
            ConfigFile::find_by_path(&self.conn, "/etc/nginx/nginx.conf")
                .unwrap()
                .unwrap()
                .status
        }

        fn generation_backup_exists(&self, debt: &GenerationPublication) -> bool {
            conary_core::db::backup::verify_generation_db_backup(
                self.runtime_root
                    .generation_path(debt.generation_number.unwrap()),
                Some(self.runtime_root.root()),
            )
            .is_ok()
        }

        fn candidate_exists(&self) -> bool {
            super::super::selected_root::publication_candidate_dir(&self.runtime_root, &self.debt)
                .unwrap()
                .exists()
        }

        fn assert_terminal_publication(&self) {
            let debt = self.reload_debt();
            assert_eq!(debt.phase, GenerationPublicationPhase::DatabaseBackedUp);
            assert_eq!(debt.status, GenerationPublicationStatus::Complete);
            assert!(!debt.recoverable);
            assert_eq!(self.config_status(), ConfigStatus::Pristine);
            assert!(self.generation_backup_exists(&debt));
            assert!(!self.candidate_exists());
            assert!(
                GenerationPublication::pending_recoverable(&self.conn)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    fn config_artifact(runtime_root: &ConaryRuntimeRoot, content: &[u8]) -> ConfigArtifact {
        let node = ResolvedPayloadNode::from_numeric_source(
            crate::commands::test_helpers::test_regular_payload_node(0o644),
        )
        .unwrap();
        let cas = conary_core::filesystem::CasStore::new(runtime_root.objects_dir()).unwrap();
        let sha256 = cas.store(content).unwrap();
        ConfigArtifact::regular(
            conary_core::payload::PayloadContentAuthority {
                sha256,
                size: content.len() as u64,
            },
            node,
        )
        .unwrap()
    }
}
