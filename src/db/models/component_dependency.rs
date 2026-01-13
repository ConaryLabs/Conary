// src/db/models/component_dependency.rs

//! ComponentDependency model - dependencies between components
//!
//! Component dependencies track relationships between components, both within
//! the same package and across packages. For example:
//! - `nginx:devel` depends on `nginx:lib` (same package)
//! - `nginx:runtime` depends on `openssl:lib` (different package)

use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};

/// Dependency type for component dependencies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentDepType {
    /// Required at runtime
    Runtime,
    /// Required for building
    Build,
    /// Optional dependency
    Optional,
}

impl ComponentDepType {
    /// Convert to string for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Build => "build",
            Self::Optional => "optional",
        }
    }

    /// Parse from string
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "runtime" => Some(Self::Runtime),
            "build" => Some(Self::Build),
            "optional" => Some(Self::Optional),
            _ => None,
        }
    }
}

/// A ComponentDependency represents a dependency from one component to another
#[derive(Debug, Clone)]
pub struct ComponentDependency {
    pub id: Option<i64>,
    pub component_id: i64,
    /// Name of the component being depended on (e.g., "lib", "runtime")
    pub depends_on_component: String,
    /// Package name, or None for same-package dependency
    pub depends_on_package: Option<String>,
    pub dependency_type: ComponentDepType,
    pub version_constraint: Option<String>,
}

impl ComponentDependency {
    /// Create a new ComponentDependency within the same package
    pub fn new_same_package(
        component_id: i64,
        depends_on_component: String,
        dependency_type: ComponentDepType,
    ) -> Self {
        Self {
            id: None,
            component_id,
            depends_on_component,
            depends_on_package: None,
            dependency_type,
            version_constraint: None,
        }
    }

    /// Create a new ComponentDependency to another package
    pub fn new_cross_package(
        component_id: i64,
        depends_on_component: String,
        depends_on_package: String,
        dependency_type: ComponentDepType,
    ) -> Self {
        Self {
            id: None,
            component_id,
            depends_on_component,
            depends_on_package: Some(depends_on_package),
            dependency_type,
            version_constraint: None,
        }
    }

    /// Insert this dependency into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO component_dependencies
             (component_id, depends_on_component, depends_on_package, dependency_type, version_constraint)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.component_id,
                &self.depends_on_component,
                &self.depends_on_package,
                self.dependency_type.as_str(),
                &self.version_constraint,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all dependencies for a component
    pub fn find_by_component(conn: &Connection, component_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, component_id, depends_on_component, depends_on_package, dependency_type, version_constraint
             FROM component_dependencies WHERE component_id = ?1",
        )?;

        let deps = stmt
            .query_map([component_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(deps)
    }

    /// Find a dependency by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, component_id, depends_on_component, depends_on_package, dependency_type, version_constraint
             FROM component_dependencies WHERE id = ?1",
        )?;

        let dep = stmt.query_row([id], Self::from_row).optional()?;
        Ok(dep)
    }

    /// Find all components that depend on a specific package:component
    pub fn find_reverse_deps(
        conn: &Connection,
        package: &str,
        component: &str,
    ) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, component_id, depends_on_component, depends_on_package, dependency_type, version_constraint
             FROM component_dependencies
             WHERE depends_on_package = ?1 AND depends_on_component = ?2",
        )?;

        let deps = stmt
            .query_map(params![package, component], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(deps)
    }

    /// Delete a dependency by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM component_dependencies WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete all dependencies for a component
    pub fn delete_by_component(conn: &Connection, component_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM component_dependencies WHERE component_id = ?1",
            [component_id],
        )?;
        Ok(())
    }

    /// Convert a database row to a ComponentDependency
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let dep_type_str: String = row.get(4)?;
        let dependency_type =
            ComponentDepType::parse(&dep_type_str).unwrap_or(ComponentDepType::Runtime);

        Ok(Self {
            id: Some(row.get(0)?),
            component_id: row.get(1)?,
            depends_on_component: row.get(2)?,
            depends_on_package: row.get(3)?,
            dependency_type,
            version_constraint: row.get(5)?,
        })
    }
}

/// ComponentProvide represents a capability provided by a component
#[derive(Debug, Clone)]
pub struct ComponentProvide {
    pub id: Option<i64>,
    pub component_id: i64,
    pub capability: String,
    pub version: Option<String>,
}

impl ComponentProvide {
    /// Create a new ComponentProvide
    pub fn new(component_id: i64, capability: String) -> Self {
        Self {
            id: None,
            component_id,
            capability,
            version: None,
        }
    }

    /// Insert this provide into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO component_provides (component_id, capability, version)
             VALUES (?1, ?2, ?3)",
            params![&self.component_id, &self.capability, &self.version,],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all provides for a component
    pub fn find_by_component(conn: &Connection, component_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, component_id, capability, version
             FROM component_provides WHERE component_id = ?1",
        )?;

        let provides = stmt
            .query_map([component_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Find components that provide a capability
    pub fn find_by_capability(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, component_id, capability, version
             FROM component_provides WHERE capability = ?1",
        )?;

        let provides = stmt
            .query_map([capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(provides)
    }

    /// Delete a provide by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM component_provides WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete all provides for a component
    pub fn delete_by_component(conn: &Connection, component_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM component_provides WHERE component_id = ?1",
            [component_id],
        )?;
        Ok(())
    }

    /// Convert a database row to a ComponentProvide
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            component_id: row.get(1)?,
            capability: row.get(2)?,
            version: row.get(3)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{Component, Trove, TroveType};
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    fn create_test_trove_and_component(conn: &Connection, name: &str, comp_name: &str) -> (i64, i64) {
        let mut trove = Trove::new(name.to_string(), "1.0.0".to_string(), TroveType::Package);
        let trove_id = trove.insert(conn).unwrap();

        let mut comp = Component::new(trove_id, comp_name.to_string());
        let comp_id = comp.insert(conn).unwrap();

        (trove_id, comp_id)
    }

    #[test]
    fn test_dep_type_conversion() {
        assert_eq!(ComponentDepType::Runtime.as_str(), "runtime");
        assert_eq!(ComponentDepType::Build.as_str(), "build");
        assert_eq!(ComponentDepType::Optional.as_str(), "optional");

        assert_eq!(ComponentDepType::parse("runtime"), Some(ComponentDepType::Runtime));
        assert_eq!(ComponentDepType::parse("build"), Some(ComponentDepType::Build));
        assert_eq!(ComponentDepType::parse("optional"), Some(ComponentDepType::Optional));
        assert_eq!(ComponentDepType::parse("invalid"), None);
    }

    #[test]
    fn test_dependency_same_package() {
        let (_temp, conn) = create_test_db();
        let (_, comp_id) = create_test_trove_and_component(&conn, "nginx", "devel");

        let mut dep = ComponentDependency::new_same_package(
            comp_id,
            "lib".to_string(),
            ComponentDepType::Runtime,
        );
        let id = dep.insert(&conn).unwrap();
        assert!(id > 0);

        let found = ComponentDependency::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.depends_on_component, "lib");
        assert!(found.depends_on_package.is_none());
        assert_eq!(found.dependency_type, ComponentDepType::Runtime);
    }

    #[test]
    fn test_dependency_cross_package() {
        let (_temp, conn) = create_test_db();
        let (_, comp_id) = create_test_trove_and_component(&conn, "nginx", "runtime");

        let mut dep = ComponentDependency::new_cross_package(
            comp_id,
            "lib".to_string(),
            "openssl".to_string(),
            ComponentDepType::Runtime,
        );
        dep.version_constraint = Some(">=3.0.0".to_string());
        let id = dep.insert(&conn).unwrap();

        let found = ComponentDependency::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.depends_on_component, "lib");
        assert_eq!(found.depends_on_package, Some("openssl".to_string()));
        assert_eq!(found.version_constraint, Some(">=3.0.0".to_string()));
    }

    #[test]
    fn test_find_dependencies_by_component() {
        let (_temp, conn) = create_test_db();
        let (_, comp_id) = create_test_trove_and_component(&conn, "myapp", "runtime");

        // Add multiple dependencies
        let mut dep1 = ComponentDependency::new_same_package(
            comp_id,
            "lib".to_string(),
            ComponentDepType::Runtime,
        );
        dep1.insert(&conn).unwrap();

        let mut dep2 = ComponentDependency::new_cross_package(
            comp_id,
            "lib".to_string(),
            "openssl".to_string(),
            ComponentDepType::Runtime,
        );
        dep2.insert(&conn).unwrap();

        let deps = ComponentDependency::find_by_component(&conn, comp_id).unwrap();
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_reverse_deps() {
        let (_temp, conn) = create_test_db();

        // Create nginx:runtime depending on openssl:lib
        let (_, nginx_comp_id) = create_test_trove_and_component(&conn, "nginx", "runtime");
        let mut dep = ComponentDependency::new_cross_package(
            nginx_comp_id,
            "lib".to_string(),
            "openssl".to_string(),
            ComponentDepType::Runtime,
        );
        dep.insert(&conn).unwrap();

        // Find what depends on openssl:lib
        let reverse_deps = ComponentDependency::find_reverse_deps(&conn, "openssl", "lib").unwrap();
        assert_eq!(reverse_deps.len(), 1);
        assert_eq!(reverse_deps[0].component_id, nginx_comp_id);
    }

    #[test]
    fn test_component_provide_crud() {
        let (_temp, conn) = create_test_db();
        let (_, comp_id) = create_test_trove_and_component(&conn, "openssl", "lib");

        // Create provides
        let mut provide = ComponentProvide::new(comp_id, "libssl.so.3".to_string());
        provide.version = Some("3.0.0".to_string());
        let id = provide.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by component
        let provides = ComponentProvide::find_by_component(&conn, comp_id).unwrap();
        assert_eq!(provides.len(), 1);
        assert_eq!(provides[0].capability, "libssl.so.3");

        // Find by capability
        let by_cap = ComponentProvide::find_by_capability(&conn, "libssl.so.3").unwrap();
        assert_eq!(by_cap.len(), 1);
        assert_eq!(by_cap[0].component_id, comp_id);

        // Delete
        ComponentProvide::delete(&conn, id).unwrap();
        let provides = ComponentProvide::find_by_component(&conn, comp_id).unwrap();
        assert_eq!(provides.len(), 0);
    }

    #[test]
    fn test_cascade_delete_on_component() {
        let (_temp, conn) = create_test_db();
        let (_, comp_id) = create_test_trove_and_component(&conn, "testpkg", "runtime");

        // Add dependency and provide
        let mut dep = ComponentDependency::new_same_package(
            comp_id,
            "lib".to_string(),
            ComponentDepType::Runtime,
        );
        dep.insert(&conn).unwrap();

        let mut provide = ComponentProvide::new(comp_id, "testcap".to_string());
        provide.insert(&conn).unwrap();

        // Delete the component
        Component::delete(&conn, comp_id).unwrap();

        // Dependencies and provides should be cascade deleted
        let deps = ComponentDependency::find_by_component(&conn, comp_id).unwrap();
        assert_eq!(deps.len(), 0);

        let provides = ComponentProvide::find_by_component(&conn, comp_id).unwrap();
        assert_eq!(provides.len(), 0);
    }
}
