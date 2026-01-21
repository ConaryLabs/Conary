// src/db/models/subpackage.rs

//! Subpackage relationship model
//!
//! Tracks relationships between base packages and their subpackages
//! (e.g., nginx-devel is a subpackage of nginx). This enables:
//! - Component-level dependencies (subpackage depends on base)
//! - Virtual provides (subpackage provides base:component_type)
//! - User guidance when installing related packages

use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};

/// A subpackage relationship record
#[derive(Debug, Clone)]
pub struct SubpackageRelationship {
    pub id: Option<i64>,
    /// Base package name (without suffix like -devel, -doc)
    pub base_package: String,
    /// Full subpackage name (e.g., nginx-devel)
    pub subpackage_name: String,
    /// Component type this subpackage represents
    /// Common types: devel, doc, debuginfo, libs, common, data, lang
    pub component_type: String,
    /// Source format where this relationship was detected (rpm, deb, arch)
    pub source_format: String,
    /// When this relationship was recorded
    pub created_at: Option<String>,
}

impl SubpackageRelationship {
    /// Create a new subpackage relationship
    pub fn new(
        base_package: String,
        subpackage_name: String,
        component_type: String,
        source_format: String,
    ) -> Self {
        Self {
            id: None,
            base_package,
            subpackage_name,
            component_type,
            source_format,
            created_at: None,
        }
    }

    /// Create from a database row
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            base_package: row.get(1)?,
            subpackage_name: row.get(2)?,
            component_type: row.get(3)?,
            source_format: row.get(4)?,
            created_at: row.get(5)?,
        })
    }

    /// Insert this relationship into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT OR IGNORE INTO subpackage_relationships
             (base_package, subpackage_name, component_type, source_format)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                &self.base_package,
                &self.subpackage_name,
                &self.component_type,
                &self.source_format,
            ],
        )?;

        // Get the ID (either from insert or existing row)
        let id = conn.query_row(
            "SELECT id FROM subpackage_relationships
             WHERE base_package = ?1 AND subpackage_name = ?2",
            params![&self.base_package, &self.subpackage_name],
            |row| row.get(0),
        )?;

        self.id = Some(id);
        Ok(id)
    }

    /// Find all subpackages of a base package
    pub fn find_by_base(conn: &Connection, base_package: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, base_package, subpackage_name, component_type, source_format, created_at
             FROM subpackage_relationships
             WHERE base_package = ?1
             ORDER BY component_type",
        )?;

        let results = stmt
            .query_map([base_package], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Find the base package for a subpackage
    pub fn find_by_subpackage(conn: &Connection, subpackage_name: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, base_package, subpackage_name, component_type, source_format, created_at
                 FROM subpackage_relationships
                 WHERE subpackage_name = ?1",
                [subpackage_name],
                Self::from_row,
            )
            .optional()?;

        Ok(result)
    }

    /// Check if a package is a subpackage
    pub fn is_subpackage(conn: &Connection, package_name: &str) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM subpackage_relationships WHERE subpackage_name = ?1",
            [package_name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Check if a package has subpackages
    pub fn has_subpackages(conn: &Connection, package_name: &str) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM subpackage_relationships WHERE base_package = ?1",
            [package_name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Find subpackages by component type
    pub fn find_by_component_type(conn: &Connection, component_type: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, base_package, subpackage_name, component_type, source_format, created_at
             FROM subpackage_relationships
             WHERE component_type = ?1
             ORDER BY base_package",
        )?;

        let results = stmt
            .query_map([component_type], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all unique base packages that have subpackages
    pub fn list_base_packages(conn: &Connection) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT base_package FROM subpackage_relationships ORDER BY base_package",
        )?;

        let results = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all relationships for display
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, base_package, subpackage_name, component_type, source_format, created_at
             FROM subpackage_relationships
             ORDER BY base_package, component_type",
        )?;

        let results = stmt
            .query_map([], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Delete a relationship
    pub fn delete(&self, conn: &Connection) -> Result<()> {
        if let Some(id) = self.id {
            conn.execute("DELETE FROM subpackage_relationships WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    /// Get the virtual provide name for this subpackage
    /// e.g., nginx-devel provides "nginx:devel"
    pub fn virtual_provide(&self) -> String {
        format!("{}:{}", self.base_package, self.component_type)
    }

    /// Check if a package name matches the virtual provide pattern
    /// e.g., "nginx:devel" matches base="nginx", component="devel"
    pub fn parse_virtual_provide(name: &str) -> Option<(String, String)> {
        if let Some((base, component)) = name.split_once(':') {
            if !base.is_empty() && !component.is_empty() {
                return Some((base.to_string(), component.to_string()));
            }
        }
        None
    }

    /// Find a subpackage by virtual provide name
    /// e.g., "nginx:devel" -> find nginx-devel
    pub fn find_by_virtual_provide(
        conn: &Connection,
        base_package: &str,
        component_type: &str,
    ) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, base_package, subpackage_name, component_type, source_format, created_at
                 FROM subpackage_relationships
                 WHERE base_package = ?1 AND component_type = ?2",
                params![base_package, component_type],
                Self::from_row,
            )
            .optional()?;

        Ok(result)
    }

    /// Get related packages for user guidance
    ///
    /// Returns (base_package, subpackages) if this is a base package with subpackages,
    /// or (base_package, siblings) if this is a subpackage.
    pub fn get_related_packages(
        conn: &Connection,
        package_name: &str,
    ) -> Result<RelatedPackages> {
        // Check if this is a subpackage
        if let Some(rel) = Self::find_by_subpackage(conn, package_name)? {
            // This is a subpackage - find siblings and base
            let siblings = Self::find_by_base(conn, &rel.base_package)?
                .into_iter()
                .filter(|r| r.subpackage_name != package_name)
                .collect();

            return Ok(RelatedPackages::Subpackage {
                base_package: rel.base_package,
                component_type: rel.component_type,
                siblings,
            });
        }

        // Check if this is a base package with subpackages
        let subpackages = Self::find_by_base(conn, package_name)?;
        if !subpackages.is_empty() {
            return Ok(RelatedPackages::BasePackage { subpackages });
        }

        // No related packages
        Ok(RelatedPackages::None)
    }
}

/// Related packages for user guidance
#[derive(Debug)]
pub enum RelatedPackages {
    /// This is a base package with subpackages
    BasePackage {
        subpackages: Vec<SubpackageRelationship>,
    },
    /// This is a subpackage
    Subpackage {
        base_package: String,
        component_type: String,
        siblings: Vec<SubpackageRelationship>,
    },
    /// No related packages found
    None,
}

impl RelatedPackages {
    /// Check if there are any related packages
    pub fn has_related(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// Get a human-readable summary
    pub fn summary(&self) -> Option<String> {
        match self {
            Self::BasePackage { subpackages } => {
                if subpackages.is_empty() {
                    None
                } else {
                    let types: Vec<_> = subpackages.iter().map(|s| s.component_type.as_str()).collect();
                    Some(format!(
                        "Available subpackages: {}",
                        types.join(", ")
                    ))
                }
            }
            Self::Subpackage {
                base_package,
                component_type,
                siblings,
            } => {
                let mut msg = format!(
                    "This is the {} component of '{}'",
                    component_type, base_package
                );
                if !siblings.is_empty() {
                    let sibling_types: Vec<_> = siblings.iter().map(|s| s.component_type.as_str()).collect();
                    msg.push_str(&format!(". Other components: {}", sibling_types.join(", ")));
                }
                Some(msg)
            }
            Self::None => None,
        }
    }
}

/// Display user guidance about related packages after installation
///
/// This function checks for related packages and prints helpful guidance.
/// Call this after a package has been successfully installed.
pub fn show_subpackage_guidance(conn: &Connection, package_name: &str) {
    match SubpackageRelationship::get_related_packages(conn, package_name) {
        Ok(related) => {
            if let Some(summary) = related.summary() {
                println!();
                println!("Hint: {}", summary);
                match &related {
                    RelatedPackages::BasePackage { subpackages } => {
                        // Show install commands for subpackages
                        for subpkg in subpackages.iter().take(3) {
                            println!(
                                "  Install {}: conary install {}",
                                subpkg.component_type, subpkg.subpackage_name
                            );
                        }
                        if subpackages.len() > 3 {
                            println!("  ... and {} more", subpackages.len() - 3);
                        }
                    }
                    RelatedPackages::Subpackage { base_package, .. } => {
                        // Check if base is installed
                        let base_installed: bool = conn
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM troves WHERE name = ?1)",
                                [base_package],
                                |row| row.get(0),
                            )
                            .unwrap_or(false);

                        if !base_installed {
                            println!(
                                "  Note: Base package '{}' should be installed for full functionality",
                                base_package
                            );
                        }
                    }
                    RelatedPackages::None => {}
                }
            }
        }
        Err(e) => {
            // Non-fatal - just log and continue
            tracing::debug!("Could not check related packages: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_subpackage_crud() {
        let (_temp, conn) = create_test_db();

        // Create a subpackage relationship
        let mut rel = SubpackageRelationship::new(
            "nginx".to_string(),
            "nginx-devel".to_string(),
            "devel".to_string(),
            "rpm".to_string(),
        );
        let id = rel.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by base
        let found = SubpackageRelationship::find_by_base(&conn, "nginx").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].subpackage_name, "nginx-devel");

        // Find by subpackage
        let found = SubpackageRelationship::find_by_subpackage(&conn, "nginx-devel")
            .unwrap()
            .unwrap();
        assert_eq!(found.base_package, "nginx");
        assert_eq!(found.component_type, "devel");

        // Check is_subpackage
        assert!(SubpackageRelationship::is_subpackage(&conn, "nginx-devel").unwrap());
        assert!(!SubpackageRelationship::is_subpackage(&conn, "nginx").unwrap());

        // Check has_subpackages
        assert!(SubpackageRelationship::has_subpackages(&conn, "nginx").unwrap());
        assert!(!SubpackageRelationship::has_subpackages(&conn, "nginx-devel").unwrap());
    }

    #[test]
    fn test_virtual_provide() {
        let rel = SubpackageRelationship::new(
            "nginx".to_string(),
            "nginx-devel".to_string(),
            "devel".to_string(),
            "rpm".to_string(),
        );

        assert_eq!(rel.virtual_provide(), "nginx:devel");
    }

    #[test]
    fn test_parse_virtual_provide() {
        let (base, component) = SubpackageRelationship::parse_virtual_provide("nginx:devel").unwrap();
        assert_eq!(base, "nginx");
        assert_eq!(component, "devel");

        assert!(SubpackageRelationship::parse_virtual_provide("nginx").is_none());
        assert!(SubpackageRelationship::parse_virtual_provide(":devel").is_none());
        assert!(SubpackageRelationship::parse_virtual_provide("nginx:").is_none());
    }

    #[test]
    fn test_find_by_virtual_provide() {
        let (_temp, conn) = create_test_db();

        let mut rel = SubpackageRelationship::new(
            "nginx".to_string(),
            "nginx-devel".to_string(),
            "devel".to_string(),
            "rpm".to_string(),
        );
        rel.insert(&conn).unwrap();

        let found = SubpackageRelationship::find_by_virtual_provide(&conn, "nginx", "devel")
            .unwrap()
            .unwrap();
        assert_eq!(found.subpackage_name, "nginx-devel");
    }

    #[test]
    fn test_related_packages_base() {
        let (_temp, conn) = create_test_db();

        // Create multiple subpackages
        let mut rel1 = SubpackageRelationship::new(
            "nginx".to_string(),
            "nginx-devel".to_string(),
            "devel".to_string(),
            "rpm".to_string(),
        );
        rel1.insert(&conn).unwrap();

        let mut rel2 = SubpackageRelationship::new(
            "nginx".to_string(),
            "nginx-doc".to_string(),
            "doc".to_string(),
            "rpm".to_string(),
        );
        rel2.insert(&conn).unwrap();

        // Check related for base package
        let related = SubpackageRelationship::get_related_packages(&conn, "nginx").unwrap();
        match related {
            RelatedPackages::BasePackage { subpackages } => {
                assert_eq!(subpackages.len(), 2);
            }
            _ => panic!("Expected BasePackage"),
        }
    }

    #[test]
    fn test_related_packages_subpackage() {
        let (_temp, conn) = create_test_db();

        // Create multiple subpackages
        let mut rel1 = SubpackageRelationship::new(
            "nginx".to_string(),
            "nginx-devel".to_string(),
            "devel".to_string(),
            "rpm".to_string(),
        );
        rel1.insert(&conn).unwrap();

        let mut rel2 = SubpackageRelationship::new(
            "nginx".to_string(),
            "nginx-doc".to_string(),
            "doc".to_string(),
            "rpm".to_string(),
        );
        rel2.insert(&conn).unwrap();

        // Check related for subpackage
        let related = SubpackageRelationship::get_related_packages(&conn, "nginx-devel").unwrap();
        match related {
            RelatedPackages::Subpackage {
                base_package,
                component_type,
                siblings,
            } => {
                assert_eq!(base_package, "nginx");
                assert_eq!(component_type, "devel");
                assert_eq!(siblings.len(), 1);
                assert_eq!(siblings[0].subpackage_name, "nginx-doc");
            }
            _ => panic!("Expected Subpackage"),
        }
    }

    #[test]
    fn test_related_packages_summary() {
        let related = RelatedPackages::BasePackage {
            subpackages: vec![
                SubpackageRelationship::new(
                    "nginx".to_string(),
                    "nginx-devel".to_string(),
                    "devel".to_string(),
                    "rpm".to_string(),
                ),
                SubpackageRelationship::new(
                    "nginx".to_string(),
                    "nginx-doc".to_string(),
                    "doc".to_string(),
                    "rpm".to_string(),
                ),
            ],
        };

        let summary = related.summary().unwrap();
        assert!(summary.contains("devel"));
        assert!(summary.contains("doc"));
    }

    #[test]
    fn test_list_base_packages() {
        let (_temp, conn) = create_test_db();

        let mut rel1 = SubpackageRelationship::new(
            "nginx".to_string(),
            "nginx-devel".to_string(),
            "devel".to_string(),
            "rpm".to_string(),
        );
        rel1.insert(&conn).unwrap();

        let mut rel2 = SubpackageRelationship::new(
            "openssl".to_string(),
            "openssl-devel".to_string(),
            "devel".to_string(),
            "rpm".to_string(),
        );
        rel2.insert(&conn).unwrap();

        let bases = SubpackageRelationship::list_base_packages(&conn).unwrap();
        assert_eq!(bases.len(), 2);
        assert!(bases.contains(&"nginx".to_string()));
        assert!(bases.contains(&"openssl".to_string()));
    }
}
