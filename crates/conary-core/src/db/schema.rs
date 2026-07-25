// conary-core/src/db/schema.rs

//! Current-only SQLite schema initialization.
//!
//! Conary is pre-alpha and does not carry forward databases from former
//! schema revisions. A database either has the current schema or
//! must be rebuilt from authoritative package and repository inputs.

use super::current_schema;
use crate::error::{Error, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;
use tracing::info;

/// Revision 10 of the current-only schema epoch.
///
/// Revision 10 adds normalized, byte-exact Debian debconf state. Pre-alpha
/// databases from revision 9 must be rebuilt; no compatibility migration is
/// provided.
pub const SCHEMA_VERSION: i32 = 10;
/// Stable identity that distinguishes this epoch from retired schema revisions.
pub const SCHEMA_EPOCH: &str = "conary-current-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaCompatibility {
    Fresh,
    Current,
    RebuildRequired { observed: String },
}

/// Inspect a database without creating, validating, or otherwise mutating it.
pub fn inspect(path: impl AsRef<Path>) -> Result<SchemaCompatibility> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(SchemaCompatibility::Fresh);
    }

    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let current_version = get_schema_version(&conn)?;
    match get_schema_identity(&conn)? {
        Some((epoch, revision)) if epoch == SCHEMA_EPOCH && revision == SCHEMA_VERSION => {
            if current_version == SCHEMA_VERSION {
                Ok(SchemaCompatibility::Current)
            } else {
                Ok(SchemaCompatibility::RebuildRequired {
                    observed: format!(
                        "schema epoch {epoch} with inconsistent version {current_version}"
                    ),
                })
            }
        }
        Some((epoch, revision)) => Ok(SchemaCompatibility::RebuildRequired {
            observed: format!("schema epoch {epoch} revision {revision}"),
        }),
        None if database_is_fresh(&conn)? => Ok(SchemaCompatibility::Fresh),
        None if current_version == 0 => Ok(SchemaCompatibility::RebuildRequired {
            observed: "unversioned non-empty database".to_string(),
        }),
        None => Ok(SchemaCompatibility::RebuildRequired {
            observed: format!("retired migration-chain schema version {current_version}"),
        }),
    }
}

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
pub fn ensure_current(conn: &Connection) -> Result<()> {
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
    current_schema::create_current_schema(&tx)?;
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
            WHERE name NOT LIKE 'sqlite_%'
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
    fn ensure_current_creates_schema_atomically() {
        let conn = Connection::open_in_memory().unwrap();

        ensure_current(&conn).unwrap();

        assert_eq!(get_schema_version(&conn).unwrap(), SCHEMA_VERSION);
        assert_eq!(
            get_schema_identity(&conn).unwrap(),
            Some((SCHEMA_EPOCH.to_string(), SCHEMA_VERSION))
        );
        for table in [
            "troves",
            "repositories",
            "converted_packages",
            "installed_file_capabilities",
            "generation_publications",
            "activation_requests",
            "generation_activation_intents",
            "debian_alternative_groups",
        ] {
            assert!(table_exists(&conn, table).unwrap(), "missing {table}");
        }
    }

    #[test]
    fn ensure_current_is_idempotent_for_current_epoch() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        ensure_current(&conn).unwrap();
        assert_eq!(get_schema_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn read_only_inspection_names_exact_deployment_action() {
        let missing_dir = tempfile::tempdir().unwrap();
        let missing = missing_dir.path().join("missing.db");
        assert_eq!(inspect(&missing).unwrap(), SchemaCompatibility::Fresh);

        let current = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(current.path()).unwrap();
        ensure_current(&conn).unwrap();
        drop(conn);
        assert_eq!(
            inspect(current.path()).unwrap(),
            SchemaCompatibility::Current
        );

        let retired = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(retired.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            INSERT INTO schema_version (version) VALUES (79);",
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            inspect(retired.path()).unwrap(),
            SchemaCompatibility::RebuildRequired {
                observed: "retired migration-chain schema version 79".to_string()
            }
        );
    }

    #[test]
    fn current_schema_seeds_required_runtime_records() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();

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
    fn current_schema_requires_explicit_installation_semantics() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();

        let mut trove_columns = conn.prepare("PRAGMA table_info('troves')").unwrap();
        let trove_columns = trove_columns
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for required in ["install_source", "install_reason", "version_scheme"] {
            let (_, not_null, default) = trove_columns
                .iter()
                .find(|(name, _, _)| name == required)
                .unwrap_or_else(|| panic!("missing troves.{required}"));
            assert!(*not_null, "troves.{required} must be NOT NULL");
            assert_eq!(
                default, &None,
                "troves.{required} must not invent a default"
            );
        }

        for sql in [
            "INSERT INTO troves (
                 name, version, type, install_reason, version_scheme
             ) VALUES ('missing-source', '1', 'package', 'explicit', 'conary')",
            "INSERT INTO troves (
                 name, version, type, install_source, version_scheme
             ) VALUES ('missing-reason', '1', 'package', 'file', 'conary')",
            "INSERT INTO troves (
                 name, version, type, install_source, install_reason
             ) VALUES ('missing-scheme', '1', 'package', 'file', 'explicit')",
        ] {
            let error = conn.execute(sql, []).unwrap_err();
            assert!(
                error.to_string().contains("NOT NULL constraint failed"),
                "{error}"
            );
        }

        conn.execute(
            "INSERT INTO system_states (state_number, summary, package_count)
             VALUES (1, 'explicit semantics', 0)",
            [],
        )
        .unwrap();
        let error = conn
            .execute(
                "INSERT INTO state_members (state_id, trove_name, trove_version)
                 VALUES (1, 'missing-reason', '1')",
                [],
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("state_members.install_reason"),
            "{error}"
        );
    }

    #[test]
    fn current_schema_requires_exact_native_identity_for_native_owned_troves() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();

        for sql in [
            "INSERT INTO troves (
                 name, version, type, install_source, install_reason, version_scheme
             ) VALUES (
                 'missing-native-id', '1', 'package', 'adopted-full', 'explicit', 'rpm'
             )",
            "INSERT INTO troves (
                 name, version, type, install_source, install_reason, version_scheme,
                 native_package_identity_json
             ) VALUES (
                 'malformed-native-id', '1', 'package', 'taken', 'explicit', 'rpm', '{bad'
             )",
            "INSERT INTO troves (
                 name, version, type, install_source, install_reason, version_scheme,
                 native_package_identity_json
             ) VALUES (
                 'conary-with-native-id', '1', 'package', 'file', 'explicit', 'conary',
                 '{\"manager\":\"pacman\",\"selector\":\"bash\",\"name\":\"bash\",
                   \"version\":\"5.3-1\",\"architecture\":\"x86_64\"}'
             )",
        ] {
            assert!(conn.execute(sql, []).is_err(), "{sql}");
        }

        conn.execute(
            "INSERT INTO troves (
                 name, version, type, architecture, install_source, install_reason,
                 version_scheme, native_package_identity_json
             ) VALUES (
                 'perl', '4:5.40.0-1.fc44', 'package', 'x86_64',
                 'adopted-full', 'explicit', 'rpm',
                 '{\"manager\":\"rpm\",
                   \"selector\":\"perl-4:5.40.0-1.fc44.x86_64\",
                   \"name\":\"perl\",\"epoch\":4,\"version\":\"5.40.0\",
                   \"release\":\"1.fc44\",\"architecture\":\"x86_64\"}'
             )",
            [],
        )
        .unwrap();

        for sql in [
            "INSERT INTO troves (
                 name, version, type, architecture, install_source, install_reason,
                 version_scheme, native_package_identity_json
             ) VALUES (
                 'wrong-name', '2.41-12', 'package', 'i386',
                 'adopted-track', 'explicit', 'debian',
                 '{\"manager\":\"dpkg\",\"selector\":\"libc6:i386\",\"name\":\"libc6\",
                   \"version\":\"2.41-12\",\"architecture\":\"i386\"}'
             )",
            "INSERT INTO troves (
                 name, version, type, architecture, install_source, install_reason,
                 version_scheme, native_package_identity_json
             ) VALUES (
                 'libc6', '2.41-11', 'package', 'i386',
                 'adopted-track', 'explicit', 'debian',
                 '{\"manager\":\"dpkg\",\"selector\":\"libc6:i386\",\"name\":\"libc6\",
                   \"version\":\"2.41-12\",\"architecture\":\"i386\"}'
             )",
            "INSERT INTO troves (
                 name, version, type, architecture, install_source, install_reason,
                 version_scheme, native_package_identity_json
             ) VALUES (
                 'libc6', '2.41-12', 'package', 'amd64',
                 'adopted-track', 'explicit', 'debian',
                 '{\"manager\":\"dpkg\",\"selector\":\"libc6:i386\",\"name\":\"libc6\",
                   \"version\":\"2.41-12\",\"architecture\":\"i386\"}'
             )",
            "INSERT INTO troves (
                 name, version, type, architecture, install_source, install_reason,
                 version_scheme, native_package_identity_json
             ) VALUES (
                 'libc6', '2.41-12', 'package', 'i386',
                 'adopted-track', 'explicit', 'debian',
                 '{\"manager\":\"dpkg\",\"selector\":\"libc6:amd64\",\"name\":\"libc6\",
                   \"version\":\"2.41-12\",\"architecture\":\"i386\"}'
             )",
        ] {
            assert!(conn.execute(sql, []).is_err(), "{sql}");
        }
    }

    #[test]
    fn current_schema_separates_installed_and_repository_conversions() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        ensure_current(&conn).unwrap();
        conn.execute(
            "INSERT INTO troves (
                 name, version, type, install_source, install_reason, version_scheme
             ) VALUES ('installed', '1', 'package', 'file', 'explicit', 'conary')",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO converted_packages (
                 artifact_kind, trove_id, original_format, original_checksum,
                 conversion_version
             ) VALUES ('installed', 1, 'rpm', 'sha256:installed', 11)",
            [],
        )
        .unwrap();

        for sql in [
            "INSERT INTO converted_packages (
                 artifact_kind, original_format, original_checksum, conversion_version
             ) VALUES ('installed', 'rpm', 'sha256:installed-without-trove', 11)",
            "INSERT INTO converted_packages (
                 artifact_kind, trove_id, original_format, original_checksum,
                 conversion_version, package_name
             ) VALUES ('installed', 1, 'rpm', 'sha256:installed-with-repo-field', 11, 'pkg')",
            "INSERT INTO converted_packages (
                 artifact_kind, original_format, original_checksum, conversion_version,
                 package_version, distro, chunk_hashes_json, total_size, content_hash, ccs_path
             ) VALUES (
                 'repository', 'rpm', 'sha256:missing-name', 11,
                 '1', 'fedora', '[]', 0, 'sha256:content', '/tmp/pkg.ccs'
             )",
            "INSERT INTO converted_packages (
                 artifact_kind, original_format, original_checksum, conversion_version,
                 package_name, distro, chunk_hashes_json, total_size, content_hash, ccs_path
             ) VALUES (
                 'repository', 'rpm', 'sha256:missing-version', 11,
                 'pkg', 'fedora', '[]', 0, 'sha256:content', '/tmp/pkg.ccs'
             )",
            "INSERT INTO converted_packages (
                 artifact_kind, original_format, original_checksum, conversion_version,
                 package_name, package_version, distro, total_size, content_hash, ccs_path
             ) VALUES (
                 'repository', 'rpm', 'sha256:missing-chunks', 11,
                 'pkg', '1', 'fedora', 0, 'sha256:content', '/tmp/pkg.ccs'
             )",
            "INSERT INTO converted_packages (
                 artifact_kind, original_format, original_checksum, conversion_version,
                 package_name, package_version, distro, chunk_hashes_json, content_hash, ccs_path
             ) VALUES (
                 'repository', 'rpm', 'sha256:missing-size', 11,
                 'pkg', '1', 'fedora', '[]', 'sha256:content', '/tmp/pkg.ccs'
             )",
            "INSERT INTO converted_packages (
                 artifact_kind, original_format, original_checksum, conversion_version,
                 package_name, package_version, distro, chunk_hashes_json, total_size, ccs_path
             ) VALUES (
                 'repository', 'rpm', 'sha256:missing-hash', 11,
                 'pkg', '1', 'fedora', '[]', 0, '/tmp/pkg.ccs'
             )",
            "INSERT INTO converted_packages (
                 artifact_kind, original_format, original_checksum, conversion_version,
                 package_name, package_version, distro, chunk_hashes_json, total_size, content_hash
             ) VALUES (
                 'repository', 'rpm', 'sha256:missing-path', 11,
                 'pkg', '1', 'fedora', '[]', 0, 'sha256:content'
             )",
        ] {
            assert!(conn.execute(sql, []).is_err(), "{sql}");
        }
    }

    #[test]
    fn current_schema_persists_only_executable_daemon_job_kinds() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();

        for kind in ["install", "remove", "update", "enhance"] {
            conn.execute(
                "INSERT INTO daemon_jobs (id, kind, spec_json) VALUES (?1, ?2, '{}')",
                params![format!("job-{kind}"), kind],
            )
            .unwrap();
        }
        for kind in ["dry_run", "rollback", "verify", "garbage_collect", "gc"] {
            assert!(
                conn.execute(
                    "INSERT INTO daemon_jobs (id, kind, spec_json) VALUES (?1, ?2, '{}')",
                    params![format!("job-{kind}"), kind],
                )
                .is_err(),
                "unsupported daemon job {kind} was persisted"
            );
        }
    }

    #[test]
    fn ensure_current_rejects_every_retired_incremental_schema_version() {
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

            let error = ensure_current(&conn).unwrap_err().to_string();
            assert!(
                error.contains(&format!("migration-chain schema version {version}")),
                "{error}"
            );
            assert!(error.contains("Rebuild the database"));
        }
    }

    #[test]
    fn ensure_current_rejects_nonempty_unversioned_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE stray (id INTEGER PRIMARY KEY)", [])
            .unwrap();

        let error = ensure_current(&conn).unwrap_err().to_string();
        assert!(error.contains("unversioned non-empty"));
        assert!(!table_exists(&conn, "troves").unwrap());
    }

    #[test]
    fn ensure_current_rejects_unversioned_schema_objects_without_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE VIEW stray AS SELECT 1 AS id", [])
            .unwrap();

        let error = ensure_current(&conn).unwrap_err().to_string();
        assert!(error.contains("unversioned non-empty"));
        assert!(error.contains("Rebuild the database"));
        assert!(!table_exists(&conn, "troves").unwrap());
    }

    #[test]
    fn ensure_current_rejects_unversioned_indexes_and_triggers() {
        for ddl in [
            "CREATE TABLE stray (id INTEGER PRIMARY KEY);
             CREATE INDEX stray_index ON stray(id);",
            "CREATE TABLE stray (id INTEGER PRIMARY KEY);
             CREATE TRIGGER stray_trigger AFTER INSERT ON stray
             BEGIN
                 SELECT NEW.id;
             END;",
        ] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(ddl).unwrap();

            let error = ensure_current(&conn).unwrap_err().to_string();
            assert!(error.contains("unversioned non-empty"), "{error}");
            assert!(error.contains("Rebuild the database"), "{error}");
            assert!(!table_exists(&conn, "troves").unwrap());
        }
    }

    #[test]
    fn current_schema_has_no_scriptlet_review_queue_objects() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();

        for name in [
            "scriptlet_evidence_clusters",
            "scriptlet_evidence_cluster_samples",
            "scriptlet_evidence_state_events",
            "scriptlet_evidence_notes",
            "scriptlet_evidence_backfill_runs",
            "idx_scriptlet_evidence_clusters_state_last_seen",
            "idx_scriptlet_evidence_clusters_class",
            "idx_scriptlet_evidence_samples_cluster",
            "idx_scriptlet_evidence_samples_package",
            "idx_scriptlet_evidence_samples_unique_observation",
            "idx_scriptlet_evidence_backfill_status",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM sqlite_schema WHERE name = ?1
                    )",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(
                !exists,
                "retired scriptlet review queue object remains: {name}"
            );
        }

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
