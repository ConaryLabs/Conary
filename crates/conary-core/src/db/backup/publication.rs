// crates/conary-core/src/db/backup/publication.rs

//! Validate the publication and changeset authority retained in a SQLite backup.

use super::{GenerationDbBackupManifest, open_immutable_sqlite_snapshot};
use crate::db::models::{ChangesetStatus, GenerationPublicationPhase, GenerationPublicationStatus};
use crate::{Error, Result};
use rusqlite::Connection;
use std::path::Path;

pub(super) fn verify_generation_publication_state(
    manifest: &GenerationDbBackupManifest,
    backup_path: &Path,
) -> Result<()> {
    verify_generation_publication_state_values(
        manifest.generation_number,
        manifest.state_number,
        backup_path,
    )
}

pub(super) fn verify_generation_publication_state_values(
    generation_number: i64,
    expected_state_number: i64,
    backup_path: &Path,
) -> Result<()> {
    let conn = open_immutable_sqlite_snapshot(backup_path)?;
    verify_generation_publication_state_connection(&conn, generation_number, expected_state_number)
}

pub(super) fn verify_generation_publication_state_connection(
    conn: &Connection,
    generation_number: i64,
    expected_state_number: i64,
) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT phase, status, recoverable, state_number
         FROM generation_publications
         WHERE generation_number = ?1
         ORDER BY id DESC",
    )?;
    let mut rows = stmt.query([generation_number])?;
    while let Some(row) = rows.next()? {
        let phase = GenerationPublicationPhase::try_from(row.get::<_, String>(0)?.as_str())
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
        let status = GenerationPublicationStatus::try_from(row.get::<_, String>(1)?.as_str())
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
        let recoverable: i64 = row.get(2)?;
        let state_number: Option<i64> = row.get(3)?;
        if state_number != Some(expected_state_number) {
            continue;
        }
        let complete = phase == GenerationPublicationPhase::DatabaseBackedUp
            && status == GenerationPublicationStatus::Complete
            && recoverable == 0;
        let backup_snapshot = phase == GenerationPublicationPhase::ActiveMarked
            && status == GenerationPublicationStatus::Running
            && recoverable == 1;
        if complete || backup_snapshot {
            return Ok(());
        }
    }

    Err(Error::RecoveryFailed(format!(
        "generation DB backup has no complete or active_marked/running publication state for generation {} state {}",
        generation_number, expected_state_number
    )))
}

pub(super) fn verify_transaction_high_water_mark(
    expected: Option<i64>,
    backup_path: &Path,
) -> Result<()> {
    let conn = open_immutable_sqlite_snapshot(backup_path)?;
    verify_transaction_high_water_mark_connection(&conn, expected)
}

pub(super) fn verify_transaction_high_water_mark_connection(
    conn: &Connection,
    expected: Option<i64>,
) -> Result<()> {
    let actual: Option<i64> = conn.query_row(
        "SELECT MAX(id) FROM changesets WHERE status = ?1",
        rusqlite::params![ChangesetStatus::Applied.as_str()],
        |row| row.get(0),
    )?;
    if actual != expected {
        return Err(Error::RecoveryFailed(format!(
            "generation DB backup transaction high-water mark changed: manifest={expected:?}, backup={actual:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_publication_values_cannot_validate_a_backup() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE generation_publications (
            id INTEGER, phase TEXT, status TEXT, recoverable INTEGER, state_number INTEGER,
            generation_number INTEGER)",
        )
        .unwrap();
        for (phase, status) in [
            ("obsolete", GenerationPublicationStatus::Complete.as_str()),
            (
                GenerationPublicationPhase::DatabaseBackedUp.as_str(),
                "obsolete",
            ),
        ] {
            conn.execute("DELETE FROM generation_publications", [])
                .unwrap();
            conn.execute(
                "INSERT INTO generation_publications VALUES (1, ?1, ?2, 0, 1, 1)",
                [phase, status],
            )
            .unwrap();
            let error = verify_generation_publication_state_connection(&conn, 1, 1).unwrap_err();
            let Error::Database(rusqlite::Error::FromSqlConversionFailure(_, _, source)) = error
            else {
                panic!("expected typed non-authority: {error:?}");
            };
            assert!(
                source
                    .downcast_ref::<crate::db::models::InvalidPersistedValue>()
                    .is_some()
            );
        }
    }
}
