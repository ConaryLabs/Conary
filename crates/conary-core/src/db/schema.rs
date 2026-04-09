// conary-core/src/db/schema.rs

//! Database schema definitions and migrations for Conary
//!
//! This module defines the SQLite schema for all core tables and provides
//! a migration system to evolve the schema over time.

use super::migrations;
use crate::error::Result;
use rusqlite::Connection;
use tracing::info;

/// Current schema version
pub const SCHEMA_VERSION: i32 = 66;

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

    let version = conn.query_row(
        "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
        [],
        |row| row.get(0),
    );

    match version {
        Ok(v) => Ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
        Err(e) => Err(e.into()),
    }
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

    // Apply migrations in order, each wrapped in a transaction for atomicity
    for version in (current_version + 1)..=SCHEMA_VERSION {
        info!("Applying migration to version {}", version);
        apply_migration_version(conn, version)?;
    }

    info!(
        "Schema migration complete. Now at version {}",
        SCHEMA_VERSION
    );
    Ok(())
}

fn migration_requires_foreign_keys_disabled(version: i32) -> bool {
    version == 63
}

fn apply_migration_version(conn: &Connection, version: i32) -> Result<()> {
    if migration_requires_foreign_keys_disabled(version) {
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    }

    let tx = conn.unchecked_transaction()?;
    let result = apply_migration(&tx, version).and_then(|()| set_schema_version(&tx, version));

    match result {
        Ok(()) => tx.commit()?,
        Err(e) => {
            drop(tx); // rollback on drop
            if migration_requires_foreign_keys_disabled(version) {
                let _ = conn.execute_batch("PRAGMA foreign_keys = ON;");
            }

            let observed_version = get_schema_version(conn)?;
            if observed_version >= version {
                info!(
                    "Migration version {} was already committed by another connection",
                    version
                );
                return Ok(());
            }

            return Err(e);
        }
    };

    if migration_requires_foreign_keys_disabled(version) {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
        if stmt.exists([])? {
            return Err(crate::error::Error::InitError(format!(
                "Foreign key check failed after migration version {}",
                version
            )));
        }
    }

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
        29 => migrations::migrate_v29(conn),
        30 => migrations::migrate_v30(conn),
        31 => migrations::migrate_v31(conn),
        32 => migrations::migrate_v32(conn),
        33 => migrations::migrate_v33(conn),
        34 => migrations::migrate_v34(conn),
        35 => migrations::migrate_v35(conn),
        36 => migrations::migrate_v36(conn),
        37 => migrations::migrate_v37(conn),
        38 => migrations::migrate_v38(conn),
        39 => migrations::migrate_v39(conn),
        40 => migrations::migrate_v40(conn),
        41 => migrations::migrate_v41(conn),
        42 => migrations::migrate_v42(conn),
        43 => migrations::migrate_v43(conn),
        44 => migrations::migrate_v44(conn),
        45 => migrations::migrate_v45(conn),
        46 => migrations::migrate_v46(conn),
        47 => migrations::migrate_v47(conn),
        48 => migrations::migrate_v48(conn),
        49 => migrations::migrate_v49(conn),
        50 => migrations::migrate_v50(conn),
        51 => migrations::migrate_v51(conn),
        52 => migrations::migrate_v52(conn),
        53 => migrations::migrate_v53(conn),
        54 => migrations::migrate_v54(conn),
        55 => migrations::migrate_v55(conn),
        56 => migrations::migrate_v56(conn),
        57 => migrations::migrate_v57(conn),
        58 => migrations::migrate_v58(conn),
        59 => migrations::migrate_v59(conn),
        60 => migrations::migrate_v60(conn),
        61 => migrations::migrate_v61(conn),
        62 => migrations::migrate_v62(conn),
        63 => migrations::migrate_v63(conn),
        64 => migrations::migrate_v64(conn),
        65 => migrations::migrate_v65(conn),
        66 => migrations::migrate_v66(conn),
        _ => Err(crate::error::Error::InitError(format!(
            "Unknown migration version: {}",
            version
        ))),
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

    fn create_test_db_at_version(version: i32) -> (NamedTempFile, Connection) {
        let (temp_file, conn) = create_test_db();
        init_schema_version(&conn).unwrap();

        for current in 1..=version {
            apply_migration(&conn, current).unwrap();
            set_schema_version(&conn, current).unwrap();
        }

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
    fn test_migrate_allows_post_hooks_failed_changesets() {
        let (_temp, conn) = create_test_db();
        migrate(&conn).unwrap();

        conn.execute(
            "INSERT INTO changesets (description, status, applied_at)
             VALUES (?1, 'post_hooks_failed', CURRENT_TIMESTAMP)",
            ["post-hooks degraded install"],
        )
        .unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM changesets ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "post_hooks_failed");
    }

    #[test]
    fn test_apply_migration_version_handles_concurrent_completion() {
        let (temp_file, conn_a) = create_test_db_at_version(64);
        let conn_b = Connection::open(temp_file.path()).unwrap();

        let stale_version = get_schema_version(&conn_a).unwrap();
        assert_eq!(stale_version, 64);

        apply_migration_version(&conn_b, 65).unwrap();
        apply_migration_version(&conn_a, 65).unwrap();

        let version_a = get_schema_version(&conn_a).unwrap();
        let version_b = get_schema_version(&conn_b).unwrap();
        assert_eq!(version_a, 65);
        assert_eq!(version_b, 65);

        let mut stmt = conn_a
            .prepare("PRAGMA table_info(derived_packages)")
            .unwrap();
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

        assert!(columns.contains(&"last_built_version".to_string()));
        assert!(columns.contains(&"last_built_parent_version".to_string()));
        assert!(columns.contains(&"build_artifact_hash".to_string()));
        assert!(columns.contains(&"build_artifact_path".to_string()));
        assert!(columns.contains(&"build_artifact_size".to_string()));
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
            rusqlite::params![
                "/usr/sbin/nginx",
                "abc123",
                1024,
                755,
                trove_id,
                component_id
            ],
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
