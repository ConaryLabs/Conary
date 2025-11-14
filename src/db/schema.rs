// src/db/schema.rs

//! Database schema definitions and migrations for Conary
//!
//! This module defines the SQLite schema for all core tables and provides
//! a migration system to evolve the schema over time.

use crate::error::Result;
use rusqlite::Connection;
use tracing::{debug, info};

/// Current schema version
pub const SCHEMA_VERSION: i32 = 1;

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
        1 => migrate_v1(conn),
        _ => panic!("Unknown migration version: {}", version),
    }
}

/// Initial schema - Version 1
///
/// Creates all core tables for Conary:
/// - troves: Package/component/collection metadata
/// - changesets: Transactional operation history
/// - files: File-level tracking with hashes
/// - flavors: Build-time variations
/// - provenance: Supply chain tracking
/// - dependencies: Trove relationships
fn migrate_v1(conn: &Connection) -> Result<()> {
    debug!("Creating schema version 1");

    conn.execute_batch(
        "
        -- Troves: The core unit (package, component, or collection)
        CREATE TABLE troves (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            type TEXT NOT NULL CHECK(type IN ('package', 'component', 'collection')),
            architecture TEXT,
            description TEXT,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            installed_by_changeset_id INTEGER,
            UNIQUE(name, version, architecture),
            FOREIGN KEY (installed_by_changeset_id) REFERENCES changesets(id)
        );

        CREATE INDEX idx_troves_name ON troves(name);
        CREATE INDEX idx_troves_type ON troves(type);

        -- Changesets: Atomic transactional operations
        CREATE TABLE changesets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            description TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('pending', 'applied', 'rolled_back')),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            applied_at TEXT,
            rolled_back_at TEXT
        );

        CREATE INDEX idx_changesets_status ON changesets(status);
        CREATE INDEX idx_changesets_created_at ON changesets(created_at);

        -- Files: File-level tracking with content hashing
        CREATE TABLE files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE,
            sha256_hash TEXT NOT NULL,
            size INTEGER NOT NULL,
            permissions INTEGER NOT NULL,
            owner TEXT,
            group_name TEXT,
            trove_id INTEGER NOT NULL,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_files_path ON files(path);
        CREATE INDEX idx_files_trove_id ON files(trove_id);
        CREATE INDEX idx_files_sha256 ON files(sha256_hash);

        -- Flavors: Build-time variations (arch, features, toolchain, etc.)
        CREATE TABLE flavors (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            UNIQUE(trove_id, key),
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_flavors_trove_id ON flavors(trove_id);
        CREATE INDEX idx_flavors_key ON flavors(key);

        -- Provenance: Supply chain tracking
        CREATE TABLE provenance (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL UNIQUE,
            source_url TEXT,
            source_branch TEXT,
            source_commit TEXT,
            build_host TEXT,
            build_time TEXT,
            builder TEXT,
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_provenance_trove_id ON provenance(trove_id);

        -- Dependencies: Relationships between troves
        CREATE TABLE dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL,
            depends_on_name TEXT NOT NULL,
            depends_on_version TEXT,
            dependency_type TEXT NOT NULL CHECK(dependency_type IN ('runtime', 'build', 'optional')),
            version_constraint TEXT,
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_dependencies_trove_id ON dependencies(trove_id);
        CREATE INDEX idx_dependencies_depends_on ON dependencies(depends_on_name);
        ",
    )?;

    info!("Schema version 1 created successfully");
    Ok(())
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
}
