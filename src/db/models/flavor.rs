// src/db/models/flavor.rs

//! Flavor model - build-time variations (architecture, features, toolchain)

use crate::error::Result;
use rusqlite::{Connection, Row, params};

/// A Flavor represents a build-time variation (e.g., architecture, features, toolchain)
#[derive(Debug, Clone)]
pub struct Flavor {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub key: String,
    pub value: String,
}

impl Flavor {
    /// Create a new Flavor
    pub fn new(trove_id: i64, key: String, value: String) -> Self {
        Self {
            id: None,
            trove_id,
            key,
            value,
        }
    }

    /// Insert this flavor into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO flavors (trove_id, key, value) VALUES (?1, ?2, ?3)",
            params![&self.trove_id, &self.key, &self.value],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all flavors for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, key, value FROM flavors WHERE trove_id = ?1 ORDER BY key",
        )?;

        let flavors = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(flavors)
    }

    /// Find flavors by key name across all troves
    pub fn find_by_key(conn: &Connection, key: &str) -> Result<Vec<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, trove_id, key, value FROM flavors WHERE key = ?1")?;

        let flavors = stmt
            .query_map([key], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(flavors)
    }

    /// Delete a flavor by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM flavors WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Convert a database row to a Flavor
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            key: row.get(2)?,
            value: row.get(3)?,
        })
    }
}
