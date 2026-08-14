// conary-core/src/db/generation_delta.rs

//! Typed SQLite session changesets for generation recovery deltas.

use crate::{Error, Result};
use rusqlite::Connection;
use rusqlite::session::Session;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Why a transaction-local changeset cannot be complete generation authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenerationDbDeltaFallbackReason {
    ConcurrentConnectionWrite,
}

impl GenerationDbDeltaFallbackReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConcurrentConnectionWrite => "concurrent-connection-write",
        }
    }
}

/// Exact bytes and identity emitted by SQLite's session extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationDbDelta {
    bytes: Vec<u8>,
    sha256: String,
}

impl GenerationDbDelta {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let sha256 = crate::hash::sha256(&bytes);
        Self { bytes, sha256 }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn payload_bytes(&self) -> u64 {
        self.bytes.len().try_into().unwrap_or(u64::MAX)
    }
}

/// Result of trying to turn one connection's writes into a recovery delta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenerationDbDeltaCapture {
    Captured(GenerationDbDelta),
    Fallback(GenerationDbDeltaFallbackReason),
}

/// Records every session-compatible table mutation made through one connection.
///
/// SQLite increments `PRAGMA data_version` on this connection when another
/// connection commits. A changed value therefore rejects the candidate. The
/// eventual publication owner must additionally prove the candidate's base
/// identity and hold write serialization through durable publication.
pub struct GenerationDbDeltaRecorder<'connection> {
    session: Session<'connection>,
    source: &'connection Connection,
    source_path: PathBuf,
    starting_data_version: i64,
}

impl<'connection> GenerationDbDeltaRecorder<'connection> {
    pub fn begin(source: &'connection Connection, source_path: impl AsRef<Path>) -> Result<Self> {
        let source_path = source_path.as_ref();
        validate_source_connection_path(source, source_path)?;
        let starting_data_version = data_version(source)?;
        let mut session = Session::new(source)?;
        session.attach::<&str>(None)?;
        Ok(Self {
            session,
            source,
            source_path: source_path.to_path_buf(),
            starting_data_version,
        })
    }

    pub fn finish(mut self) -> Result<GenerationDbDeltaCapture> {
        let mut bytes = Vec::new();
        self.session.changeset_strm(&mut bytes)?;
        let ending_data_version = data_version(self.source)?;
        if ending_data_version != self.starting_data_version {
            return Ok(GenerationDbDeltaCapture::Fallback(
                GenerationDbDeltaFallbackReason::ConcurrentConnectionWrite,
            ));
        }

        Ok(GenerationDbDeltaCapture::Captured(
            GenerationDbDelta::from_bytes(bytes),
        ))
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
}

fn data_version(source: &Connection) -> Result<i64> {
    source
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .map_err(Into::into)
}

fn validate_source_connection_path(source: &Connection, source_path: &Path) -> Result<()> {
    let connected_path: String = source.query_row(
        "SELECT file FROM pragma_database_list WHERE name = 'main'",
        [],
        |row| row.get(0),
    )?;
    let connected = Path::new(&connected_path).canonicalize()?;
    let requested = source_path.canonicalize()?;
    if connected != requested {
        return Err(Error::RecoveryFailed(format!(
            "generation DB delta source mismatch: connection={} requested={}",
            connected.display(),
            requested.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::session::ConflictAction;

    fn create_fixture(path: &Path) -> Connection {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE authority (
                     id INTEGER PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 INSERT INTO authority (id, value) VALUES (1, 'base');",
            )
            .unwrap();
        connection
    }

    #[test]
    fn session_delta_replays_exact_insert_update_and_delete() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let base_path = temp.path().join("base.sqlite");
        let source = create_fixture(&source_path);
        let base = create_fixture(&base_path);
        let recorder = GenerationDbDeltaRecorder::begin(&source, &source_path).unwrap();

        source
            .execute_batch(
                "UPDATE authority SET value = 'updated' WHERE id = 1;
                 INSERT INTO authority (id, value) VALUES (2, 'inserted');
                 INSERT INTO authority (id, value) VALUES (3, 'deleted');
                 DELETE FROM authority WHERE id = 3;",
            )
            .unwrap();

        let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
            panic!("same-connection mutations unexpectedly required a full snapshot");
        };
        assert!(!delta.bytes().is_empty());
        assert_eq!(delta.sha256(), crate::hash::sha256(delta.bytes()));

        let mut input = delta.bytes();
        base.apply_strm(&mut input, None::<fn(&str) -> bool>, |_conflict, _item| {
            ConflictAction::SQLITE_CHANGESET_ABORT
        })
        .unwrap();
        let rows = base
            .prepare("SELECT id, value FROM authority ORDER BY id")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(1, "updated".to_string()), (2, "inserted".to_string())]
        );
    }

    #[test]
    fn another_connection_commit_forces_typed_full_snapshot_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let source = create_fixture(&source_path);
        let writer = Connection::open(&source_path).unwrap();
        let recorder = GenerationDbDeltaRecorder::begin(&source, &source_path).unwrap();

        writer
            .execute(
                "INSERT INTO authority (id, value) VALUES (2, 'other connection')",
                [],
            )
            .unwrap();

        assert_eq!(
            recorder.finish().unwrap(),
            GenerationDbDeltaCapture::Fallback(
                GenerationDbDeltaFallbackReason::ConcurrentConnectionWrite
            )
        );
    }

    #[test]
    fn source_connection_must_name_the_exact_database_path() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let other_path = temp.path().join("other.sqlite");
        let source = create_fixture(&source_path);
        create_fixture(&other_path);

        let error = match GenerationDbDeltaRecorder::begin(&source, &other_path) {
            Ok(_) => panic!("mismatched source path unexpectedly admitted"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("delta source mismatch"));
    }

    #[test]
    fn every_current_schema_table_is_session_compatible() {
        let (_database, connection) = crate::db::testing::create_test_db();
        let mut statement = connection
            .prepare(
                "SELECT m.name
                 FROM sqlite_schema AS m
                 WHERE m.type = 'table'
                   AND m.name NOT LIKE 'sqlite_%'
                   AND NOT EXISTS (
                       SELECT 1 FROM pragma_table_info(m.name) WHERE pk > 0
                   )
                 ORDER BY m.name",
            )
            .unwrap();
        let tables = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            tables.is_empty(),
            "SQLite session changesets require a declared primary key: {tables:?}"
        );
    }
}
