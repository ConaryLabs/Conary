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
pub const SCHEMA_VERSION: i32 = 74;

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
        67 => migrations::migrate_v67(conn),
        68 => migrations::migrate_v68(conn),
        69 => migrations::migrate_v69(conn),
        70 => migrations::migrate_v70(conn),
        71 => migrations::migrate_v71(conn),
        72 => migrations::migrate_v72(conn),
        73 => migrations::migrate_v73(conn),
        74 => migrations::migrate_v74(conn),
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
        assert!(tables.contains(&"repository_package_keys".to_string()));
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
    fn migration_adds_scriptlet_metadata_columns_to_converted_packages() {
        let (_temp, conn) = create_test_db_at_version(69);
        conn.execute(
            "INSERT INTO converted_packages (original_format, original_checksum, conversion_version, conversion_fidelity, enhancement_version, enhancement_status)
             VALUES ('rpm', 'sha256:old', 3, 'high', 0, 'pending')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let row = conn
            .query_row(
                "SELECT scriptlet_fidelity, target_compatibility, publication_status, blocked_reason_codes_json, scriptlet_summary_json
                 FROM converted_packages
                 WHERE original_checksum = 'sha256:old'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0, "unknown");
        assert_eq!(row.1, "unknown");
        assert_eq!(row.2, "public");
        assert_eq!(row.3, "[]");
        assert_eq!(row.4, "{}");
    }

    #[test]
    fn migration_v71_creates_installed_legacy_scriptlet_bundles_table() {
        let (_temp, conn) = create_test_db_at_version(70);

        apply_migration_version(&conn, 71).unwrap();

        assert_eq!(get_schema_version(&conn).unwrap(), 71);

        let columns: Vec<(String, String, bool)> = conn
            .prepare("PRAGMA table_info(installed_legacy_scriptlet_bundles)")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)? != 0,
                ))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let column_names: Vec<&str> = columns.iter().map(|(name, _, _)| name.as_str()).collect();

        for required in [
            "id",
            "trove_id",
            "source_format",
            "source_family",
            "source_distro",
            "source_release",
            "source_arch",
            "source_package",
            "source_version",
            "target_id",
            "target_compatibility",
            "foreign_replay_policy",
            "scriptlet_fidelity",
            "publication_status",
            "evidence_digest",
            "replay_policy",
            "replay_enabled",
            "bundle_toml",
            "installed_changeset_id",
            "installed_at",
        ] {
            assert!(
                column_names.contains(&required),
                "missing installed bundle column {required}"
            );
        }

        assert!(
            columns
                .iter()
                .any(|(name, ty, required)| name == "trove_id" && ty == "INTEGER" && *required)
        );
        assert!(
            columns
                .iter()
                .any(|(name, ty, required)| name == "bundle_toml" && ty == "TEXT" && *required)
        );

        let indexes: Vec<String> = conn
            .prepare("PRAGMA index_list(installed_legacy_scriptlet_bundles)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(indexes.contains(&"idx_installed_legacy_scriptlet_bundles_trove".to_string()));
        assert!(indexes.contains(&"idx_installed_legacy_scriptlet_bundles_evidence".to_string()));

        let fks: Vec<(String, String, String)> = conn
            .prepare("PRAGMA foreign_key_list(installed_legacy_scriptlet_bundles)")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(fks.iter().any(|(from, table, on_delete)| {
            from == "trove_id" && table == "troves" && on_delete.eq_ignore_ascii_case("CASCADE")
        }));
        assert!(fks.iter().any(|(from, table, on_delete)| {
            from == "installed_changeset_id"
                && table == "changesets"
                && on_delete.eq_ignore_ascii_case("SET NULL")
        }));
    }

    #[test]
    fn migration_v72_creates_repository_package_keys_table() {
        let (_temp, conn) = create_test_db_at_version(71);

        apply_migration_version(&conn, 72).unwrap();

        assert_eq!(get_schema_version(&conn).unwrap(), 72);

        let columns: Vec<(String, String, bool, Option<String>, i32)> = conn
            .prepare("PRAGMA table_info(repository_package_keys)")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)? != 0,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i32>(5)?,
                ))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let column_names: Vec<&str> = columns
            .iter()
            .map(|(name, _, _, _, _)| name.as_str())
            .collect();

        for required in [
            "repository_id",
            "public_key",
            "key_id",
            "status",
            "synced_at",
        ] {
            assert!(
                column_names.contains(&required),
                "missing repository package key column {required}"
            );
        }

        assert!(columns.iter().any(|(name, ty, required, _, pk)| {
            name == "repository_id" && ty == "INTEGER" && *required && *pk == 1
        }));
        assert!(columns.iter().any(|(name, ty, required, _, pk)| {
            name == "public_key" && ty == "TEXT" && *required && *pk == 2
        }));
        assert!(
            columns
                .iter()
                .any(|(name, ty, required, _, _)| name == "key_id" && ty == "TEXT" && !*required)
        );
        assert!(
            columns
                .iter()
                .any(|(name, ty, required, _, _)| name == "status" && ty == "TEXT" && *required)
        );
        assert!(columns.iter().any(|(name, ty, required, default, _)| {
            name == "synced_at"
                && ty == "TEXT"
                && *required
                && default
                    .as_deref()
                    .is_some_and(|value| value.contains("strftime('%Y-%m-%dT%H:%M:%SZ', 'now')"))
        }));

        let create_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'repository_package_keys'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(create_sql.contains("status IN ('active', 'retired')"));
        assert!(create_sql.contains("REFERENCES repositories(id) ON DELETE CASCADE"));

        let indexes: Vec<String> = conn
            .prepare("PRAGMA index_list(repository_package_keys)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(indexes.contains(&"idx_repository_package_keys_repo".to_string()));
    }

    #[test]
    fn migration_v73_creates_try_sessions_table() {
        let (_temp, conn) = create_test_db_at_version(72);

        migrate(&conn).unwrap();

        assert_eq!(get_schema_version(&conn).unwrap(), SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, 74);

        let columns: Vec<(String, String, bool, Option<String>, i32)> = conn
            .prepare("PRAGMA table_info(try_sessions)")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)? != 0,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i32>(5)?,
                ))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let column_names: Vec<&str> = columns
            .iter()
            .map(|(name, _, _, _, _)| name.as_str())
            .collect();

        for required in [
            "id",
            "package_path",
            "package_name",
            "package_version",
            "previous_generation_id",
            "try_generation_id",
            "launcher_pid",
            "launcher_boot_id",
            "status",
            "mode",
            "open_slot",
            "work_dir",
            "last_error",
            "started_at",
            "updated_at",
            "completed_at",
        ] {
            assert!(
                column_names.contains(&required),
                "missing try_sessions column {required}"
            );
        }

        assert!(
            columns
                .iter()
                .any(|(name, ty, _, _, pk)| { name == "id" && ty == "TEXT" && *pk == 1 })
        );
        assert!(columns.iter().any(|(name, ty, required, _, _)| {
            name == "package_path" && ty == "TEXT" && *required
        }));
        assert!(
            columns
                .iter()
                .any(|(name, ty, required, _, _)| name == "status" && ty == "TEXT" && *required)
        );
        assert!(
            columns
                .iter()
                .any(|(name, ty, required, _, _)| name == "mode" && ty == "TEXT" && *required)
        );
        assert!(columns.iter().any(|(name, ty, required, default, _)| {
            name == "open_slot" && ty == "INTEGER" && *required && default.as_deref() == Some("1")
        }));
        assert!(columns.iter().any(|(name, ty, required, default, _)| {
            name == "started_at"
                && ty == "TEXT"
                && *required
                && default
                    .as_deref()
                    .is_some_and(|value| value.contains("strftime('%Y-%m-%dT%H:%M:%SZ', 'now')"))
        }));
        assert!(columns.iter().any(|(name, ty, required, default, _)| {
            name == "updated_at"
                && ty == "TEXT"
                && *required
                && default
                    .as_deref()
                    .is_some_and(|value| value.contains("strftime('%Y-%m-%dT%H:%M:%SZ', 'now')"))
        }));

        let create_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'try_sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(create_sql.contains("status IN ('active', 'orphaned', 'kept', 'rolled_back')"));
        assert!(create_sql.contains("mode IN ('namespace', 'activated')"));
        assert!(create_sql.contains("CHECK (open_slot = 1)"));

        let index_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'idx_try_sessions_single_open'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(index_sql.contains("ON try_sessions(open_slot)"));
        assert!(index_sql.contains("WHERE status IN ('active', 'orphaned')"));
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
