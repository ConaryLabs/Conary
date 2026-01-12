// src/db/models/changeset.rs

//! Changeset model - atomic transactional operations

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::str::FromStr;

/// Changeset status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesetStatus {
    Pending,
    Applied,
    RolledBack,
}

impl ChangesetStatus {
    pub fn as_str(&self) -> &str {
        match self {
            ChangesetStatus::Pending => "pending",
            ChangesetStatus::Applied => "applied",
            ChangesetStatus::RolledBack => "rolled_back",
        }
    }
}

impl FromStr for ChangesetStatus {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pending" => Ok(ChangesetStatus::Pending),
            "applied" => Ok(ChangesetStatus::Applied),
            "rolled_back" => Ok(ChangesetStatus::RolledBack),
            _ => Err(format!("Invalid changeset status: {s}")),
        }
    }
}

/// A Changeset represents an atomic transactional operation
#[derive(Debug, Clone)]
pub struct Changeset {
    pub id: Option<i64>,
    pub description: String,
    pub status: ChangesetStatus,
    pub created_at: Option<String>,
    pub applied_at: Option<String>,
    pub rolled_back_at: Option<String>,
    pub reversed_by_changeset_id: Option<i64>,
}

impl Changeset {
    /// Create a new Changeset
    pub fn new(description: String) -> Self {
        Self {
            id: None,
            description,
            status: ChangesetStatus::Pending,
            created_at: None,
            applied_at: None,
            rolled_back_at: None,
            reversed_by_changeset_id: None,
        }
    }

    /// Insert this changeset into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES (?1, ?2)",
            params![&self.description, self.status.as_str()],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a changeset by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, description, status, created_at, applied_at, rolled_back_at, reversed_by_changeset_id
             FROM changesets WHERE id = ?1",
        )?;

        let changeset = stmt.query_row([id], Self::from_row).optional()?;

        Ok(changeset)
    }

    /// List all changesets
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, description, status, created_at, applied_at, rolled_back_at, reversed_by_changeset_id
             FROM changesets ORDER BY created_at DESC",
        )?;

        let changesets = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(changesets)
    }

    /// Update changeset status
    pub fn update_status(&mut self, conn: &Connection, new_status: ChangesetStatus) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update changeset without ID".to_string())
        })?;

        let timestamp_field = match new_status {
            ChangesetStatus::Applied => "applied_at",
            ChangesetStatus::RolledBack => "rolled_back_at",
            _ => "",
        };

        if timestamp_field.is_empty() {
            conn.execute(
                "UPDATE changesets SET status = ?1 WHERE id = ?2",
                params![new_status.as_str(), id],
            )?;
        } else {
            conn.execute(
                &format!(
                    "UPDATE changesets SET status = ?1, {timestamp_field} = CURRENT_TIMESTAMP WHERE id = ?2"
                ),
                params![new_status.as_str(), id],
            )?;
        }

        self.status = new_status;
        Ok(())
    }

    /// Convert a database row to a Changeset
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let status_str: String = row.get(2)?;
        let status = status_str.parse::<ChangesetStatus>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            )
        })?;

        Ok(Self {
            id: Some(row.get(0)?),
            description: row.get(1)?,
            status,
            created_at: row.get(3)?,
            applied_at: row.get(4)?,
            rolled_back_at: row.get(5)?,
            reversed_by_changeset_id: row.get(6)?,
        })
    }
}
