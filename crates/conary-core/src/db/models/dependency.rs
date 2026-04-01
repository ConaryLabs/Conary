// conary-core/src/db/models/dependency.rs

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
    /// OR-group identifier: rows sharing the same `(trove_id, group_id)` are
    /// alternatives (any one satisfies the requirement).  `None` means this
    /// is a simple single-clause dependency.
    pub group_id: Option<i64>,
}

impl DependencyEntry {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, trove_id, depends_on_name, depends_on_version, \
         dependency_type, version_constraint, kind, group_id";

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
            group_id: None,
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
            group_id: None,
        }
    }

    /// Insert this dependency into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO dependencies (trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint, kind, group_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &self.trove_id,
                &self.depends_on_name,
                &self.depends_on_version,
                &self.dependency_type,
                &self.version_constraint,
                &self.kind,
                &self.group_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Batch insert multiple dependency entries efficiently
    ///
    /// Uses a prepared statement for much better performance than individual
    /// inserts when recording dependencies for many packages at once.
    ///
    /// Caller must wrap this in a transaction for atomicity.
    pub fn batch_insert(conn: &Connection, entries: &[Self]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare_cached(
            "INSERT INTO dependencies (trove_id, depends_on_name, depends_on_version, \
             dependency_type, version_constraint, kind, group_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;

        for entry in entries {
            stmt.execute(params![
                &entry.trove_id,
                &entry.depends_on_name,
                &entry.depends_on_version,
                &entry.dependency_type,
                &entry.version_constraint,
                &entry.kind,
                &entry.group_id,
            ])?;
        }

        Ok(entries.len())
    }

    /// Find all dependencies for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM dependencies WHERE trove_id = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        let deps = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(deps)
    }

    /// Batch-load dependencies for multiple troves in a single query.
    ///
    /// Returns a map from trove_id to its dependencies. Trove IDs with no
    /// dependencies are absent from the map (use `.get(&id).unwrap_or(&vec![])`)`.
    /// Handles SQLite's variable limit by chunking at 500 IDs.
    pub fn find_by_troves(
        conn: &Connection,
        trove_ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, Vec<Self>>> {
        use std::collections::HashMap;

        let mut result: HashMap<i64, Vec<Self>> = HashMap::new();
        if trove_ids.is_empty() {
            return Ok(result);
        }

        for chunk in trove_ids.chunks(500) {
            let placeholders: String = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT {} FROM dependencies WHERE trove_id IN ({placeholders})",
                Self::COLUMNS
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), Self::from_row)?;
            for row in rows {
                let dep = row?;
                result.entry(dep.trove_id).or_default().push(dep);
            }
        }

        Ok(result)
    }

    /// Find all troves that depend on a given package name (reverse dependencies)
    pub fn find_dependents(conn: &Connection, package_name: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM dependencies WHERE depends_on_name = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        let deps = stmt
            .query_map([package_name], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(deps)
    }

    /// Find all troves that depend on a given typed capability
    pub fn find_typed_dependents(conn: &Connection, kind: &str, name: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM dependencies WHERE kind = ?1 AND depends_on_name = ?2",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        let deps = stmt
            .query_map([kind, name], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(deps)
    }

    /// Find all dependencies of a specific kind for a trove
    pub fn find_by_trove_and_kind(
        conn: &Connection,
        trove_id: i64,
        kind: &str,
    ) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM dependencies WHERE trove_id = ?1 AND kind = ?2",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        let deps = stmt
            .query_map(params![trove_id, kind], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(deps)
    }

    /// Find all packages that can satisfy a dependency (by name)
    pub fn find_providers(conn: &Connection, dependency_name: &str) -> Result<Vec<Trove>> {
        let sql = format!("SELECT {} FROM troves WHERE name = ?1", Trove::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;

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
    ///
    /// Schema v52 guarantees all columns exist -- no compat fallbacks needed.
    /// group_id (column 7) added in v62; nullable.
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            depends_on_name: row.get(2)?,
            depends_on_version: row.get(3)?,
            dependency_type: row.get(4)?,
            version_constraint: row.get(5)?,
            kind: row.get::<_, String>(6)?,
            group_id: row.get(7)?,
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
