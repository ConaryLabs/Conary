// src/db/schema.rs

//! Database schema definitions and migrations for Conary
//!
//! This module defines the SQLite schema for all core tables and provides
//! a migration system to evolve the schema over time.

use super::migrations;
use crate::error::Result;
use rusqlite::Connection;
use tracing::info;

/// Current schema version
pub const SCHEMA_VERSION: i32 = 28;

/// Initialize the schema version tracking table
fn init_schema_version(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;
    Ok(())
}

/// Get the current schema version from the database
pub fn get_schema_version(conn: &Connection) -> Result<i32> {
    init_schema_version(conn)?;

    let version = conn
        .query_row(
            "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    Ok(version)
}

/// Set the schema version
fn set_schema_version(conn: &Connection, version: i32) -> Result<()> {
    conn.execute(
        "INSERT INTO schema_version (version) VALUES (?1)",
        [version],
    )?;
    Ok(())
}

/// Apply all pending migrations to bring the database up to date
pub fn migrate(conn: &Connection) -> Result<()> {
    let current_version = get_schema_version(conn)?;
    info!("Current schema version: {}", current_version);

    if current_version >= SCHEMA_VERSION {
        info!("Schema is up to date");
        return Ok(());
    }

    // Apply migrations in order
    for version in (current_version + 1)..=SCHEMA_VERSION {
        info!("Applying migration to version {}", version);
        apply_migration(conn, version)?;
        set_schema_version(conn, version)?;
    }

    info!(
        "Schema migration complete. Now at version {}",
        SCHEMA_VERSION
    );
    Ok(())
}

/// Apply a specific migration version
fn apply_migration(conn: &Connection, version: i32) -> Result<()> {
    match version {
        1 => migrations::migrate_v1(conn),
        2 => migrations::migrate_v2(conn),
        3 => migrations::migrate_v3(conn),
        4 => migrations::migrate_v4(conn),
        5 => migrations::migrate_v5(conn),
        6 => migrations::migrate_v6(conn),
        7 => migrations::migrate_v7(conn),
        8 => migrations::migrate_v8(conn),
        9 => migrations::migrate_v9(conn),
        10 => migrations::migrate_v10(conn),
        11 => migrations::migrate_v11(conn),
        12 => migrations::migrate_v12(conn),
        13 => migrations::migrate_v13(conn),
        14 => migrations::migrate_v14(conn),
        15 => migrations::migrate_v15(conn),
        16 => migrations::migrate_v16(conn),
        17 => migrations::migrate_v17(conn),
        18 => migrations::migrate_v18(conn),
        19 => migrations::migrate_v19(conn),
        20 => migrations::migrate_v20(conn),
        21 => migrations::migrate_v21(conn),
        22 => migrations::migrate_v22(conn),
        23 => migrations::migrate_v23(conn),
        24 => migrations::migrate_v24(conn),
        25 => migrations::migrate_v25(conn),
        26 => migrations::migrate_v26(conn),
        27 => migrations::migrate_v27(conn),
        28 => migrations::migrate_v28(conn),
        _ => panic!("Unknown migration version: {}", version),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_schema_version_tracking() {
        let (_temp, conn) = create_test_db();

        // Initial version should be 0
        let version = get_schema_version(&conn).unwrap();
        assert_eq!(version, 0);

        // Set version to 1
        set_schema_version(&conn, 1).unwrap();
        let version = get_schema_version(&conn).unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn test_migrate_creates_all_tables() {
        let (_temp, conn) = create_test_db();

        // Run migration
        migrate(&conn).unwrap();

        // Verify all tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"troves".to_string()));
        assert!(tables.contains(&"changesets".to_string()));
        assert!(tables.contains(&"files".to_string()));
        assert!(tables.contains(&"flavors".to_string()));
        assert!(tables.contains(&"provenance".to_string()));
        assert!(tables.contains(&"dependencies".to_string()));
        assert!(tables.contains(&"schema_version".to_string()));
    }

    #[test]
    fn test_migrate_is_idempotent() {
        let (_temp, conn) = create_test_db();

        // Run migration twice
        migrate(&conn).unwrap();
        let version1 = get_schema_version(&conn).unwrap();

        migrate(&conn).unwrap();
        let version2 = get_schema_version(&conn).unwrap();

        assert_eq!(version1, version2);
        assert_eq!(version1, SCHEMA_VERSION);
    }

    #[test]
    fn test_troves_table_constraints() {
        let (_temp, conn) = create_test_db();
        migrate(&conn).unwrap();

        // Insert a valid trove
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture) VALUES (?1, ?2, ?3, ?4)",
            ["test-package", "1.0.0", "package", "x86_64"],
        )
        .unwrap();

        // Try to insert duplicate - should fail due to UNIQUE constraint
        let result = conn.execute(
            "INSERT INTO troves (name, version, type, architecture) VALUES (?1, ?2, ?3, ?4)",
            ["test-package", "1.0.0", "package", "x86_64"],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_foreign_key_constraints() {
        let (_temp, conn) = create_test_db();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        migrate(&conn).unwrap();

        // Try to insert a file without a trove - should fail
        let result = conn.execute(
            "INSERT INTO files (path, sha256_hash, size, permissions, trove_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            [
                "/usr/bin/test",
                "abc123",
                "1024",
                "755",
                "999", // Non-existent trove_id
            ],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_v11_creates_component_tables() {
        let (_temp, conn) = create_test_db();
        migrate(&conn).unwrap();

        // Verify component tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"components".to_string()));
        assert!(tables.contains(&"component_dependencies".to_string()));
        assert!(tables.contains(&"component_provides".to_string()));
    }

    #[test]
    fn test_v11_component_file_relationship() {
        let (_temp, conn) = create_test_db();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        migrate(&conn).unwrap();

        // Create a trove
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture) VALUES (?1, ?2, ?3, ?4)",
            ["nginx", "1.24.0", "package", "x86_64"],
        )
        .unwrap();

        let trove_id: i64 = conn
            .query_row("SELECT id FROM troves WHERE name = 'nginx'", [], |row| {
                row.get(0)
            })
            .unwrap();

        // Create a component for the trove
        conn.execute(
            "INSERT INTO components (parent_trove_id, name, description) VALUES (?1, ?2, ?3)",
            rusqlite::params![trove_id, "runtime", "Executable files"],
        )
        .unwrap();

        let component_id: i64 = conn
            .query_row(
                "SELECT id FROM components WHERE parent_trove_id = ?1 AND name = 'runtime'",
                [trove_id],
                |row| row.get(0),
            )
            .unwrap();

        // Create a file linked to the component
        conn.execute(
            "INSERT INTO files (path, sha256_hash, size, permissions, trove_id, component_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["/usr/sbin/nginx", "abc123", 1024, 755, trove_id, component_id],
        )
        .unwrap();

        // Verify the file is linked to the component
        let file_component_id: i64 = conn
            .query_row(
                "SELECT component_id FROM files WHERE path = '/usr/sbin/nginx'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(file_component_id, component_id);
    }

    #[test]
    fn test_v11_component_unique_constraint() {
        let (_temp, conn) = create_test_db();
        migrate(&conn).unwrap();

        // Create a trove
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture) VALUES (?1, ?2, ?3, ?4)",
            ["openssl", "3.0.0", "package", "x86_64"],
        )
        .unwrap();

        let trove_id: i64 = conn
            .query_row("SELECT id FROM troves WHERE name = 'openssl'", [], |row| {
                row.get(0)
            })
            .unwrap();

        // Create a component
        conn.execute(
            "INSERT INTO components (parent_trove_id, name) VALUES (?1, ?2)",
            rusqlite::params![trove_id, "lib"],
        )
        .unwrap();

        // Try to create duplicate component - should fail
        let result = conn.execute(
            "INSERT INTO components (parent_trove_id, name) VALUES (?1, ?2)",
            rusqlite::params![trove_id, "lib"],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_v11_component_cascade_delete() {
        let (_temp, conn) = create_test_db();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        migrate(&conn).unwrap();

        // Create a trove
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture) VALUES (?1, ?2, ?3, ?4)",
            ["curl", "8.0.0", "package", "x86_64"],
        )
        .unwrap();

        let trove_id: i64 = conn
            .query_row("SELECT id FROM troves WHERE name = 'curl'", [], |row| {
                row.get(0)
            })
            .unwrap();

        // Create components
        conn.execute(
            "INSERT INTO components (parent_trove_id, name) VALUES (?1, ?2)",
            rusqlite::params![trove_id, "runtime"],
        )
        .unwrap();

        // Delete the trove - components should cascade delete
        conn.execute("DELETE FROM troves WHERE id = ?1", [trove_id])
            .unwrap();

        // Verify component was deleted
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM components WHERE parent_trove_id = ?1",
                [trove_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
