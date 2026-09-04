// apps/conary/src/commands/generation/publication/tests.rs

use super::*;
#[cfg(feature = "test-hooks")]
use conary_core::config_transaction::{
    ConfigArtifact, ConfigPathTransaction, ConfigTransactionOperation,
};
#[cfg(feature = "test-hooks")]
use conary_core::db::models::{ConfigFile, ConfigStatus, Trove};
#[cfg(feature = "test-hooks")]
use conary_core::payload::ResolvedPayloadNode;
#[cfg(feature = "test-hooks")]
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
#[cfg(feature = "test-hooks")]
fn recovery_replays_projection_and_backup_after_link_swap_before_phase() {
    let fixture = PublicationCrashFixture::new();
    let mut interrupted = false;
    let outcome = publish_pending_debt_with_hook(
        &fixture.conn,
        &fixture.db_path,
        "link-before-projection crash",
        fixture.debt.clone(),
        None,
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
        outcome.failure_reason.as_deref(),
        interrupted_debt.last_error.as_deref()
    );
    assert_eq!(
        outcome.failure_reason.as_deref(),
        Some("simulated death after current link swap")
    );
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
    assert_eq!(recovered.failure_reason, None);
    fixture.assert_terminal_publication();
}

#[test]
#[cfg(feature = "test-hooks")]
fn recovery_replays_backup_after_link_swap_and_active_state_mark() {
    let fixture = PublicationCrashFixture::new();
    let mut interrupted = false;
    let outcome = publish_pending_debt_with_hook(
        &fixture.conn,
        &fixture.db_path,
        "active-before-backup crash",
        fixture.debt.clone(),
        None,
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
#[cfg(feature = "test-hooks")]
fn recovery_completes_after_database_backup_before_terminal_state() {
    let fixture = PublicationCrashFixture::new();
    let mut interrupted = false;
    let outcome = publish_pending_debt_with_hook(
        &fixture.conn,
        &fixture.db_path,
        "database-backup crash",
        fixture.debt.clone(),
        None,
        |checkpoint| {
            if checkpoint == PublicationReplayCheckpoint::DatabaseBackedUpBeforeTerminalState
                && !interrupted
            {
                interrupted = true;
                return Err(anyhow!(
                    "simulated death after database backup before terminal state"
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
    assert!(fixture.snapshot_exists());
    assert!(fixture.generation_backup_exists(&interrupted_debt));

    let recovered = retry_pending_publication(
        &fixture.conn,
        &fixture.db_path,
        "recover database-backup crash",
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
#[cfg(feature = "test-hooks")]
fn normal_publication_finalizes_one_self_contained_terminal_delta() {
    let fixture = PublicationCrashFixture::new();
    fixture
        .conn
        .execute(
            "INSERT INTO generation_publications (
                     db_path, runtime_root, phase, status, state_number,
                     generation_number, summary, recoverable, completed_at
                 ) VALUES (?1, ?2, 'database_backed_up', 'complete', 1, 1,
                           'prior generation', 0, CURRENT_TIMESTAMP)",
            rusqlite::params![
                &fixture.db_path,
                fixture.runtime_root.root().display().to_string()
            ],
        )
        .unwrap();
    conary_core::db::backup::create_generation_db_backup(
        &fixture.conn,
        &fixture.db_path,
        fixture.runtime_root.generation_path(1),
        1,
        1,
    )
    .unwrap();
    let mut recorder = GenerationDbDeltaRecorder::begin(&fixture.conn, &fixture.db_path).unwrap();

    let outcome = publish_pending_debt_with_hook(
        &fixture.conn,
        &fixture.db_path,
        "delta publication",
        fixture.debt.clone(),
        Some(&mut recorder),
        |_| Ok(()),
    )
    .unwrap();

    assert!(!outcome.needs_publication, "{outcome:?}");
    let generation = outcome.generation_number.unwrap();
    let manifest = conary_core::db::generation_backup_chain::read_generation_db_chain(
        fixture
            .runtime_root
            .generation_path(generation)
            .join("state"),
    )
    .unwrap();
    assert_eq!(manifest.deltas.len(), 1);
    assert_eq!(
        manifest.final_baseline,
        conary_core::db::generation_delta::read_baseline_token(&fixture.conn).unwrap()
    );
    let state_dir = fixture
        .runtime_root
        .generation_path(generation)
        .join("state");
    let delta_artifact_count = std::fs::read_dir(state_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with("conary.db.delta-"))
        .count();
    assert_eq!(delta_artifact_count, 1);
    assert!(fixture.generation_backup_exists(&fixture.reload_debt()));
}

#[cfg(feature = "test-hooks")]
struct PublicationCrashFixture {
    _temp: tempfile::TempDir,
    _mount_guard: crate::commands::composefs_ops::TestMountSkipGuard,
    db_path: String,
    conn: Connection,
    debt: GenerationPublication,
    runtime_root: ConaryRuntimeRoot,
}

#[cfg(feature = "test-hooks")]
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
            .persist_for_publication(&conn, &runtime_root, &debt)
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

    fn snapshot_exists(&self) -> bool {
        let debt = self.reload_debt();
        debt.selected_root_snapshot_id
            .and_then(|id| {
                conary_core::generation::root_manifest::SelectedRootSnapshot::find(&self.conn, id)
                    .unwrap()
            })
            .is_some()
    }

    fn assert_terminal_publication(&self) {
        let debt = self.reload_debt();
        assert_eq!(debt.phase, GenerationPublicationPhase::DatabaseBackedUp);
        assert_eq!(debt.status, GenerationPublicationStatus::Complete);
        assert!(!debt.recoverable);
        assert_eq!(self.config_status(), ConfigStatus::Pristine);
        assert!(self.generation_backup_exists(&debt));
        assert!(self.snapshot_exists());
        assert!(
            GenerationPublication::pending_recoverable(&self.conn)
                .unwrap()
                .is_empty()
        );
    }
}

#[cfg(feature = "test-hooks")]
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
