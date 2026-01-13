// src/db/models/scriptlet_entry.rs

//! ScriptletEntry model - package install/remove hooks

use crate::error::Result;
use rusqlite::{params, Connection, Row};

/// A ScriptletEntry represents a package scriptlet (install/remove hook)
#[derive(Debug, Clone)]
pub struct ScriptletEntry {
    pub id: Option<i64>,
    pub trove_id: i64,
    /// Phase: pre-install, post-install, pre-remove, post-remove, pre-upgrade, post-upgrade
    pub phase: String,
    /// Interpreter path: /bin/sh, /bin/bash, /usr/bin/lua, etc.
    pub interpreter: String,
    /// The script content
    pub content: String,
    /// Optional flags (RPM-specific)
    pub flags: Option<String>,
    /// Package format: rpm, deb, arch - needed for argument handling
    pub package_format: String,
}

impl ScriptletEntry {
    /// Create a new ScriptletEntry
    pub fn new(
        trove_id: i64,
        phase: String,
        interpreter: String,
        content: String,
        package_format: &str,
    ) -> Self {
        Self {
            id: None,
            trove_id,
            phase,
            interpreter,
            content,
            flags: None,
            package_format: package_format.to_string(),
        }
    }

    /// Create a new ScriptletEntry with flags
    pub fn with_flags(
        trove_id: i64,
        phase: String,
        interpreter: String,
        content: String,
        flags: Option<String>,
        package_format: &str,
    ) -> Self {
        Self {
            id: None,
            trove_id,
            phase,
            interpreter,
            content,
            flags,
            package_format: package_format.to_string(),
        }
    }

    /// Insert this scriptlet into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO scriptlets (trove_id, phase, interpreter, content, flags, package_format)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &self.trove_id,
                &self.phase,
                &self.interpreter,
                &self.content,
                &self.flags,
                &self.package_format
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all scriptlets for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, phase, interpreter, content, flags, package_format
             FROM scriptlets WHERE trove_id = ?1 ORDER BY phase",
        )?;

        let scriptlets = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(scriptlets)
    }

    /// Find a specific scriptlet by trove and phase
    pub fn find_by_phase(conn: &Connection, trove_id: i64, phase: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, phase, interpreter, content, flags, package_format
             FROM scriptlets WHERE trove_id = ?1 AND phase = ?2",
        )?;

        let mut rows = stmt.query(params![trove_id, phase])?;

        if let Some(row) = rows.next()? {
            Ok(Some(Self::from_row(row)?))
        } else {
            Ok(None)
        }
    }

    /// Delete all scriptlets for a trove
    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute("DELETE FROM scriptlets WHERE trove_id = ?1", [trove_id])?;
        Ok(())
    }

    /// Convert a database row to a ScriptletEntry
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            phase: row.get(2)?,
            interpreter: row.get(3)?,
            content: row.get(4)?,
            flags: row.get(5)?,
            package_format: row.get(6)?,
        })
    }
}
