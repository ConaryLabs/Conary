// src/db/models/provenance.rs

//! Provenance model - supply chain tracking for troves

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// Provenance tracks the supply chain for a trove
#[derive(Debug, Clone)]
pub struct Provenance {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub source_url: Option<String>,
    pub source_branch: Option<String>,
    pub source_commit: Option<String>,
    pub build_host: Option<String>,
    pub build_time: Option<String>,
    pub builder: Option<String>,
}

impl Provenance {
    /// Create a new Provenance
    pub fn new(trove_id: i64) -> Self {
        Self {
            id: None,
            trove_id,
            source_url: None,
            source_branch: None,
            source_commit: None,
            build_host: None,
            build_time: None,
            builder: None,
        }
    }

    /// Insert this provenance into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO provenance (trove_id, source_url, source_branch, source_commit, build_host, build_time, builder)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &self.trove_id,
                &self.source_url,
                &self.source_branch,
                &self.source_commit,
                &self.build_host,
                &self.build_time,
                &self.builder,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find provenance for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, source_url, source_branch, source_commit, build_host, build_time, builder
             FROM provenance WHERE trove_id = ?1",
        )?;

        let provenance = stmt.query_row([trove_id], Self::from_row).optional()?;

        Ok(provenance)
    }

    /// Update provenance information
    pub fn update(&self, conn: &Connection) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update provenance without ID".to_string())
        })?;

        conn.execute(
            "UPDATE provenance SET source_url = ?1, source_branch = ?2, source_commit = ?3,
             build_host = ?4, build_time = ?5, builder = ?6 WHERE id = ?7",
            params![
                &self.source_url,
                &self.source_branch,
                &self.source_commit,
                &self.build_host,
                &self.build_time,
                &self.builder,
                id,
            ],
        )?;

        Ok(())
    }

    /// Delete provenance by trove ID
    pub fn delete(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute("DELETE FROM provenance WHERE trove_id = ?1", [trove_id])?;
        Ok(())
    }

    /// Convert a database row to a Provenance
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            source_url: row.get(2)?,
            source_branch: row.get(3)?,
            source_commit: row.get(4)?,
            build_host: row.get(5)?,
            build_time: row.get(6)?,
            builder: row.get(7)?,
        })
    }
}
