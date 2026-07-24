// conary-core/src/db/schema.rs

//! Current-only SQLite schema initialization.
//!
//! Conary is pre-alpha and does not carry forward databases from the former
//! incremental migration epoch. A database either has the current schema or
//! must be rebuilt from authoritative package and repository inputs.

use super::migrations;
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use tracing::info;

/// Version 1 of the current-only schema epoch.
pub const SCHEMA_VERSION: i32 = 1;
/// Stable identity that distinguishes this epoch from the retired migration chain.
pub const SCHEMA_EPOCH: &str = "conary-current-v1";

/// Return zero for a fresh database or the exact schema epoch stored on disk.
pub fn get_schema_version(conn: &Connection) -> Result<i32> {
    if !table_exists(conn, "schema_version")? {
        return Ok(0);
    }

    conn.query_row(
        "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map(|version| version.unwrap_or(0))
    .map_err(Into::into)
}

/// Initialize a fresh database or validate that it already uses this epoch.
///
/// Any prior schema is rejected with an explicit rebuild requirement. This is
/// deliberate: carrying an untested compatibility chain would make old
/// structure, queue normalization, and retired workflow state authoritative.
pub fn migrate(conn: &Connection) -> Result<()> {
    let current_version = get_schema_version(conn)?;
    match get_schema_identity(conn)? {
        Some((epoch, revision)) if epoch == SCHEMA_EPOCH && revision == SCHEMA_VERSION => {
            if current_version != SCHEMA_VERSION {
                return Err(rebuild_required(&format!(
                    "schema epoch {epoch} with inconsistent version {current_version}"
                )));
            }
            info!("Schema is current at epoch {}", SCHEMA_VERSION);
            return Ok(());
        }
        Some((epoch, revision)) => {
            return Err(rebuild_required(&format!(
                "schema epoch {epoch} revision {revision}"
            )));
        }
        None if database_is_fresh(conn)? => {}
        None if current_version == 0 => return Err(rebuild_required("unversioned non-empty")),
        None => {
            return Err(rebuild_required(&format!(
                "retired migration-chain schema version {current_version}"
            )));
        }
    }

    let tx = conn.unchecked_transaction()?;
    migrations::create_current_schema(&tx)?;
    tx.execute(
        "INSERT INTO schema_identity (epoch, revision) VALUES (?1, ?2)",
        params![SCHEMA_EPOCH, SCHEMA_VERSION],
    )?;
    tx.execute(
        "INSERT INTO schema_version (version) VALUES (?1)",
        params![SCHEMA_VERSION],
    )?;
    tx.commit()?;
    info!("Initialized current schema epoch {}", SCHEMA_VERSION);
    Ok(())
}

fn get_schema_identity(conn: &Connection) -> Result<Option<(String, i32)>> {
    if !table_exists(conn, "schema_identity")? {
        return Ok(None);
    }
    conn.query_row(
        "SELECT epoch, revision FROM schema_identity LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM sqlite_schema
            WHERE type = 'table' AND name = ?1
        )",
        [table],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn database_is_fresh(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT NOT EXISTS(
            SELECT 1
            FROM sqlite_schema
            WHERE type = 'table'
              AND name NOT LIKE 'sqlite_%'
        )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn rebuild_required(observed: &str) -> Error {
    Error::InitError(format!(
        "database uses {observed}; this pre-alpha build supports only schema epoch \
         {SCHEMA_EPOCH} revision {SCHEMA_VERSION}. Rebuild the database from authoritative inputs"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_creates_current_schema_atomically() {
        let conn = Connection::open_in_memory().unwrap();

        migrate(&conn).unwrap();

        assert_eq!(get_schema_version(&conn).unwrap(), SCHEMA_VERSION);
        assert_eq!(
            get_schema_identity(&conn).unwrap(),
            Some((SCHEMA_EPOCH.to_string(), SCHEMA_VERSION))
        );
        for table in [
            "troves",
            "repositories",
            "converted_packages",
            "scriptlet_evidence_clusters",
            "installed_file_capabilities",
        ] {
            assert!(table_exists(&conn, table).unwrap(), "missing {table}");
        }
    }

    #[test]
    fn migrate_is_idempotent_for_current_epoch() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(get_schema_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn current_schema_seeds_required_runtime_records() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let trigger_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM triggers WHERE builtin = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trigger_count, 10);
        assert_eq!(
            conn.query_row(
                "SELECT handler FROM triggers WHERE name = 'ldconfig'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "/sbin/ldconfig"
        );
        assert_eq!(
            conn.query_row(
                "SELECT value FROM server_metadata WHERE key = 'canonical_map_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "0"
        );
    }

    #[test]
    fn migrate_rejects_every_retired_incremental_schema_version() {
        for version in [1, 79] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(&format!(
                "CREATE TABLE schema_version (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                INSERT INTO schema_version (version) VALUES ({version});"
            ))
            .unwrap();

            let error = migrate(&conn).unwrap_err().to_string();
            assert!(
                error.contains(&format!("migration-chain schema version {version}")),
                "{error}"
            );
            assert!(error.contains("Rebuild the database"));
        }
    }

    #[test]
    fn migrate_rejects_nonempty_unversioned_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE stray (id INTEGER PRIMARY KEY)", [])
            .unwrap();

        let error = migrate(&conn).unwrap_err().to_string();
        assert!(error.contains("unversioned non-empty"));
        assert!(!table_exists(&conn, "troves").unwrap());
    }

    #[test]
    fn current_scriptlet_queue_has_no_reconciliation_history_columns() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let mut stmt = conn
            .prepare("PRAGMA table_info('scriptlet_evidence_clusters')")
            .unwrap();
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        assert!(!columns.contains(&"normalization_version".to_string()));
        assert!(!columns.contains(&"superseded_at".to_string()));
        assert!(!table_exists(&conn, "scriptlet_evidence_cluster_reconciliation_links").unwrap());

        let mut stmt = conn
            .prepare("PRAGMA table_info('converted_packages')")
            .unwrap();
        let converted_columns = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(!converted_columns.contains(&"conversion_fidelity".to_string()));
        assert!(!converted_columns.contains(&"detected_hooks".to_string()));
    }
}
