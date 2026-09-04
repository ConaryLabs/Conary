// crates/conary-core/src/db/models/repository_requirement.rs

//! Normalized repository requirement expression groups and clause indexes.

use crate::error::Result;
use rusqlite::{Connection, Row, params};
use std::collections::BTreeSet;

/// A searchable clause index belonging to an authoritative requirement group.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepositoryRequirement {
    pub id: Option<i64>,
    pub repository_package_id: i64,
    /// Required FK to the authoritative requirement group.
    pub group_id: i64,
    pub capability: String,
    pub version_constraint: Option<String>,
    pub kind: String,
    pub dependency_type: String,
    pub raw: Option<String>,
}

/// A requirement group row from `repository_requirement_groups`.
///
/// Each group represents a single dependency entry that may contain one or more
/// alternative clauses (OR semantics).  The clauses themselves live in
/// `repository_requirements` linked by `group_id`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepositoryRequirementGroup {
    pub id: Option<i64>,
    pub repository_package_id: i64,
    /// Requirement kind: depends, pre_depends, optional, build, conflict, breaks.
    pub kind: String,
    /// Conditional behavior: hard, conditional, unsupported_rich.
    pub behavior: String,
    /// Optional description (for optional/recommended deps).
    pub description: Option<String>,
    /// Original native text for the whole group.
    pub native_text: Option<String>,
    /// Serialized typed native requirement expression.
    pub expression_json: String,
}

// ---------------------------------------------------------------------------
// RepositoryRequirement (flat clause table)
// ---------------------------------------------------------------------------

impl RepositoryRequirement {
    pub fn new(
        repository_package_id: i64,
        group_id: i64,
        capability: String,
        version_constraint: Option<String>,
        kind: String,
        dependency_type: String,
        raw: Option<String>,
    ) -> Self {
        Self {
            id: None,
            repository_package_id,
            group_id,
            capability,
            version_constraint,
            kind,
            dependency_type,
            raw,
        }
    }

    /// Set the group this clause belongs to.
    #[must_use]
    pub fn with_group(mut self, group_id: i64) -> Self {
        self.group_id = group_id;
        self
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        if self.group_id <= 0 {
            return Err(crate::error::Error::InitError(
                "repository requirement atom has no authoritative group".to_string(),
            ));
        }
        conn.execute(
            "INSERT INTO repository_requirements
             (repository_package_id, group_id, capability, version_constraint, kind, dependency_type, raw)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                self.repository_package_id,
                self.group_id,
                &self.capability,
                &self.version_constraint,
                &self.kind,
                &self.dependency_type,
                &self.raw,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    pub fn batch_insert(conn: &Connection, requirements: &[Self]) -> Result<usize> {
        if requirements.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare_cached(
            "INSERT INTO repository_requirements
             (repository_package_id, group_id, capability, version_constraint, kind, dependency_type, raw)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;

        for requirement in requirements {
            if requirement.group_id <= 0 {
                return Err(crate::error::Error::InitError(
                    "repository requirement atom has no authoritative group".to_string(),
                ));
            }
            stmt.execute(params![
                requirement.repository_package_id,
                requirement.group_id,
                &requirement.capability,
                &requirement.version_constraint,
                &requirement.kind,
                &requirement.dependency_type,
                &requirement.raw,
            ])?;
        }

        Ok(requirements.len())
    }

    pub fn find_by_repository_package(
        conn: &Connection,
        repository_package_id: i64,
    ) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_package_id, group_id, capability, version_constraint, kind, dependency_type, raw
             FROM resolved_repository_requirements
             WHERE repository_package_id = ?1
             ORDER BY capability, version_constraint",
        )?;
        let rows = stmt
            .query_map([repository_package_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Load exact requirement clauses for a bounded set of repository
    /// packages in one query.
    pub fn find_by_repository_packages(
        conn: &Connection,
        repository_package_ids: &[i64],
    ) -> Result<Vec<Self>> {
        if repository_package_ids.is_empty() {
            return Ok(Vec::new());
        }
        let package_ids = repository_package_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let batch_size = super::sqlite_variable_batch_size(conn)?;
        let mut rows = Vec::new();
        for package_ids in package_ids.chunks(batch_size) {
            let placeholders = (1..=package_ids.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT id, repository_package_id, group_id, capability, version_constraint, kind,
                        dependency_type, raw
                 FROM resolved_repository_requirements
                 WHERE repository_package_id IN ({placeholders})"
            );
            let mut stmt = conn.prepare(&sql)?;
            rows.extend(
                stmt.query_map(
                    rusqlite::params_from_iter(package_ids.iter()),
                    Self::from_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            );
        }
        rows.sort_by(|left, right| {
            (
                left.repository_package_id,
                left.group_id,
                &left.capability,
                &left.version_constraint,
            )
                .cmp(&(
                    right.repository_package_id,
                    right.group_id,
                    &right.capability,
                    &right.version_constraint,
                ))
        });
        Ok(rows)
    }

    /// List all requirement clauses belonging to a specific group.
    pub fn find_by_group(conn: &Connection, group_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_package_id, group_id, capability, version_constraint, kind, dependency_type, raw
             FROM resolved_repository_requirements
             WHERE group_id = ?1
             ORDER BY capability, version_constraint",
        )?;
        let rows = stmt
            .query_map([group_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete all requirements for a specific repository package.
    pub fn delete_by_package(conn: &Connection, repository_package_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_requirements WHERE repository_package_id = ?1",
            [repository_package_id],
        )?;
        Ok(())
    }

    /// Delete all requirements for packages belonging to a repository.
    pub fn delete_by_repository(conn: &Connection, repository_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_requirements
             WHERE repository_package_id IN (
                 SELECT id FROM repository_packages WHERE repository_id = ?1
             )",
            [repository_id],
        )?;
        Ok(())
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            repository_package_id: row.get(1)?,
            group_id: row.get(2)?,
            capability: row.get(3)?,
            version_constraint: row.get(4)?,
            kind: row.get(5)?,
            dependency_type: row.get(6)?,
            raw: row.get(7)?,
        })
    }
}

// ---------------------------------------------------------------------------
// RepositoryRequirementGroup
// ---------------------------------------------------------------------------

impl RepositoryRequirementGroup {
    pub fn new(
        repository_package_id: i64,
        kind: String,
        behavior: String,
        expression_json: String,
    ) -> Self {
        Self {
            id: None,
            repository_package_id,
            kind,
            behavior,
            description: None,
            native_text: None,
            expression_json,
        }
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO repository_requirement_groups
             (repository_package_id, kind, behavior, description, native_text, expression_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.repository_package_id,
                &self.kind,
                &self.behavior,
                &self.description,
                &self.native_text,
                &self.expression_json,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    pub fn batch_insert(conn: &Connection, groups: &[Self]) -> Result<usize> {
        if groups.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare_cached(
            "INSERT INTO repository_requirement_groups
             (repository_package_id, kind, behavior, description, native_text, expression_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        for group in groups {
            stmt.execute(params![
                group.repository_package_id,
                &group.kind,
                &group.behavior,
                &group.description,
                &group.native_text,
                &group.expression_json,
            ])?;
        }

        Ok(groups.len())
    }

    /// Batch insert requirement groups and populate their generated IDs.
    pub fn batch_insert_with_ids(conn: &Connection, groups: &mut [Self]) -> Result<usize> {
        if groups.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare_cached(
            "INSERT INTO repository_requirement_groups
             (repository_package_id, kind, behavior, description, native_text, expression_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        for group in groups.iter_mut() {
            stmt.execute(params![
                group.repository_package_id,
                &group.kind,
                &group.behavior,
                &group.description,
                &group.native_text,
                &group.expression_json,
            ])?;
            group.id = Some(conn.last_insert_rowid());
        }

        Ok(groups.len())
    }

    /// List all requirement groups for a given repository package.
    pub fn find_by_repository_package(
        conn: &Connection,
        repository_package_id: i64,
    ) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_package_id, kind, behavior, description, native_text, expression_json
             FROM resolved_repository_requirement_groups
             WHERE repository_package_id = ?1
             ORDER BY id",
        )?;
        let rows = stmt
            .query_map([repository_package_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Load exact requirement groups for a bounded set of repository packages
    /// in one query.
    pub fn find_by_repository_packages(
        conn: &Connection,
        repository_package_ids: &[i64],
    ) -> Result<Vec<Self>> {
        if repository_package_ids.is_empty() {
            return Ok(Vec::new());
        }
        let package_ids = repository_package_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let batch_size = super::sqlite_variable_batch_size(conn)?;
        let mut rows = Vec::new();
        for package_ids in package_ids.chunks(batch_size) {
            let placeholders = (1..=package_ids.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT id, repository_package_id, kind, behavior, description, native_text,
                        expression_json
                 FROM resolved_repository_requirement_groups
                 WHERE repository_package_id IN ({placeholders})"
            );
            let mut stmt = conn.prepare(&sql)?;
            rows.extend(
                stmt.query_map(
                    rusqlite::params_from_iter(package_ids.iter()),
                    Self::from_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            );
        }
        rows.sort_by_key(|group| (group.repository_package_id, group.id));
        Ok(rows)
    }

    /// Delete all requirement groups for a specific repository package.
    pub fn delete_by_package(conn: &Connection, repository_package_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_requirement_groups WHERE repository_package_id = ?1",
            [repository_package_id],
        )?;
        Ok(())
    }

    /// Delete all requirement groups for packages belonging to a repository.
    pub fn delete_by_repository(conn: &Connection, repository_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_requirement_groups
             WHERE repository_package_id IN (
                 SELECT id FROM repository_packages WHERE repository_id = ?1
             )",
            [repository_id],
        )?;
        Ok(())
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            repository_package_id: row.get(1)?,
            kind: row.get(2)?,
            behavior: row.get(3)?,
            description: row.get(4)?,
            native_text: row.get(5)?,
            expression_json: row.get(6)?,
        })
    }
}

#[cfg(test)]
mod tests;
