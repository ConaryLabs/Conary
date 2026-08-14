// conary-core/src/db/generation_delta.rs

//! Typed SQLite session changesets for generation recovery deltas.

use crate::{Error, Result};
use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use rusqlite::session::Session;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MUTATION_TOKEN_FUNCTION: &str = "conary_generation_mutation_token";
const MUTATION_EPOCH_TABLE: &str = "generation_db_mutation_epoch";

/// Why a transaction-local changeset cannot be complete generation authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenerationDbDeltaFallbackReason {
    ConcurrentConnectionWrite,
    SourceBaselineChanged,
}

impl GenerationDbDeltaFallbackReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConcurrentConnectionWrite => "concurrent-connection-write",
            Self::SourceBaselineChanged => "source-baseline-changed",
        }
    }
}

/// Cheap persisted SQLite identity used only to admit an incremental delta.
///
/// The cryptographic recovery identity remains the base digest plus ordered
/// changeset digests. This token proves that the checkpointed source file has
/// not moved away from the exact prior generation baseline before recording
/// begins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationDbBaselineToken {
    pub mutation_token: String,
}

/// Exact bytes and identity emitted by SQLite's session extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationDbDelta {
    bytes: Vec<u8>,
    sha256: String,
    source_baseline: GenerationDbBaselineToken,
    result_baseline: GenerationDbBaselineToken,
}

impl GenerationDbDelta {
    pub fn from_bytes(
        bytes: Vec<u8>,
        source_baseline: GenerationDbBaselineToken,
        result_baseline: GenerationDbBaselineToken,
    ) -> Self {
        let sha256 = crate::hash::sha256(&bytes);
        Self {
            bytes,
            sha256,
            source_baseline,
            result_baseline,
        }
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

    pub fn result_baseline(&self) -> &GenerationDbBaselineToken {
        &self.result_baseline
    }

    pub fn source_baseline(&self) -> &GenerationDbBaselineToken {
        &self.source_baseline
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
    source_baseline: GenerationDbBaselineToken,
    starting_data_version: i64,
    baseline_fallback: Option<GenerationDbDeltaFallbackReason>,
}

impl<'connection> GenerationDbDeltaRecorder<'connection> {
    pub fn begin(source: &'connection Connection, source_path: impl AsRef<Path>) -> Result<Self> {
        let source_path = source_path.as_ref();
        validate_source_connection_path(source, source_path)?;
        let source_baseline = read_baseline_token(source)?;
        let starting_data_version = data_version(source)?;
        let mut session = Session::new(source)?;
        session.table_filter(Some(|table: &str| table != MUTATION_EPOCH_TABLE));
        session.attach::<&str>(None)?;
        Ok(Self {
            session,
            source,
            source_path: source_path.to_path_buf(),
            source_baseline,
            starting_data_version,
            baseline_fallback: None,
        })
    }

    pub fn begin_against(
        source: &'connection Connection,
        source_path: impl AsRef<Path>,
        expected: GenerationDbBaselineToken,
    ) -> Result<Self> {
        let source_path = source_path.as_ref();
        let observed = read_baseline_token(source)?;
        let mut recorder = Self::begin(source, source_path)?;
        recorder.baseline_fallback = if observed == expected {
            None
        } else {
            Some(GenerationDbDeltaFallbackReason::SourceBaselineChanged)
        };
        Ok(recorder)
    }

    /// Capture the cumulative changes observed since this recorder began.
    ///
    /// Calling this more than once does not reset the session. Publication can
    /// therefore persist a recoverable pre-terminal checkpoint and later
    /// replace its manifest with the exact terminal delta without reopening a
    /// gap in crash recovery.
    pub fn capture(&mut self) -> Result<GenerationDbDeltaCapture> {
        let mut bytes = Vec::new();
        self.session.changeset_strm(&mut bytes)?;
        let ending_data_version = data_version(self.source)?;
        if let Some(reason) = self.baseline_fallback {
            return Ok(GenerationDbDeltaCapture::Fallback(reason));
        }
        if ending_data_version != self.starting_data_version {
            return Ok(GenerationDbDeltaCapture::Fallback(
                GenerationDbDeltaFallbackReason::ConcurrentConnectionWrite,
            ));
        }

        let result_baseline = read_baseline_token(self.source)?;

        Ok(GenerationDbDeltaCapture::Captured(
            GenerationDbDelta::from_bytes(bytes, self.source_baseline.clone(), result_baseline),
        ))
    }

    pub fn finish(mut self) -> Result<GenerationDbDeltaCapture> {
        self.capture()
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
}

pub fn read_baseline_token(source: &Connection) -> Result<GenerationDbBaselineToken> {
    let mutation_token = source.query_row(
        "SELECT token FROM generation_db_mutation_epoch WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(GenerationDbBaselineToken { mutation_token })
}

pub(crate) fn configure_mutation_epoch(source: &Connection) -> Result<()> {
    let transaction_token = Arc::new(Mutex::new(None::<String>));
    let function_token = Arc::clone(&transaction_token);
    source.create_scalar_function(
        MUTATION_TOKEN_FUNCTION,
        0,
        FunctionFlags::SQLITE_UTF8,
        move |_| {
            let mut token = function_token.lock().map_err(|_| {
                rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                    "generation DB mutation token lock is poisoned",
                )))
            })?;
            Ok(token
                .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
                .clone())
        },
    )?;
    let committed_token = Arc::clone(&transaction_token);
    source.commit_hook(Some(move || {
        committed_token.lock().map_or(true, |mut token| {
            *token = None;
            false
        })
    }))?;
    source.rollback_hook(Some(move || {
        if let Ok(mut token) = transaction_token.lock() {
            *token = None;
        }
    }))?;
    Ok(())
}

pub(crate) fn create_mutation_epoch_triggers(source: &Connection) -> Result<()> {
    let mut statement = source.prepare(
        "SELECT name
         FROM sqlite_schema
         WHERE type = 'table'
           AND name NOT LIKE 'sqlite_%'
           AND name != ?1
         ORDER BY name",
    )?;
    let tables = statement
        .query_map([MUTATION_EPOCH_TABLE], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    for (index, table) in tables.iter().enumerate() {
        let table = quote_identifier(table);
        for (operation, suffix) in [("INSERT", "ai"), ("UPDATE", "au"), ("DELETE", "ad")] {
            let trigger = quote_identifier(&format!(
                "conary_generation_mutation_epoch_{index:03}_{suffix}"
            ));
            source.execute_batch(&format!(
                "CREATE TRIGGER {trigger}
                 AFTER {operation} ON {table}
                 WHEN (SELECT token FROM {MUTATION_EPOCH_TABLE} WHERE singleton = 1)
                      != {MUTATION_TOKEN_FUNCTION}()
                 BEGIN
                     UPDATE {MUTATION_EPOCH_TABLE}
                     SET token = {MUTATION_TOKEN_FUNCTION}()
                     WHERE singleton = 1;
                 END;"
            ))?;
        }
    }
    Ok(())
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
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

    fn create_epoch_fixture(path: &Path) -> Connection {
        let connection = Connection::open(path).unwrap();
        configure_mutation_epoch(&connection).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE generation_db_mutation_epoch (
                     singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                     token TEXT NOT NULL CHECK(length(token) = 36)
                 );
                 INSERT INTO generation_db_mutation_epoch (singleton, token)
                 VALUES (1, '00000000-0000-0000-0000-000000000000');
                 CREATE TABLE authority (
                     id INTEGER PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 INSERT INTO authority (id, value) VALUES (1, 'base');",
            )
            .unwrap();
        create_mutation_epoch_triggers(&connection).unwrap();
        connection
    }

    #[test]
    fn session_delta_replays_exact_insert_update_and_delete() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let base_path = temp.path().join("base.sqlite");
        let source = create_epoch_fixture(&source_path);
        let base = create_epoch_fixture(&base_path);
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
        base.execute(
            "UPDATE generation_db_mutation_epoch SET token = ?1 WHERE singleton = 1",
            [&delta.result_baseline().mutation_token],
        )
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
        let source = create_epoch_fixture(&source_path);
        let writer = Connection::open(&source_path).unwrap();
        configure_mutation_epoch(&writer).unwrap();
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
    fn mutation_token_admits_only_the_exact_prior_source_state() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let source = create_epoch_fixture(&source_path);
        let baseline = read_baseline_token(&source).unwrap();
        let recorder =
            GenerationDbDeltaRecorder::begin_against(&source, &source_path, baseline.clone())
                .unwrap();
        source
            .execute(
                "INSERT INTO authority (id, value) VALUES (2, 'same connection')",
                [],
            )
            .unwrap();

        let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
            panic!("matching mutation token unexpectedly required a full snapshot");
        };
        assert_ne!(delta.result_baseline(), &baseline);
        assert_eq!(
            delta.result_baseline(),
            &read_baseline_token(&source).unwrap()
        );
    }

    #[test]
    fn cumulative_capture_excludes_internal_epoch_and_tracks_latest_token() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let base_path = temp.path().join("base.sqlite");
        let source = create_epoch_fixture(&source_path);
        let base = create_epoch_fixture(&base_path);
        let baseline = read_baseline_token(&source).unwrap();
        let mut recorder =
            GenerationDbDeltaRecorder::begin_against(&source, &source_path, baseline).unwrap();

        source
            .execute("INSERT INTO authority (id, value) VALUES (2, 'first')", [])
            .unwrap();
        let GenerationDbDeltaCapture::Captured(first) = recorder.capture().unwrap() else {
            panic!("first cumulative capture unexpectedly required a full snapshot");
        };
        source
            .execute("INSERT INTO authority (id, value) VALUES (3, 'second')", [])
            .unwrap();
        let GenerationDbDeltaCapture::Captured(second) = recorder.capture().unwrap() else {
            panic!("second cumulative capture unexpectedly required a full snapshot");
        };

        assert!(second.payload_bytes() > first.payload_bytes());
        let mut input = second.bytes();
        base.apply_strm(&mut input, None::<fn(&str) -> bool>, |_conflict, _item| {
            ConflictAction::SQLITE_CHANGESET_ABORT
        })
        .unwrap();
        base.execute(
            "UPDATE generation_db_mutation_epoch SET token = ?1 WHERE singleton = 1",
            [&second.result_baseline().mutation_token],
        )
        .unwrap();

        assert_eq!(
            base.query_row("SELECT COUNT(*) FROM authority", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            read_baseline_token(&base).unwrap(),
            *second.result_baseline()
        );
    }

    #[test]
    fn committed_source_change_rejects_the_prior_baseline_token() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let source = create_epoch_fixture(&source_path);
        let baseline = read_baseline_token(&source).unwrap();
        source
            .execute(
                "INSERT INTO authority (id, value) VALUES (2, 'committed divergence')",
                [],
            )
            .unwrap();
        let recorder =
            GenerationDbDeltaRecorder::begin_against(&source, &source_path, baseline).unwrap();

        assert_eq!(
            recorder.finish().unwrap(),
            GenerationDbDeltaCapture::Fallback(
                GenerationDbDeltaFallbackReason::SourceBaselineChanged
            )
        );
    }

    #[test]
    fn one_transaction_uses_one_mutation_token_for_many_rows() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let source = create_epoch_fixture(&source_path);
        let before = read_baseline_token(&source).unwrap();
        let tx = source.unchecked_transaction().unwrap();
        tx.execute("INSERT INTO authority (id, value) VALUES (2, 'first')", [])
            .unwrap();
        let after_first = read_baseline_token(&tx).unwrap();
        tx.execute("INSERT INTO authority (id, value) VALUES (3, 'second')", [])
            .unwrap();
        let after_second = read_baseline_token(&tx).unwrap();
        tx.commit().unwrap();

        assert_ne!(after_first, before);
        assert_eq!(after_second, after_first);
        assert_eq!(read_baseline_token(&source).unwrap(), after_first);
    }

    #[test]
    fn another_configured_connection_advances_the_persisted_token() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let source = create_epoch_fixture(&source_path);
        let before = read_baseline_token(&source).unwrap();
        let writer = Connection::open(&source_path).unwrap();
        configure_mutation_epoch(&writer).unwrap();

        writer
            .execute(
                "INSERT INTO authority (id, value) VALUES (2, 'other connection')",
                [],
            )
            .unwrap();

        assert_ne!(read_baseline_token(&source).unwrap(), before);
    }

    #[test]
    fn unconfigured_writer_cannot_bypass_the_mutation_epoch() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let _source = create_epoch_fixture(&source_path);
        let writer = Connection::open(&source_path).unwrap();

        let error = writer
            .execute(
                "INSERT INTO authority (id, value) VALUES (2, 'untracked write')",
                [],
            )
            .unwrap_err();

        assert!(error.to_string().contains(MUTATION_TOKEN_FUNCTION));
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

        let table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*)
                 FROM sqlite_schema
                 WHERE type = 'table'
                   AND name NOT LIKE 'sqlite_%'
                   AND name != ?1",
                [MUTATION_EPOCH_TABLE],
                |row| row.get(0),
            )
            .unwrap();
        let trigger_count: i64 = connection
            .query_row(
                "SELECT COUNT(*)
                 FROM sqlite_schema
                 WHERE type = 'trigger'
                   AND name LIKE 'conary_generation_mutation_epoch_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trigger_count, table_count * 3);
    }
}
