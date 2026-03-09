// conary-core/src/db/models/canonical.rs

//! Canonical package identity models
//!
//! These models map distro-neutral package identities to distro-specific
//! implementations, enabling cross-distro package resolution.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// A distro-neutral package identity
#[derive(Debug, Clone)]
pub struct CanonicalPackage {
    pub id: Option<i64>,
    pub name: String,
    pub appstream_id: Option<String>,
    pub description: Option<String>,
    pub kind: String,
    pub category: Option<String>,
}

impl CanonicalPackage {
    /// Create a new canonical package
    pub fn new(name: String, kind: String) -> Self {
        Self {
            id: None,
            name,
            appstream_id: None,
            description: None,
            kind,
            category: None,
        }
    }

    /// Insert into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO canonical_packages (name, appstream_id, description, kind, category)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.name,
                &self.appstream_id,
                &self.description,
                &self.kind,
                &self.category,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or ignore if name already exists; returns the id whether inserted or existing
    pub fn insert_or_ignore(&mut self, conn: &Connection) -> Result<Option<i64>> {
        conn.execute(
            "INSERT OR IGNORE INTO canonical_packages (name, appstream_id, description, kind, category)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.name,
                &self.appstream_id,
                &self.description,
                &self.kind,
                &self.category,
            ],
        )?;

        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM canonical_packages WHERE name = ?1",
                [&self.name],
                |row| row.get(0),
            )
            .optional()?;

        self.id = id;
        Ok(id)
    }

    /// Find by canonical name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, name, appstream_id, description, kind, category
                 FROM canonical_packages WHERE name = ?1",
                [name],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Find by AppStream ID
    pub fn find_by_appstream_id(conn: &Connection, appstream_id: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, name, appstream_id, description, kind, category
                 FROM canonical_packages WHERE appstream_id = ?1",
                [appstream_id],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Find by id
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, name, appstream_id, description, kind, category
                 FROM canonical_packages WHERE id = ?1",
                [id],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Resolve a name to a canonical package using multiple strategies:
    /// 1. Try as canonical name
    /// 2. If contains '.', try as AppStream ID
    /// 3. Try as distro-specific name via `PackageImplementation`
    pub fn resolve_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        // 1. Direct canonical name
        if let Some(pkg) = Self::find_by_name(conn, name)? {
            return Ok(Some(pkg));
        }

        // 2. AppStream ID (reverse-DNS pattern contains dots)
        if name.contains('.')
            && let Some(pkg) = Self::find_by_appstream_id(conn, name)?
        {
            return Ok(Some(pkg));
        }

        // 3. Distro-specific name lookup
        if let Some(impl_entry) = PackageImplementation::find_by_any_distro_name(conn, name)? {
            return Self::find_by_id(conn, impl_entry.canonical_id);
        }

        Ok(None)
    }

    /// List all canonical packages of a given kind
    pub fn list_by_kind(conn: &Connection, kind: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, appstream_id, description, kind, category
             FROM canonical_packages WHERE kind = ?1 ORDER BY name",
        )?;
        let rows = stmt
            .query_map([kind], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Search canonical packages by name or description (LIKE)
    pub fn search(conn: &Connection, query: &str) -> Result<Vec<Self>> {
        let pattern = format!("%{query}%");
        let mut stmt = conn.prepare(
            "SELECT id, name, appstream_id, description, kind, category
             FROM canonical_packages
             WHERE name LIKE ?1 OR description LIKE ?1
             ORDER BY name",
        )?;
        let rows = stmt
            .query_map([&pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Map a database row to a `CanonicalPackage`
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            appstream_id: row.get(2)?,
            description: row.get(3)?,
            kind: row.get(4)?,
            category: row.get(5)?,
        })
    }
}

/// A distro-specific implementation of a canonical package
#[derive(Debug, Clone)]
pub struct PackageImplementation {
    pub id: Option<i64>,
    pub canonical_id: i64,
    pub distro: String,
    pub distro_name: String,
    pub repo_id: Option<i64>,
    pub source: String,
}

impl PackageImplementation {
    /// Create a new implementation entry
    pub fn new(canonical_id: i64, distro: String, distro_name: String, source: String) -> Self {
        Self {
            id: None,
            canonical_id,
            distro,
            distro_name,
            repo_id: None,
            source,
        }
    }

    /// Insert into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO package_implementations (canonical_id, distro, distro_name, repo_id, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.canonical_id,
                &self.distro,
                &self.distro_name,
                &self.repo_id,
                &self.source,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or ignore if the (canonical_id, distro, distro_name) combination exists
    pub fn insert_or_ignore(&mut self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO package_implementations (canonical_id, distro, distro_name, repo_id, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.canonical_id,
                &self.distro,
                &self.distro_name,
                &self.repo_id,
                &self.source,
            ],
        )?;
        Ok(())
    }

    /// Find all implementations for a canonical package
    pub fn find_by_canonical(conn: &Connection, canonical_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, canonical_id, distro, distro_name, repo_id, source
             FROM package_implementations WHERE canonical_id = ?1
             ORDER BY distro",
        )?;
        let rows = stmt
            .query_map([canonical_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Find by distro-specific name within a single distro
    pub fn find_by_distro_name(
        conn: &Connection,
        distro: &str,
        name: &str,
    ) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, canonical_id, distro, distro_name, repo_id, source
                 FROM package_implementations WHERE distro = ?1 AND distro_name = ?2 LIMIT 1",
                params![distro, name],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Find by distro-specific name across all distros (first match)
    pub fn find_by_any_distro_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, canonical_id, distro, distro_name, repo_id, source
                 FROM package_implementations WHERE distro_name = ?1 LIMIT 1",
                [name],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Find the implementation for a canonical package on a specific distro
    pub fn find_for_distro(
        conn: &Connection,
        canonical_id: i64,
        distro: &str,
    ) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, canonical_id, distro, distro_name, repo_id, source
                 FROM package_implementations WHERE canonical_id = ?1 AND distro = ?2 LIMIT 1",
                params![canonical_id, distro],
                Self::from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Map a database row to a `PackageImplementation`
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            canonical_id: row.get(1)?,
            distro: row.get(2)?,
            distro_name: row.get(3)?,
            repo_id: row.get(4)?,
            source: row.get(5)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_insert_and_find_canonical() {
        let (_temp, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        pkg.description = Some("Web browser".to_string());
        pkg.category = Some("browser".to_string());
        let id = pkg.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by name
        let found = CanonicalPackage::find_by_name(&conn, "firefox")
            .unwrap()
            .unwrap();
        assert_eq!(found.name, "firefox");
        assert_eq!(found.kind, "package");
        assert_eq!(found.description, Some("Web browser".to_string()));
        assert_eq!(found.category, Some("browser".to_string()));

        // Find by id
        let found = CanonicalPackage::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.name, "firefox");

        // insert_or_ignore returns existing id
        let mut dup = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        let dup_id = dup.insert_or_ignore(&conn).unwrap();
        assert_eq!(dup_id, Some(id));
    }

    #[test]
    fn test_find_by_appstream_id() {
        let (_temp, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        pkg.appstream_id = Some("org.mozilla.firefox".to_string());
        pkg.insert(&conn).unwrap();

        let found = CanonicalPackage::find_by_appstream_id(&conn, "org.mozilla.firefox")
            .unwrap()
            .unwrap();
        assert_eq!(found.name, "firefox");

        // Not found
        let missing = CanonicalPackage::find_by_appstream_id(&conn, "org.nonexistent.app").unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn test_insert_and_find_implementation() {
        let (_temp, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        let can_id = pkg.insert(&conn).unwrap();

        let mut impl_fed = PackageImplementation::new(
            can_id,
            "fedora".to_string(),
            "firefox".to_string(),
            "auto".to_string(),
        );
        impl_fed.insert(&conn).unwrap();

        let mut impl_deb = PackageImplementation::new(
            can_id,
            "debian".to_string(),
            "firefox-esr".to_string(),
            "auto".to_string(),
        );
        impl_deb.insert(&conn).unwrap();

        // Find by canonical
        let impls = PackageImplementation::find_by_canonical(&conn, can_id).unwrap();
        assert_eq!(impls.len(), 2);

        // Find by distro name
        let found = PackageImplementation::find_by_distro_name(&conn, "debian", "firefox-esr")
            .unwrap()
            .unwrap();
        assert_eq!(found.distro, "debian");
        assert_eq!(found.distro_name, "firefox-esr");

        // Find by any distro name
        let found = PackageImplementation::find_by_any_distro_name(&conn, "firefox-esr")
            .unwrap()
            .unwrap();
        assert_eq!(found.distro, "debian");

        // Find for distro
        let found = PackageImplementation::find_for_distro(&conn, can_id, "fedora")
            .unwrap()
            .unwrap();
        assert_eq!(found.distro_name, "firefox");

        // insert_or_ignore does not fail on duplicate
        let mut dup = PackageImplementation::new(
            can_id,
            "fedora".to_string(),
            "firefox".to_string(),
            "auto".to_string(),
        );
        dup.insert_or_ignore(&conn).unwrap();
    }

    #[test]
    fn test_resolve_name_to_canonical() {
        let (_temp, conn) = create_test_db();

        let mut pkg = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        pkg.appstream_id = Some("org.mozilla.firefox".to_string());
        let can_id = pkg.insert(&conn).unwrap();

        let mut impl_deb = PackageImplementation::new(
            can_id,
            "debian".to_string(),
            "firefox-esr".to_string(),
            "auto".to_string(),
        );
        impl_deb.insert(&conn).unwrap();

        // Resolve by canonical name
        let resolved = CanonicalPackage::resolve_name(&conn, "firefox")
            .unwrap()
            .unwrap();
        assert_eq!(resolved.name, "firefox");

        // Resolve by AppStream ID
        let resolved = CanonicalPackage::resolve_name(&conn, "org.mozilla.firefox")
            .unwrap()
            .unwrap();
        assert_eq!(resolved.name, "firefox");

        // Resolve by distro-specific name
        let resolved = CanonicalPackage::resolve_name(&conn, "firefox-esr")
            .unwrap()
            .unwrap();
        assert_eq!(resolved.name, "firefox");

        // Unknown name returns None
        let missing = CanonicalPackage::resolve_name(&conn, "nonexistent").unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn test_list_groups() {
        let (_temp, conn) = create_test_db();

        let mut pkg1 = CanonicalPackage::new("firefox".to_string(), "package".to_string());
        pkg1.insert(&conn).unwrap();

        let mut pkg2 = CanonicalPackage::new("bash".to_string(), "package".to_string());
        pkg2.insert(&conn).unwrap();

        let mut grp = CanonicalPackage::new("web-browsers".to_string(), "group".to_string());
        grp.description = Some("Web browser packages".to_string());
        grp.insert(&conn).unwrap();

        // list_by_kind
        let packages = CanonicalPackage::list_by_kind(&conn, "package").unwrap();
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name, "bash"); // ordered by name

        let groups = CanonicalPackage::list_by_kind(&conn, "group").unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "web-browsers");

        // search
        let results = CanonicalPackage::search(&conn, "browser").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "web-browsers");
    }
}
