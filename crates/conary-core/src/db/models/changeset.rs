// conary-core/src/db/models/changeset.rs

//! Changeset model - atomic transactional operations

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use strum_macros::{AsRefStr, Display, EnumString};

/// Semantic role of a changeset in the effective mutation lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum ChangesetKind {
    Mutation,
    Rollback,
}

impl ChangesetKind {
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }
}

/// Changeset status
#[derive(Debug, Clone, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum ChangesetStatus {
    Pending,
    Applied,
    RolledBack,
}

impl ChangesetStatus {
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }
}

/// A Changeset represents an atomic transactional operation
#[derive(Debug, Clone)]
pub struct Changeset {
    pub id: Option<i64>,
    pub description: String,
    pub kind: ChangesetKind,
    pub status: ChangesetStatus,
    pub created_at: Option<String>,
    pub applied_at: Option<String>,
    pub rolled_back_at: Option<String>,
    pub reversed_by_changeset_id: Option<i64>,
    /// The exact forward mutation reversed by a rollback changeset.
    pub reverts_changeset_id: Option<i64>,
    /// Transaction UUID for crash recovery correlation
    pub tx_uuid: Option<String>,
    /// Serialized trove metadata snapshot stored before removal operations,
    /// enabling rollback of remove changesets.
    /// JSON-encoded trove information; `None` for install/update changesets.
    pub metadata: Option<String>,
}

impl Changeset {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, description, kind, status, created_at, applied_at, \
         rolled_back_at, reversed_by_changeset_id, reverts_changeset_id, tx_uuid, metadata";

    /// Create a new Changeset
    pub fn new(description: String) -> Self {
        Self {
            id: None,
            description,
            kind: ChangesetKind::Mutation,
            status: ChangesetStatus::Pending,
            created_at: None,
            applied_at: None,
            rolled_back_at: None,
            reversed_by_changeset_id: None,
            reverts_changeset_id: None,
            tx_uuid: None,
            metadata: None,
        }
    }

    /// Create a new Changeset with transaction UUID for crash recovery
    pub fn with_tx_uuid(description: String, tx_uuid: String) -> Self {
        Self {
            id: None,
            description,
            kind: ChangesetKind::Mutation,
            status: ChangesetStatus::Pending,
            created_at: None,
            applied_at: None,
            rolled_back_at: None,
            reversed_by_changeset_id: None,
            reverts_changeset_id: None,
            tx_uuid: Some(tx_uuid),
            metadata: None,
        }
    }

    /// Create a typed rollback changeset for one exact forward mutation.
    pub fn new_rollback(description: String, reverts_changeset_id: i64) -> Self {
        Self {
            id: None,
            description,
            kind: ChangesetKind::Rollback,
            status: ChangesetStatus::Pending,
            created_at: None,
            applied_at: None,
            rolled_back_at: None,
            reversed_by_changeset_id: None,
            reverts_changeset_id: Some(reverts_changeset_id),
            tx_uuid: None,
            metadata: None,
        }
    }

    /// Insert this changeset into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO changesets
             (description, kind, status, reverts_changeset_id, tx_uuid)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.description,
                self.kind.as_str(),
                self.status.as_str(),
                self.reverts_changeset_id,
                &self.tx_uuid
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a changeset by transaction UUID
    pub fn find_by_tx_uuid(conn: &Connection, tx_uuid: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM changesets WHERE tx_uuid = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let changeset = stmt.query_row([tx_uuid], Self::from_row).optional()?;
        Ok(changeset)
    }

    /// Find a changeset by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!("SELECT {} FROM changesets WHERE id = ?1", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        let changeset = stmt.query_row([id], Self::from_row).optional()?;
        Ok(changeset)
    }

    /// List all changesets
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM changesets ORDER BY created_at DESC",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let changesets = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(changesets)
    }

    /// Update changeset status
    pub fn update_status(&mut self, conn: &Connection, new_status: ChangesetStatus) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::MissingId("Cannot update changeset without ID".to_string())
        })?;

        match new_status {
            ChangesetStatus::Applied => {
                conn.execute(
                    "UPDATE changesets
                     SET status = ?1, applied_at = COALESCE(applied_at, CURRENT_TIMESTAMP)
                     WHERE id = ?2",
                    params![new_status.as_str(), id],
                )?;
            }
            ChangesetStatus::RolledBack => {
                conn.execute(
                    "UPDATE changesets SET status = ?1, rolled_back_at = CURRENT_TIMESTAMP WHERE id = ?2",
                    params![new_status.as_str(), id],
                )?;
            }
            _ => {
                conn.execute(
                    "UPDATE changesets SET status = ?1 WHERE id = ?2",
                    params![new_status.as_str(), id],
                )?;
            }
        }

        self.status = new_status;
        Ok(())
    }

    /// Convert a database row to a Changeset
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let kind_str: String = row.get(2)?;
        let kind = kind_str.parse::<ChangesetKind>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
        })?;
        let status_str: String = row.get(3)?;
        let status = status_str.parse::<ChangesetStatus>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?;

        Ok(Self {
            id: Some(row.get(0)?),
            description: row.get(1)?,
            kind,
            status,
            created_at: row.get(4)?,
            applied_at: row.get(5)?,
            rolled_back_at: row.get(6)?,
            reversed_by_changeset_id: row.get(7)?,
            reverts_changeset_id: row.get(8)?,
            tx_uuid: row.get(9)?,
            metadata: row.get(10)?,
        })
    }
}
