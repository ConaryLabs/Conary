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
    /// The kind of dependency (package, python, soname, pkgconfig, etc.)
    pub kind: String,
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
            kind: "package".to_string(),
        }
    }

    /// Create a new typed DependencyEntry
    pub fn new_typed(
        trove_id: i64,
        kind: &str,
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
            kind: kind.to_string(),
        }
    }

    /// Insert this dependency into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO dependencies (trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint, kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &self.trove_id,
                &self.depends_on_name,
                &self.depends_on_version,
                &self.dependency_type,
                &self.version_constraint,
                &self.kind,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all dependencies for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint, kind
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
            "SELECT id, trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint, kind
             FROM dependencies WHERE depends_on_name = ?1",
        )?;

        let deps = stmt
            .query_map([package_name], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(deps)
    }

    /// Find all troves that depend on a given typed capability
    pub fn find_typed_dependents(conn: &Connection, kind: &str, name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint, kind
             FROM dependencies WHERE kind = ?1 AND depends_on_name = ?2",
        )?;

        let deps = stmt
            .query_map([kind, name], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(deps)
    }

    /// Find all dependencies of a specific kind for a trove
    pub fn find_by_trove_and_kind(conn: &Connection, trove_id: i64, kind: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint, kind
             FROM dependencies WHERE trove_id = ?1 AND kind = ?2",
        )?;

        let deps = stmt
            .query_map(params![trove_id, kind], Self::from_row)?
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
            kind: row.get::<_, Option<String>>(6)?.unwrap_or_else(|| "package".to_string()),
        })
    }

    /// Format this dependency as a typed string (e.g., "python(requests>=2.0)")
    pub fn to_typed_string(&self) -> String {
        if self.kind == "package" || self.kind.is_empty() {
            // Plain package dependency
            if let Some(ref ver) = self.version_constraint {
                format!("{}{}", self.depends_on_name, ver)
            } else {
                self.depends_on_name.clone()
            }
        } else {
            // Typed dependency
            if let Some(ref ver) = self.version_constraint {
                format!("{}({}{})", self.kind, self.depends_on_name, ver)
            } else {
                format!("{}({})", self.kind, self.depends_on_name)
            }
        }
    }

    /// Parse a typed dependency string and create a DependencyEntry
    pub fn from_typed_string(trove_id: i64, dep_str: &str, dependency_type: &str) -> Self {
        // Try to parse as typed dependency: kind(name) or kind(name>=version)
        if let Some(open) = dep_str.find('(')
            && let Some(close) = dep_str.rfind(')')
            && close > open
        {
            let kind = &dep_str[..open];
            let inner = &dep_str[open + 1..close];

            // Check for version operators
            let version_ops = [">=", "<=", "==", ">", "<", "="];
            for op in &version_ops {
                if let Some(pos) = inner.find(op) {
                    let name = inner[..pos].trim();
                    let ver = inner[pos..].trim();
                    return Self::new_typed(
                        trove_id,
                        kind,
                        name.to_string(),
                        None,
                        dependency_type.to_string(),
                        Some(ver.to_string()),
                    );
                }
            }

            // No version constraint
            return Self::new_typed(
                trove_id,
                kind,
                inner.trim().to_string(),
                None,
                dependency_type.to_string(),
                None,
            );
        }

        // Plain package dependency
        Self::new(
            trove_id,
            dep_str.to_string(),
            None,
            dependency_type.to_string(),
            None,
        )
    }
}
