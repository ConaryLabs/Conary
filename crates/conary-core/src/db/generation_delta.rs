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
        Self::begin_with_arm_hook(source, source_path, || Ok(()))
    }

    fn begin_with_arm_hook(
        source: &'connection Connection,
        source_path: impl AsRef<Path>,
        arm_hook: impl FnOnce() -> Result<()>,
    ) -> Result<Self> {
        let source_path = source_path.as_ref();
        validate_source_connection_path(source, source_path)?;
        let before_data_version = data_version(source)?;
        let source_baseline = read_baseline_token(source)?;
        arm_hook()?;
        let mut session = Session::new(source)?;
        session.table_filter(Some(|table: &str| table != MUTATION_EPOCH_TABLE));
        session.attach::<&str>(None)?;
        let starting_data_version = data_version(source)?;
        let baseline_fallback = (starting_data_version != before_data_version)
            .then_some(GenerationDbDeltaFallbackReason::ConcurrentConnectionWrite);
        Ok(Self {
            session,
            source,
            source_path: source_path.to_path_buf(),
            source_baseline,
            starting_data_version,
            baseline_fallback,
        })
    }

    pub fn begin_against(
        source: &'connection Connection,
        source_path: impl AsRef<Path>,
        expected: GenerationDbBaselineToken,
    ) -> Result<Self> {
        let mut recorder = Self::begin(source, source_path)?;
        if recorder.source_baseline != expected && recorder.baseline_fallback.is_none() {
            recorder.baseline_fallback =
                Some(GenerationDbDeltaFallbackReason::SourceBaselineChanged);
        }
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
mod tests;
