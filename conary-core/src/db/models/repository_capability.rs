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
    /// Native version comparison scheme (rpm, debian, arch) for the provide version text.
    pub version_scheme: Option<String>,
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
            version_scheme: None,
        }
    }

    /// Create a provide with an explicit version scheme.
    pub fn with_version_scheme(mut self, scheme: String) -> Self {
        self.version_scheme = Some(scheme);
        self
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO repository_provides
             (repository_package_id, capability, version, kind, raw, version_scheme)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.repository_package_id,
                &self.capability,
                &self.version,
                &self.kind,
                &self.raw,
                &self.version_scheme,
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
             (repository_package_id, capability, version, kind, raw, version_scheme)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        for provide in provides {
            stmt.execute(params![
                provide.repository_package_id,
                &provide.capability,
                &provide.version,
                &provide.kind,
                &provide.raw,
                &provide.version_scheme,
            ])?;
        }

        Ok(provides.len())
    }

    pub fn find_by_repository_package(
        conn: &Connection,
        repository_package_id: i64,
    ) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_package_id, capability, version, kind, raw, version_scheme
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
            "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version, rp.kind, rp.raw, rp.version_scheme
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

    /// Find provides matching a capability and return each paired with its package name.
    ///
    /// This single JOIN avoids the N+1 pattern of calling `find_by_capability` and then
    /// issuing a separate `SELECT name FROM repository_packages WHERE id = ?` for each
    /// result row. The `pkg.name` column is appended at position 7 in each row so the
    /// existing `from_row` mapper is not disturbed.
    pub fn find_by_capability_with_name(
        conn: &Connection,
        capability: &str,
    ) -> Result<Vec<(Self, String)>> {
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version, rp.kind, rp.raw, rp.version_scheme, pkg.name
             FROM repository_provides rp
             JOIN repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1
             ORDER BY rp.capability, rp.version",
        )?;
        let rows = stmt
            .query_map([capability], |row| {
                let provide = Self {
                    id: row.get(0)?,
                    repository_package_id: row.get(1)?,
                    capability: row.get(2)?,
                    version: row.get(3)?,
                    kind: row.get(4)?,
                    raw: row.get(5)?,
                    version_scheme: row.get(6)?,
                };
                let pkg_name: String = row.get(7)?;
                Ok((provide, pkg_name))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Find provides matching both capability name and kind in enabled repositories.
    pub fn find_by_capability_and_kind(
        conn: &Connection,
        capability: &str,
        kind: &str,
    ) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version, rp.kind, rp.raw, rp.version_scheme
             FROM repository_provides rp
             JOIN repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1 AND rp.kind = ?2
             ORDER BY rp.capability, rp.version",
        )?;
        let rows = stmt
            .query_map(params![capability, kind], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete all provides for a specific repository package.
    pub fn delete_by_package(conn: &Connection, repository_package_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_provides WHERE repository_package_id = ?1",
            [repository_package_id],
        )?;
        Ok(())
    }

    /// Delete all provides for packages belonging to a repository.
    pub fn delete_by_repository(conn: &Connection, repository_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_provides
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
            capability: row.get(2)?,
            version: row.get(3)?,
            kind: row.get(4)?,
            raw: row.get(5)?,
            version_scheme: row.get(6)?,
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
    fn repository_provide_round_trip() {
        let conn = test_db();
        seed_repo_and_package(&conn);

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
        assert!(found[0].version_scheme.is_none());
    }

    #[test]
    fn repository_provide_with_version_scheme() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut provide = RepositoryProvide::new(
            1,
            "libc.so.6".to_string(),
            Some("2.34".to_string()),
            "soname".to_string(),
            None,
        )
        .with_version_scheme("rpm".to_string());
        provide.insert(&conn).unwrap();

        let found = RepositoryProvide::find_by_repository_package(&conn, 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].version_scheme.as_deref(), Some("rpm"));
    }

    #[test]
    fn find_by_capability_and_kind_filters_correctly() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut p1 =
            RepositoryProvide::new(1, "foo".to_string(), None, "package".to_string(), None);
        p1.insert(&conn).unwrap();
        let mut p2 =
            RepositoryProvide::new(1, "foo".to_string(), None, "virtual".to_string(), None);
        p2.insert(&conn).unwrap();

        let pkg_only =
            RepositoryProvide::find_by_capability_and_kind(&conn, "foo", "package").unwrap();
        assert_eq!(pkg_only.len(), 1);
        assert_eq!(pkg_only[0].kind, "package");
    }

    #[test]
    fn delete_by_package_removes_provides() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut provide =
            RepositoryProvide::new(1, "cap".to_string(), None, "virtual".to_string(), None);
        provide.insert(&conn).unwrap();

        RepositoryProvide::delete_by_package(&conn, 1).unwrap();
        let found = RepositoryProvide::find_by_repository_package(&conn, 1).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn delete_by_repository_removes_provides() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut provide =
            RepositoryProvide::new(1, "cap".to_string(), None, "virtual".to_string(), None);
        provide.insert(&conn).unwrap();

        RepositoryProvide::delete_by_repository(&conn, 1).unwrap();
        let found = RepositoryProvide::find_by_repository_package(&conn, 1).unwrap();
        assert!(found.is_empty());
    }
}
