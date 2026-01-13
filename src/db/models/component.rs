// src/db/models/component.rs

//! Component model - independently installable units within packages
//!
//! Components represent the different parts of a package that can be
//! installed independently. For example, a package like `nginx` may have:
//! - `:runtime` - Executables and main program files
//! - `:lib` - Shared libraries
//! - `:devel` - Headers, static libs, pkg-config files
//! - `:doc` - Documentation and man pages
//! - `:config` - Configuration files

use crate::components::ComponentType;
use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};

/// A Component represents an installable unit within a package
#[derive(Debug, Clone)]
pub struct Component {
    pub id: Option<i64>,
    pub parent_trove_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub installed_at: Option<String>,
    pub is_installed: bool,
}

impl Component {
    /// Create a new Component
    pub fn new(parent_trove_id: i64, name: String) -> Self {
        Self {
            id: None,
            parent_trove_id,
            name,
            description: None,
            installed_at: None,
            is_installed: true,
        }
    }

    /// Create a new Component from a ComponentType
    pub fn from_type(parent_trove_id: i64, component_type: ComponentType) -> Self {
        Self::new(parent_trove_id, component_type.as_str().to_string())
    }

    /// Get the ComponentType for this component
    pub fn component_type(&self) -> Option<ComponentType> {
        ComponentType::parse(&self.name)
    }

    /// Check if this component is a default component (installed by default)
    pub fn is_default(&self) -> bool {
        self.component_type()
            .map(|ct| ct.is_default())
            .unwrap_or(false)
    }

    /// Insert this component into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO components (parent_trove_id, name, description, is_installed)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                &self.parent_trove_id,
                &self.name,
                &self.description,
                &self.is_installed,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a component by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, parent_trove_id, name, description, installed_at, is_installed
             FROM components WHERE id = ?1",
        )?;

        let component = stmt.query_row([id], Self::from_row).optional()?;
        Ok(component)
    }

    /// Find all components for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, parent_trove_id, name, description, installed_at, is_installed
             FROM components WHERE parent_trove_id = ?1 ORDER BY name",
        )?;

        let components = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(components)
    }

    /// Find a specific component by trove and name
    pub fn find_by_trove_and_name(
        conn: &Connection,
        trove_id: i64,
        name: &str,
    ) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, parent_trove_id, name, description, installed_at, is_installed
             FROM components WHERE parent_trove_id = ?1 AND name = ?2",
        )?;

        let component = stmt
            .query_row(params![trove_id, name], Self::from_row)
            .optional()?;

        Ok(component)
    }

    /// Find all installed components for a trove
    pub fn find_installed_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, parent_trove_id, name, description, installed_at, is_installed
             FROM components WHERE parent_trove_id = ?1 AND is_installed = 1 ORDER BY name",
        )?;

        let components = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(components)
    }

    /// Update the installed status of a component
    pub fn set_installed(&mut self, conn: &Connection, installed: bool) -> Result<()> {
        if let Some(id) = self.id {
            conn.execute(
                "UPDATE components SET is_installed = ?1 WHERE id = ?2",
                params![installed, id],
            )?;
            self.is_installed = installed;
        }
        Ok(())
    }

    /// Delete a component by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM components WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete all components for a trove
    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM components WHERE parent_trove_id = ?1",
            [trove_id],
        )?;
        Ok(())
    }

    /// Convert a database row to a Component
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            parent_trove_id: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
            installed_at: row.get(4)?,
            is_installed: row.get(5)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::Trove;
    use crate::db::models::TroveType;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    fn create_test_trove(conn: &Connection) -> i64 {
        let mut trove = Trove::new(
            "nginx".to_string(),
            "1.24.0".to_string(),
            TroveType::Package,
        );
        trove.insert(conn).unwrap()
    }

    #[test]
    fn test_component_new() {
        let comp = Component::new(1, "runtime".to_string());
        assert_eq!(comp.parent_trove_id, 1);
        assert_eq!(comp.name, "runtime");
        assert!(comp.is_installed);
        assert!(comp.id.is_none());
    }

    #[test]
    fn test_component_from_type() {
        let comp = Component::from_type(1, ComponentType::Lib);
        assert_eq!(comp.name, "lib");
    }

    #[test]
    fn test_component_type_conversion() {
        let comp = Component::new(1, "devel".to_string());
        assert_eq!(comp.component_type(), Some(ComponentType::Devel));

        let unknown = Component::new(1, "unknown".to_string());
        assert_eq!(unknown.component_type(), None);
    }

    #[test]
    fn test_component_is_default() {
        let runtime = Component::new(1, "runtime".to_string());
        assert!(runtime.is_default());

        let lib = Component::new(1, "lib".to_string());
        assert!(lib.is_default());

        let config = Component::new(1, "config".to_string());
        assert!(config.is_default());

        let devel = Component::new(1, "devel".to_string());
        assert!(!devel.is_default());

        let doc = Component::new(1, "doc".to_string());
        assert!(!doc.is_default());
    }

    #[test]
    fn test_component_crud() {
        let (_temp, conn) = create_test_db();
        let trove_id = create_test_trove(&conn);

        // Create
        let mut comp = Component::new(trove_id, "runtime".to_string());
        comp.description = Some("Executable files".to_string());
        let id = comp.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by ID
        let found = Component::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.name, "runtime");
        assert_eq!(found.description, Some("Executable files".to_string()));

        // Find by trove
        let components = Component::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(components.len(), 1);

        // Find by trove and name
        let specific = Component::find_by_trove_and_name(&conn, trove_id, "runtime")
            .unwrap()
            .unwrap();
        assert_eq!(specific.id, Some(id));

        // Delete
        Component::delete(&conn, id).unwrap();
        let deleted = Component::find_by_id(&conn, id).unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_component_installed_status() {
        let (_temp, conn) = create_test_db();
        let trove_id = create_test_trove(&conn);

        let mut comp = Component::new(trove_id, "devel".to_string());
        comp.insert(&conn).unwrap();

        // Initially installed
        assert!(comp.is_installed);

        // Mark as not installed
        comp.set_installed(&conn, false).unwrap();
        let reloaded = Component::find_by_id(&conn, comp.id.unwrap())
            .unwrap()
            .unwrap();
        assert!(!reloaded.is_installed);

        // Find installed only
        let installed = Component::find_installed_by_trove(&conn, trove_id).unwrap();
        assert_eq!(installed.len(), 0);

        // Mark as installed again
        let mut comp2 = reloaded;
        comp2.set_installed(&conn, true).unwrap();
        let installed = Component::find_installed_by_trove(&conn, trove_id).unwrap();
        assert_eq!(installed.len(), 1);
    }

    #[test]
    fn test_component_multiple_per_trove() {
        let (_temp, conn) = create_test_db();
        let trove_id = create_test_trove(&conn);

        // Create multiple components
        for name in &["runtime", "lib", "devel", "doc", "config"] {
            let mut comp = Component::new(trove_id, name.to_string());
            comp.insert(&conn).unwrap();
        }

        let components = Component::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(components.len(), 5);

        // Verify they're ordered by name
        assert_eq!(components[0].name, "config");
        assert_eq!(components[1].name, "devel");
        assert_eq!(components[2].name, "doc");
        assert_eq!(components[3].name, "lib");
        assert_eq!(components[4].name, "runtime");
    }

    #[test]
    fn test_component_delete_by_trove() {
        let (_temp, conn) = create_test_db();
        let trove_id = create_test_trove(&conn);

        let mut comp1 = Component::new(trove_id, "runtime".to_string());
        comp1.insert(&conn).unwrap();
        let mut comp2 = Component::new(trove_id, "lib".to_string());
        comp2.insert(&conn).unwrap();

        Component::delete_by_trove(&conn, trove_id).unwrap();

        let components = Component::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(components.len(), 0);
    }
}
