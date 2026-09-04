// conary-core/src/db/models/generation_publication/tests.rs

use super::*;
use crate::payload::{PayloadNode, ResolvedPayloadNode};
use std::str::FromStr;

fn exact_config_transaction() -> GenerationConfigTransaction {
    let node = ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o640)).unwrap();
    let artifact = crate::config_transaction::ConfigArtifact::regular(
        crate::payload::PayloadContentAuthority {
            sha256: crate::hash::sha256(b"exact"),
            size: 5,
        },
        node,
    )
    .unwrap();
    GenerationConfigTransaction {
        entries: vec![crate::config_transaction::ConfigPathTransaction {
            path: "/etc/exact.conf".to_string(),
            operation: crate::config_transaction::ConfigTransactionOperation::Install,
            before: None,
            current: None,
            after: Some(crate::config_transaction::ConfigPackageState {
                source: crate::db::models::ConfigSource::Auto,
                noreplace: false,
                ghost: false,
                materialized: true,
                original_sha256: Some(artifact.sha256().to_string()),
                artifact: Some(artifact),
            }),
            auxiliaries: Vec::new(),
        }],
        ..Default::default()
    }
}

#[test]
fn phase_and_status_reject_unknown_values() {
    assert_eq!(
        GenerationPublicationPhase::from_str("artifact_ready").unwrap(),
        GenerationPublicationPhase::ArtifactReady
    );
    assert_eq!(
        GenerationPublicationPhase::from_str("configuration_projected").unwrap(),
        GenerationPublicationPhase::ConfigurationProjected
    );
    assert_eq!(
        GenerationPublicationPhase::from_str("database_backed_up").unwrap(),
        GenerationPublicationPhase::DatabaseBackedUp
    );
    assert!(GenerationPublicationPhase::from_str("current_renamed").is_err());
    assert_eq!(
        GenerationPublicationStatus::from_str("failed").unwrap(),
        GenerationPublicationStatus::Failed
    );
    assert!(GenerationPublicationStatus::from_str("mystery").is_err());
}

#[test]
fn create_pending_and_mark_complete_sweeps_covered_debts() {
    let (_tmp, conn) = crate::db::testing::create_test_db();

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

    let a = GenerationPublication::create_pending(
        &conn,
        Some(cs_a),
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "A",
        &Default::default(),
    )
    .unwrap();
    let b = GenerationPublication::create_pending(
        &conn,
        Some(cs_b),
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "B",
        &Default::default(),
    )
    .unwrap();
    a.mark_failed(&conn, "forced").unwrap();
    b.set_phase(
        &conn,
        GenerationPublicationPhase::DatabaseBackedUp,
        GenerationPublicationStatus::Running,
        Some(7),
        Some(7),
    )
    .unwrap();

    let completed = b.mark_complete_through(&conn, Some(cs_b), 7, 7).unwrap();
    assert_eq!(completed, 2);
    assert!(
        GenerationPublication::pending_recoverable(&conn)
            .unwrap()
            .is_empty()
    );
    let completed_b = GenerationPublication::find_by_id(&conn, b.id.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        completed_b.phase,
        GenerationPublicationPhase::DatabaseBackedUp
    );
}

#[test]
fn publication_cannot_become_terminal_before_database_backup_phase() {
    let (_tmp, conn) = crate::db::testing::create_test_db();
    let debt = GenerationPublication::create_pending(
        &conn,
        None,
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "crash boundary",
        &Default::default(),
    )
    .unwrap();

    for phase in [
        GenerationPublicationPhase::ArtifactReady,
        GenerationPublicationPhase::CurrentPublished,
        GenerationPublicationPhase::ConfigurationProjected,
        GenerationPublicationPhase::ActiveMarked,
    ] {
        debt.set_phase(
            &conn,
            phase,
            GenerationPublicationStatus::Running,
            Some(7),
            Some(7),
        )
        .unwrap();
        let error = debt
            .mark_complete_through(&conn, None, 7, 7)
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot become terminal"), "{error}");
    }
    assert_eq!(
        GenerationPublication::pending_recoverable(&conn)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn pending_for_changeset_finds_recoverable_debt_only() {
    let (_tmp, conn) = crate::db::testing::create_test_db();
    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('A', 'applied')",
        [],
    )
    .unwrap();
    let cs_a = conn.last_insert_rowid();

    let debt = GenerationPublication::create_pending(
        &conn,
        Some(cs_a),
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "A",
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        GenerationPublication::pending_for_changeset(&conn, cs_a)
            .unwrap()
            .unwrap()
            .id,
        debt.id
    );

    debt.set_phase(
        &conn,
        GenerationPublicationPhase::DatabaseBackedUp,
        GenerationPublicationStatus::Running,
        Some(1),
        Some(1),
    )
    .unwrap();
    debt.mark_complete_through(&conn, Some(cs_a), 1, 1).unwrap();
    assert!(
        GenerationPublication::pending_for_changeset(&conn, cs_a)
            .unwrap()
            .is_none()
    );
}

#[test]
fn abandoning_changeset_debt_excludes_forward_transaction_from_retry() {
    let (_tmp, conn) = crate::db::testing::create_test_db();
    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('upgrade', 'applied')",
        [],
    )
    .unwrap();
    let changeset_id = conn.last_insert_rowid();
    GenerationPublication::create_pending(
        &conn,
        Some(changeset_id),
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "upgrade",
        &exact_config_transaction(),
    )
    .unwrap();

    assert_eq!(
        GenerationPublication::abandon_recoverable_for_changeset(&conn, changeset_id).unwrap(),
        1
    );
    assert!(
        GenerationPublication::pending_recoverable(&conn)
            .unwrap()
            .is_empty()
    );
    let status: String = conn
        .query_row(
            "SELECT status FROM generation_publications
                 WHERE trigger_changeset_id = ?1",
            [changeset_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "abandoned");
}

#[test]
fn publication_snapshot_obeys_the_callers_sql_transaction() {
    let (_tmp, conn) = crate::db::testing::create_test_db();
    let snapshot = exact_config_transaction();

    {
        let tx = conn.unchecked_transaction().unwrap();
        GenerationPublication::create_pending(
            &tx,
            None,
            None,
            "/tmp/conary.db",
            "/tmp/conary",
            "rolled back",
            &snapshot,
        )
        .unwrap();
    }
    assert!(
        GenerationPublication::pending_recoverable(&conn)
            .unwrap()
            .is_empty()
    );

    let tx = conn.unchecked_transaction().unwrap();
    GenerationPublication::create_pending(
        &tx,
        None,
        None,
        "/tmp/conary.db",
        "/tmp/conary",
        "committed",
        &snapshot,
    )
    .unwrap();
    tx.commit().unwrap();

    let debts = GenerationPublication::pending_recoverable(&conn).unwrap();
    assert_eq!(debts.len(), 1);
    assert_eq!(debts[0].config_transaction, snapshot);
}

#[test]
fn applied_high_water_ignores_pending_and_rolled_back_changesets() {
    let (_tmp, conn) = crate::db::testing::create_test_db();
    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('A', 'applied')",
        [],
    )
    .unwrap();
    let applied = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('B', 'pending')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO changesets (description, status) VALUES ('C', 'rolled_back')",
        [],
    )
    .unwrap();

    assert_eq!(
        GenerationPublication::applied_high_water_changeset_id(&conn).unwrap(),
        Some(applied)
    );
}
