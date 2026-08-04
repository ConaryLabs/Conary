// conary-core/src/db/models/persisted_value.rs

//! Typed failures for persisted enum and tagged-value corruption.

use rusqlite::types::Type;
use thiserror::Error;

/// A persisted value that cannot be represented by its owning Rust type.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid persisted {kind} value {value:?}: expected {expected}")]
pub struct InvalidPersistedValue {
    kind: &'static str,
    value: String,
    expected: &'static str,
}

impl InvalidPersistedValue {
    pub(super) fn new(
        kind: &'static str,
        value: impl Into<String>,
        expected: &'static str,
    ) -> Self {
        Self {
            kind,
            value: value.into(),
            expected,
        }
    }

    /// The persisted value that failed validation.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Exact row context for a persisted value that violates its typed contract.
#[derive(Debug, Error)]
#[error("database corruption in {table} row {row_id}: column {column} contains {source}")]
pub struct PersistedValueCorruption {
    table: &'static str,
    row_id: i64,
    column: &'static str,
    #[source]
    source: InvalidPersistedValue,
}

impl PersistedValueCorruption {
    /// The table containing the corrupt row.
    pub fn table(&self) -> &str {
        self.table
    }

    /// The primary-key identity of the corrupt row.
    pub fn row_id(&self) -> i64 {
        self.row_id
    }

    /// The column containing the corrupt value.
    pub fn column(&self) -> &str {
        self.column
    }

    /// The invalid persisted value.
    pub fn value(&self) -> &str {
        self.source.value()
    }
}

pub(super) fn persisted_value_corruption(
    table: &'static str,
    row_id: i64,
    column: &'static str,
    column_index: usize,
    source: InvalidPersistedValue,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column_index,
        Type::Text,
        Box::new(PersistedValueCorruption {
            table,
            row_id,
            column,
            source,
        }),
    )
}
