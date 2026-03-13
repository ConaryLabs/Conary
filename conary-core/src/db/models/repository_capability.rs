// conary-core/src/db/models/repository_capability.rs

//! Normalized repository-native capability tables.

use crate::error::Result;
use rusqlite::{Connection, Row, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryProvide {
    pub id: Option<i64>,
    pub repository_package_id: i64,
    pub capability: String,
    pub version: Option<String>,
    pub kind: String,
    pub raw: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRequirement {
    pub id: Option<i64>,
    pub repository_package_id: i64,
    pub capability: String,
    pub version_constraint: Option<String>,
    pub kind: String,
    pub dependency_type: String,
    pub raw: Option<String>,
}

impl RepositoryProvide {
    pub fn new(
        repository_package_id: i64,
        capability: String,
        version: Option<String>,
        kind: String,
        raw: Option<String>,
    ) -> Self {
        Self {
            id: None,
            repository_package_id,
            capability,
            version,
            kind,
            raw,
        }
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO repository_provides
             (repository_package_id, capability, version, kind, raw)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                self.repository_package_id,
                &self.capability,
                &self.version,
                &self.kind,
                &self.raw,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    pub fn batch_insert(conn: &Connection, provides: &[Self]) -> Result<usize> {
        if provides.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare_cached(
            "INSERT INTO repository_provides
             (repository_package_id, capability, version, kind, raw)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;

        for provide in provides {
            stmt.execute(params![
                provide.repository_package_id,
                &provide.capability,
                &provide.version,
                &provide.kind,
                &provide.raw,
            ])?;
        }

        Ok(provides.len())
    }

    pub fn find_by_repository_package(conn: &Connection, repository_package_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_package_id, capability, version, kind, raw
             FROM repository_provides
             WHERE repository_package_id = ?1
             ORDER BY capability, version",
        )?;
        let rows = stmt
            .query_map([repository_package_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn find_by_capability(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version, rp.kind, rp.raw
             FROM repository_provides rp
             JOIN repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1
             ORDER BY rp.capability, rp.version",
        )?;
        let rows = stmt
            .query_map([capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            repository_package_id: row.get(1)?,
            capability: row.get(2)?,
            version: row.get(3)?,
            kind: row.get(4)?,
            raw: row.get(5)?,
        })
    }
}

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
            capability,
            version_constraint,
            kind,
            dependency_type,
            raw,
        }
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO repository_requirements
             (repository_package_id, capability, version_constraint, kind, dependency_type, raw)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.repository_package_id,
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
             (repository_package_id, capability, version_constraint, kind, dependency_type, raw)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        for requirement in requirements {
            stmt.execute(params![
                requirement.repository_package_id,
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
            "SELECT id, repository_package_id, capability, version_constraint, kind, dependency_type, raw
             FROM repository_requirements
             WHERE repository_package_id = ?1
             ORDER BY capability, version_constraint",
        )?;
        let rows = stmt
            .query_map([repository_package_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            repository_package_id: row.get(1)?,
            capability: row.get(2)?,
            version_constraint: row.get(3)?,
            kind: row.get(4)?,
            dependency_type: row.get(5)?,
            raw: row.get(6)?,
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

    #[test]
    fn repository_provide_round_trip() {
        let conn = test_db();
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

        let mut provide = RepositoryProvide::new(
            1,
            "mail-transport-agent".to_string(),
            None,
            "package".to_string(),
            Some("mail-transport-agent".to_string()),
        );
        provide.insert(&conn).unwrap();

        let found = RepositoryProvide::find_by_repository_package(&conn, 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].capability, "mail-transport-agent");
    }

    #[test]
    fn repository_requirement_round_trip() {
        let conn = test_db();
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
}
