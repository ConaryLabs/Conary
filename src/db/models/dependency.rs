// src/db/models/dependency.rs

//! DependencyEntry model - linking troves to their dependencies

use super::trove::Trove;
use crate::error::Result;
use rusqlite::{Connection, Row, params};

/// Dependency entry linking troves to their dependencies
#[derive(Debug, Clone)]
pub struct DependencyEntry {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub depends_on_name: String,
    pub depends_on_version: Option<String>,
    pub dependency_type: String,
    pub version_constraint: Option<String>,
}

impl DependencyEntry {
    /// Create a new DependencyEntry
    pub fn new(
        trove_id: i64,
        depends_on_name: String,
        depends_on_version: Option<String>,
        dependency_type: String,
        version_constraint: Option<String>,
    ) -> Self {
        Self {
            id: None,
            trove_id,
            depends_on_name,
            depends_on_version,
            dependency_type,
            version_constraint,
        }
    }

    /// Insert this dependency into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO dependencies (trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.trove_id,
                &self.depends_on_name,
                &self.depends_on_version,
                &self.dependency_type,
                &self.version_constraint,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all dependencies for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint
             FROM dependencies WHERE trove_id = ?1",
        )?;

        let deps = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(deps)
    }

    /// Find all troves that depend on a given package name (reverse dependencies)
    pub fn find_dependents(conn: &Connection, package_name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint
             FROM dependencies WHERE depends_on_name = ?1",
        )?;

        let deps = stmt
            .query_map([package_name], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(deps)
    }

    /// Find all packages that can satisfy a dependency (by name)
    pub fn find_providers(conn: &Connection, dependency_name: &str) -> Result<Vec<Trove>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id
             FROM troves WHERE name = ?1",
        )?;

        let troves = stmt
            .query_map([dependency_name], Trove::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(troves)
    }

    /// Delete a specific dependency
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM dependencies WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete all dependencies for a trove (called when removing a package)
    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute("DELETE FROM dependencies WHERE trove_id = ?1", [trove_id])?;
        Ok(())
    }

    /// Convert a database row to a DependencyEntry
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            depends_on_name: row.get(2)?,
            depends_on_version: row.get(3)?,
            dependency_type: row.get(4)?,
            version_constraint: row.get(5)?,
        })
    }
}
