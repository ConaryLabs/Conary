// src/db/schema.rs

//! Database schema definitions and migrations for Conary
//!
//! This module defines the SQLite schema for all core tables and provides
//! a migration system to evolve the schema over time.

use crate::error::Result;
use rusqlite::Connection;
use tracing::{debug, info};

/// Current schema version
pub const SCHEMA_VERSION: i32 = 9;

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
        2 => migrate_v2(conn),
        3 => migrate_v3(conn),
        4 => migrate_v4(conn),
        5 => migrate_v5(conn),
        6 => migrate_v6(conn),
        7 => migrate_v7(conn),
        8 => migrate_v8(conn),
        9 => migrate_v9(conn),
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

/// Schema Version 2: Add rollback tracking to changesets
///
/// Adds reversed_by_changeset_id to track which changeset reversed another
fn migrate_v2(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 2");

    conn.execute_batch(
        "
        ALTER TABLE changesets ADD COLUMN reversed_by_changeset_id INTEGER
            REFERENCES changesets(id) ON DELETE SET NULL;
        ",
    )?;

    info!("Schema version 2 applied successfully");
    Ok(())
}

/// Schema Version 3: Add content-addressable storage tracking
///
/// Adds tables for tracking file contents and file history:
/// - file_contents: Maps SHA-256 hashes to stored content locations
/// - file_history: Tracks file states per changeset for rollback support
fn migrate_v3(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 3");

    conn.execute_batch(
        "
        -- File contents stored in CAS (content-addressable storage)
        CREATE TABLE file_contents (
            sha256_hash TEXT PRIMARY KEY,
            content_path TEXT NOT NULL,
            size INTEGER NOT NULL,
            stored_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX idx_file_contents_stored_at ON file_contents(stored_at);

        -- File history for rollback support
        -- Tracks file states at each changeset
        CREATE TABLE file_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            changeset_id INTEGER NOT NULL,
            path TEXT NOT NULL,
            sha256_hash TEXT,
            action TEXT NOT NULL CHECK(action IN ('add', 'modify', 'delete')),
            previous_hash TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (changeset_id) REFERENCES changesets(id) ON DELETE CASCADE,
            FOREIGN KEY (sha256_hash) REFERENCES file_contents(sha256_hash),
            FOREIGN KEY (previous_hash) REFERENCES file_contents(sha256_hash)
        );

        CREATE INDEX idx_file_history_changeset ON file_history(changeset_id);
        CREATE INDEX idx_file_history_path ON file_history(path);
        ",
    )?;

    info!("Schema version 3 applied successfully");
    Ok(())
}

/// Schema Version 4: Add repository management support
///
/// Adds tables for remote repository management:
/// - repositories: Repository configuration and metadata
/// - repository_packages: Package metadata index from repositories
fn migrate_v4(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 4");

    conn.execute_batch(
        "
        -- Repositories: Remote package sources
        CREATE TABLE repositories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            url TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            priority INTEGER NOT NULL DEFAULT 0,
            gpg_check INTEGER NOT NULL DEFAULT 1,
            gpg_key_url TEXT,
            metadata_expire INTEGER NOT NULL DEFAULT 3600,
            last_sync TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX idx_repositories_name ON repositories(name);
        CREATE INDEX idx_repositories_enabled ON repositories(enabled);
        CREATE INDEX idx_repositories_priority ON repositories(priority);

        -- Repository packages: Available packages from repositories
        CREATE TABLE repository_packages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            architecture TEXT,
            description TEXT,
            checksum TEXT NOT NULL,
            size INTEGER NOT NULL,
            download_url TEXT NOT NULL,
            dependencies TEXT,
            metadata TEXT,
            synced_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (repository_id) REFERENCES repositories(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_repo_packages_name ON repository_packages(name);
        CREATE INDEX idx_repo_packages_repo ON repository_packages(repository_id);
        CREATE INDEX idx_repo_packages_checksum ON repository_packages(checksum);
        CREATE UNIQUE INDEX idx_repo_packages_unique ON repository_packages(repository_id, name, version, architecture);
        ",
    )?;

    info!("Schema version 4 applied successfully");
    Ok(())
}

/// Schema Version 5: Add delta update support
///
/// Adds tables for tracking available deltas and bandwidth metrics:
/// - package_deltas: Available delta files with metadata
/// - delta_stats: Bandwidth savings metrics per changeset
fn migrate_v5(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 5");

    conn.execute_batch(
        "
        -- Package deltas: Available delta files for updates
        CREATE TABLE package_deltas (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            package_name TEXT NOT NULL,
            from_version TEXT NOT NULL,
            to_version TEXT NOT NULL,
            from_hash TEXT NOT NULL,
            to_hash TEXT NOT NULL,
            delta_url TEXT NOT NULL,
            delta_size INTEGER NOT NULL,
            delta_checksum TEXT NOT NULL,
            full_size INTEGER NOT NULL,
            compression_ratio REAL NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (from_hash) REFERENCES file_contents(sha256_hash),
            FOREIGN KEY (to_hash) REFERENCES file_contents(sha256_hash)
        );

        CREATE INDEX idx_package_deltas_package ON package_deltas(package_name);
        CREATE INDEX idx_package_deltas_from_hash ON package_deltas(from_hash);
        CREATE INDEX idx_package_deltas_to_hash ON package_deltas(to_hash);
        CREATE UNIQUE INDEX idx_package_deltas_transition ON package_deltas(package_name, from_version, to_version);

        -- Delta statistics: Bandwidth metrics per changeset
        CREATE TABLE delta_stats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            changeset_id INTEGER NOT NULL,
            total_bytes_saved INTEGER NOT NULL,
            deltas_applied INTEGER NOT NULL,
            full_downloads INTEGER NOT NULL,
            delta_failures INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (changeset_id) REFERENCES changesets(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_delta_stats_changeset ON delta_stats(changeset_id);
        ",
    )?;

    info!("Schema version 5 applied successfully");
    Ok(())
}

/// Schema Version 6: Add install source tracking for package adoption
///
/// Adds install_source column to troves table to track how packages were installed:
/// - 'file': Installed from local package file
/// - 'repository': Installed from Conary repository
/// - 'adopted-track': Adopted from system, metadata only
/// - 'adopted-full': Adopted from system with full CAS storage
fn migrate_v6(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 6");

    conn.execute_batch(
        "
        -- Add install source tracking to troves
        ALTER TABLE troves ADD COLUMN install_source TEXT DEFAULT 'file';
        ",
    )?;

    info!("Schema version 6 applied successfully");
    Ok(())
}

/// Schema Version 7: Add metadata storage for rollback of removals
///
/// Adds metadata column to changesets table to store trove information
/// before deletion, enabling rollback of remove operations.
fn migrate_v7(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 7");

    conn.execute_batch(
        "
        -- Add metadata column to store trove info for removal rollback
        ALTER TABLE changesets ADD COLUMN metadata TEXT;
        ",
    )?;

    info!("Schema version 7 applied successfully");
    Ok(())
}

/// Schema Version 8: Add provides table for capability tracking
///
/// Creates a provides table to track what capabilities each package offers.
/// This enables self-contained dependency resolution without querying the
/// host package manager.
///
/// Capabilities include:
/// - Package names (e.g., "perl-Text-CharWidth")
/// - Virtual provides (e.g., "perl(Text::CharWidth)")
/// - Library sonames (e.g., "libc.so.6")
/// - File paths (e.g., "/usr/bin/perl")
fn migrate_v8(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 8");

    conn.execute_batch(
        "
        -- Capabilities/provides that packages offer
        CREATE TABLE provides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            capability TEXT NOT NULL,
            version TEXT,
            UNIQUE(trove_id, capability)
        );

        -- Index for fast capability lookups during dependency resolution
        CREATE INDEX idx_provides_capability ON provides(capability);
        ",
    )?;

    info!("Schema version 8 applied successfully");
    Ok(())
}

/// Schema Version 9: Add scriptlets table for package install/remove hooks
///
/// Creates a scriptlets table to store package scriptlets (install/remove hooks).
/// This enables execution of scriptlets during installation and removal,
/// and storage for later removal operations.
///
/// Scriptlet phases include:
/// - pre-install, post-install
/// - pre-remove, post-remove
/// - pre-upgrade, post-upgrade (Arch-specific)
fn migrate_v9(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 9");

    conn.execute_batch(
        "
        -- Scriptlets: Package install/remove hooks
        CREATE TABLE scriptlets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            phase TEXT NOT NULL,
            interpreter TEXT NOT NULL,
            content TEXT NOT NULL,
            flags TEXT,
            package_format TEXT NOT NULL DEFAULT 'rpm',
            UNIQUE(trove_id, phase)
        );

        -- Index for fast lookup of scriptlets by trove
        CREATE INDEX idx_scriptlets_trove ON scriptlets(trove_id);
        ",
    )?;

    info!("Schema version 9 applied successfully");
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
