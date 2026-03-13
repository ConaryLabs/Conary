// conary-core/src/db/models/repository_requirement.rs

//! Normalized repository requirement tables (groups and flat clauses).

use crate::error::Result;
use rusqlite::{Connection, Row, params};

/// A flat requirement row from `repository_requirements` (legacy / simple queries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRequirement {
    pub id: Option<i64>,
    pub repository_package_id: i64,
    /// FK to `repository_requirement_groups.id` when this clause belongs to a group.
    pub group_id: Option<i64>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

// ---------------------------------------------------------------------------
// RepositoryRequirement (flat clause table)
// ---------------------------------------------------------------------------

impl RepositoryRequirement {
    pub fn new(
        repository_package_id: i64,
        capability: String,
        version_constraint: Option<String>,
        kind: String,
        dependency_type: String,
        raw: Option<String>,
    ) -> Self {
        Self {
            id: None,
            repository_package_id,
            group_id: None,
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
        self.group_id = Some(group_id);
        self
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
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

    pub fn find_by_repository_package(conn: &Connection, repository_package_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_package_id, group_id, capability, version_constraint, kind, dependency_type, raw
             FROM repository_requirements
             WHERE repository_package_id = ?1
             ORDER BY capability, version_constraint",
        )?;
        let rows = stmt
            .query_map([repository_package_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// List all requirement clauses belonging to a specific group.
    pub fn find_by_group(conn: &Connection, group_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_package_id, group_id, capability, version_constraint, kind, dependency_type, raw
             FROM repository_requirements
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
    ) -> Self {
        Self {
            id: None,
            repository_package_id,
            kind,
            behavior,
            description: None,
            native_text: None,
        }
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO repository_requirement_groups
             (repository_package_id, kind, behavior, description, native_text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                self.repository_package_id,
                &self.kind,
                &self.behavior,
                &self.description,
                &self.native_text,
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
             (repository_package_id, kind, behavior, description, native_text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;

        for group in groups {
            stmt.execute(params![
                group.repository_package_id,
                &group.kind,
                &group.behavior,
                &group.description,
                &group.native_text,
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
             (repository_package_id, kind, behavior, description, native_text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;

        for group in groups.iter_mut() {
            stmt.execute(params![
                group.repository_package_id,
                &group.kind,
                &group.behavior,
                &group.description,
                &group.native_text,
            ])?;
            group.id = Some(conn.last_insert_rowid());
        }

        Ok(groups.len())
    }

    /// List all requirement groups for a given repository package.
    pub fn find_by_repository_package(conn: &Connection, repository_package_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_package_id, kind, behavior, description, native_text
             FROM repository_requirement_groups
             WHERE repository_package_id = ?1
             ORDER BY id",
        )?;
        let rows = stmt
            .query_map([repository_package_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    fn seed_repo_and_package(conn: &Connection) {
        conn.execute(
            "INSERT INTO repositories (name, url) VALUES ('repo', 'https://example.test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url)
             VALUES (1, 'pkg', '1.0', 'sha256:test', 1, 'https://example.test/pkg')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn repository_requirement_round_trip() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut requirement = RepositoryRequirement::new(
            1,
            "libmagic".to_string(),
            Some(">= 1.0".to_string()),
            "package".to_string(),
            "runtime".to_string(),
            Some("libmagic >= 1.0".to_string()),
        );
        requirement.insert(&conn).unwrap();

        let found = RepositoryRequirement::find_by_repository_package(&conn, 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].capability, "libmagic");
        assert_eq!(found[0].version_constraint.as_deref(), Some(">= 1.0"));
    }

    #[test]
    fn delete_by_package_removes_requirements() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut req = RepositoryRequirement::new(
            1, "glibc".to_string(), None, "package".to_string(), "runtime".to_string(), None,
        );
        req.insert(&conn).unwrap();

        RepositoryRequirement::delete_by_package(&conn, 1).unwrap();
        let found = RepositoryRequirement::find_by_repository_package(&conn, 1).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn delete_by_repository_removes_requirements() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut req = RepositoryRequirement::new(
            1, "glibc".to_string(), None, "package".to_string(), "runtime".to_string(), None,
        );
        req.insert(&conn).unwrap();

        RepositoryRequirement::delete_by_repository(&conn, 1).unwrap();
        let found = RepositoryRequirement::find_by_repository_package(&conn, 1).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn requirement_group_round_trip() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut group = RepositoryRequirementGroup::new(
            1,
            "depends".to_string(),
            "hard".to_string(),
        );
        group.native_text = Some("default-mta | mail-transport-agent".to_string());
        group.insert(&conn).unwrap();
        assert!(group.id.is_some());

        let found = RepositoryRequirementGroup::find_by_repository_package(&conn, 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "depends");
        assert_eq!(found[0].behavior, "hard");
        assert_eq!(
            found[0].native_text.as_deref(),
            Some("default-mta | mail-transport-agent"),
        );
    }

    #[test]
    fn delete_groups_by_package() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut group = RepositoryRequirementGroup::new(1, "depends".to_string(), "hard".to_string());
        group.insert(&conn).unwrap();

        RepositoryRequirementGroup::delete_by_package(&conn, 1).unwrap();
        let found = RepositoryRequirementGroup::find_by_repository_package(&conn, 1).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn delete_groups_by_repository() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut group = RepositoryRequirementGroup::new(1, "depends".to_string(), "hard".to_string());
        group.insert(&conn).unwrap();

        RepositoryRequirementGroup::delete_by_repository(&conn, 1).unwrap();
        let found = RepositoryRequirementGroup::find_by_repository_package(&conn, 1).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn batch_insert_groups() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let groups = vec![
            RepositoryRequirementGroup::new(1, "depends".to_string(), "hard".to_string()),
            RepositoryRequirementGroup::new(1, "optional".to_string(), "hard".to_string()),
        ];
        let count = RepositoryRequirementGroup::batch_insert(&conn, &groups).unwrap();
        assert_eq!(count, 2);

        let found = RepositoryRequirementGroup::find_by_repository_package(&conn, 1).unwrap();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn find_clauses_by_group() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        // Create a group for an OR-dependency: default-mta | mail-transport-agent
        let mut group = RepositoryRequirementGroup::new(
            1,
            "depends".to_string(),
            "hard".to_string(),
        );
        group.native_text = Some("default-mta | mail-transport-agent".to_string());
        group.insert(&conn).unwrap();
        let group_id = group.id.unwrap();

        // Insert two OR-alternative clauses linked to the group
        let mut clause_a = RepositoryRequirement::new(
            1,
            "default-mta".to_string(),
            None,
            "package".to_string(),
            "runtime".to_string(),
            None,
        )
        .with_group(group_id);
        clause_a.insert(&conn).unwrap();

        let mut clause_b = RepositoryRequirement::new(
            1,
            "mail-transport-agent".to_string(),
            None,
            "package".to_string(),
            "runtime".to_string(),
            None,
        )
        .with_group(group_id);
        clause_b.insert(&conn).unwrap();

        // Also insert a clause with no group (legacy / ungrouped)
        let mut ungrouped = RepositoryRequirement::new(
            1, "libc".to_string(), None, "package".to_string(), "runtime".to_string(), None,
        );
        ungrouped.insert(&conn).unwrap();

        // find_by_group should return only the two linked clauses
        let clauses = RepositoryRequirement::find_by_group(&conn, group_id).unwrap();
        assert_eq!(clauses.len(), 2);
        assert_eq!(clauses[0].capability, "default-mta");
        assert_eq!(clauses[1].capability, "mail-transport-agent");
        assert_eq!(clauses[0].group_id, Some(group_id));

        // find_by_repository_package returns all 3
        let all = RepositoryRequirement::find_by_repository_package(&conn, 1).unwrap();
        assert_eq!(all.len(), 3);
    }
}
