// crates/conary-core/src/repository/catalog/candidate/lifecycle.rs

//! Candidate lifecycle helpers shared by creation, validation, and cleanup.

use std::fs;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use super::super::store::sidecar_path;
use crate::error::{Error, Result};

pub(super) fn read_positive_pragma(connection: &Connection, pragma: &str) -> Result<u64> {
    let value: i64 = connection.query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))?;
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| Error::InitError(format!("catalog candidate has invalid {pragma} {value}")))
}

pub(super) fn table_exists(connection: &Connection, table: &str) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub(super) fn remove_candidate_files(path: &Path) {
    let _ = fs::remove_file(path);
    for suffix in ["-journal", "-wal", "-shm"] {
        let _ = fs::remove_file(sidecar_path(path, suffix));
    }
}
