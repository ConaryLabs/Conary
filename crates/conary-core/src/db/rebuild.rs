// conary-core/src/db/rebuild.rs

//! Explicit current-schema replacement for retired pre-alpha databases.

use super::backup::backup_dir_for_db_path;
use super::schema::{self, SchemaCompatibility};
use crate::filesystem::durable::sync_parent_directory;
use crate::{Error, Result};
use chrono::Utc;
use rusqlite::{Connection, OpenFlags};
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseRebuildOutcome {
    pub observed_schema: String,
    pub retired_snapshot_path: PathBuf,
}

/// Replace a retired database with the one current pre-alpha schema.
///
/// This deliberately does not infer installed state from runtime artifacts.
/// The complete committed retired database is first consolidated into a
/// durable SQLite snapshot so an operator can inspect or recover it manually.
pub fn rebuild_discarding_state(path: impl AsRef<Path>) -> Result<DatabaseRebuildOutcome> {
    let path = path.as_ref();
    let observed_schema = match schema::inspect(path)? {
        SchemaCompatibility::RebuildRequired { observed } => observed,
        SchemaCompatibility::Current => {
            return Err(Error::InitError(format!(
                "database at {} already uses the current schema; refusing to discard it",
                path.display()
            )));
        }
        SchemaCompatibility::Fresh => {
            return Err(Error::InitError(format!(
                "database at {} is fresh; run `conary system init` instead",
                path.display()
            )));
        }
    };

    let retired_snapshot_path = create_retired_snapshot(path)?;
    remove_active_database_files(path)?;

    if let Err(init_error) = super::init(path) {
        if let Err(restore_error) = restore_retired_snapshot(path, &retired_snapshot_path) {
            return Err(Error::InitError(format!(
                "failed to initialize the replacement database: {init_error}; also failed to restore the retired snapshot: {restore_error}"
            )));
        }
        return Err(init_error);
    }

    Ok(DatabaseRebuildOutcome {
        observed_schema,
        retired_snapshot_path,
    })
}

fn create_retired_snapshot(db_path: &Path) -> Result<PathBuf> {
    let backup_dir = backup_dir_for_db_path(db_path);
    std::fs::create_dir_all(&backup_dir)?;

    let now = Utc::now();
    let timestamp = now.format("%Y%m%dT%H%M%SZ");
    let unique = now
        .timestamp_nanos_opt()
        .unwrap_or_else(|| now.timestamp_micros() * 1_000);
    let snapshot_path = backup_dir.join(format!(
        "conary-db-{timestamp}-{unique}-retired-schema.sqlite"
    ));
    let snapshot_string = snapshot_path.to_string_lossy().into_owned();

    let source = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    source.execute("VACUUM main INTO ?1", [snapshot_string.as_str()])?;
    drop(source);

    File::open(&snapshot_path)?.sync_all()?;
    sync_parent_directory(&snapshot_path)?;
    Ok(snapshot_path)
}

fn remove_active_database_files(db_path: &Path) -> Result<()> {
    for path in [
        sqlite_sidecar_path(db_path, "-wal"),
        sqlite_sidecar_path(db_path, "-shm"),
        db_path.to_path_buf(),
    ] {
        remove_if_exists(&path)?;
    }
    Ok(())
}

fn restore_retired_snapshot(db_path: &Path, snapshot_path: &Path) -> Result<()> {
    remove_active_database_files(db_path)?;
    std::fs::copy(snapshot_path, db_path)?;
    File::open(db_path)?.sync_all()?;
    sync_parent_directory(db_path)
}

fn sqlite_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut path = OsString::from(db_path.as_os_str());
    path.push(suffix);
    PathBuf::from(path)
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => sync_parent_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_retired_database(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            INSERT INTO schema_version (version) VALUES (66);
            CREATE TABLE retired_evidence (value TEXT NOT NULL);
            INSERT INTO retired_evidence (value) VALUES ('preserved');",
        )
        .unwrap();
    }

    #[test]
    fn retired_database_is_snapshotted_and_replaced_with_current_schema() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        create_retired_database(&db_path);

        let outcome = rebuild_discarding_state(&db_path).unwrap();

        assert_eq!(
            outcome.observed_schema,
            "retired migration-chain schema version 66"
        );
        assert_eq!(
            schema::inspect(&db_path).unwrap(),
            SchemaCompatibility::Current
        );
        assert!(outcome.retired_snapshot_path.exists());
        let retired = Connection::open(&outcome.retired_snapshot_path).unwrap();
        assert_eq!(
            retired
                .query_row("SELECT value FROM retired_evidence", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "preserved"
        );
    }

    #[test]
    fn rebuild_refuses_current_and_fresh_databases() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current.db");
        super::super::init(&current).unwrap();
        let current_error = rebuild_discarding_state(&current).unwrap_err().to_string();
        assert!(current_error.contains("already uses the current schema"));
        assert_eq!(
            schema::inspect(&current).unwrap(),
            SchemaCompatibility::Current
        );

        let fresh = temp.path().join("missing.db");
        let fresh_error = rebuild_discarding_state(&fresh).unwrap_err().to_string();
        assert!(fresh_error.contains("run `conary system init` instead"));
        assert!(!fresh.exists());
    }
}
