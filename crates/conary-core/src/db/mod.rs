// conary-core/src/db/mod.rs

//! Database layer for Conary
//!
//! This module handles all SQLite operations including:
//! - Database initialization and schema creation
//! - Connection management
//! - Transaction handling
//! - CRUD operations for troves, changesets, files, etc.

pub mod backup;
pub mod current_schema;
pub mod generation_backup_chain;
pub mod generation_delta;
pub mod generation_snapshot;
pub mod models;
pub mod paths;
pub mod rebuild;
pub(crate) mod remi_universe;
pub mod schema;

use crate::error::{Error, Result};
use rusqlite::{Connection, OpenFlags};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;
use tracing::{debug, info};

/// Standard PRAGMAs applied to every connection.
///
/// WAL mode persists in the file, but `synchronous`, `foreign_keys`, and
/// `busy_timeout` are session-level and must be set on each open.
const CONNECTION_PRAGMAS: &str = "\
    PRAGMA journal_mode = WAL;\
    PRAGMA synchronous = NORMAL;\
    PRAGMA foreign_keys = ON;\
    PRAGMA busy_timeout = 5000;\
";

const READ_ONLY_CONNECTION_PRAGMAS: &str = "\
    PRAGMA foreign_keys = ON;\
    PRAGMA busy_timeout = 5000;\
";

const SQLITE_WAL_HEADER_SIZE: u64 = 32;
const SQLITE_WAL_MAGIC_BE: [u32; 2] = [0x377f0682, 0x377f0683];

/// Apply standard PRAGMAs to a connection
fn configure(conn: &Connection) -> Result<()> {
    generation_delta::configure_mutation_epoch(conn)?;
    conn.execute_batch(CONNECTION_PRAGMAS)?;
    Ok(())
}

fn database_wal_path(path: &Path) -> std::path::PathBuf {
    // Construct WAL path: "foo.db" -> "foo.db-wal", "foo" -> "foo-wal"
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}-wal")),
        None => {
            let mut p = path.as_os_str().to_os_string();
            p.push("-wal");
            std::path::PathBuf::from(p)
        }
    }
}

fn validate_wal_file(path: &Path) -> Result<()> {
    let wal_path = database_wal_path(path);
    let file = match File::open(&wal_path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };

    validate_open_wal_file(&wal_path, file)
}

fn validate_open_wal_file(wal_path: &Path, mut file: File) -> Result<()> {
    let metadata = file.metadata()?;
    if metadata.len() == 0 {
        return Ok(());
    }
    if metadata.len() < SQLITE_WAL_HEADER_SIZE {
        return Err(Error::InitError(format!(
            "database WAL appears corrupted: {} is too small ({} bytes)",
            wal_path.display(),
            metadata.len()
        )));
    }

    let mut header = [0_u8; 4];
    file.read_exact(&mut header)?;
    let magic = u32::from_be_bytes(header);
    if !SQLITE_WAL_MAGIC_BE.contains(&magic) {
        return Err(Error::InitError(format!(
            "database WAL appears corrupted: invalid header in {}",
            wal_path.display()
        )));
    }

    Ok(())
}

/// Initialize a new Conary database at the specified path
///
/// Creates the database file and sets up the initial schema.
/// This is idempotent - calling it on an existing database is safe.
///
/// # Arguments
///
/// * `db_path` - Path where the database should be created
///
/// # Returns
///
/// * `Result<()>` - Ok if successful, Error otherwise
pub fn init(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    debug!("Initializing database at: {}", path.display());

    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::InitError(format!("Failed to create database directory: {}", e)))?;
    }

    let conn = Connection::open(path)?;
    configure(&conn)?;
    schema::ensure_current(&conn)?;

    info!("Database initialized successfully");
    Ok(())
}

/// Open an existing Conary database
///
/// # Arguments
///
/// * `db_path` - Path to the database file
///
/// # Returns
///
/// * `Result<Connection>` - Database connection if successful
pub fn open(path: impl AsRef<Path>) -> Result<Connection> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(Error::DatabaseNotFound(path.to_string_lossy().to_string()));
    }

    validate_wal_file(path)?;
    let conn = Connection::open(path)?;
    configure(&conn)?;
    schema::ensure_current(&conn)?;
    remi_universe::attach_active_index(&conn, path)?;

    Ok(conn)
}

/// Open and validate an existing current-schema database without mutation.
pub fn open_read_only(path: impl AsRef<Path>) -> Result<Connection> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(Error::DatabaseNotFound(path.to_string_lossy().to_string()));
    }

    validate_wal_file(path)?;
    let wal_path = database_wal_path(path);
    if wal_path.metadata().is_ok_and(|metadata| metadata.len() > 0) {
        return Err(Error::ConflictError(format!(
            "read-only database inspection requires a checkpointed WAL; active frames remain in {}",
            wal_path.display()
        )));
    }
    let immutable_path = path.canonicalize()?;
    let mut uri = url::Url::from_file_path(&immutable_path).map_err(|_| {
        Error::ConfigError(format!(
            "database path {} cannot be represented as an immutable SQLite URI",
            immutable_path.display()
        ))
    })?;
    uri.query_pairs_mut().append_pair("immutable", "1");
    let conn = Connection::open_with_flags(
        uri.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.execute_batch(READ_ONLY_CONNECTION_PRAGMAS)?;
    schema::require_current(&conn)?;
    remi_universe::attach_active_index(&conn, path)?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    Ok(conn)
}

/// Open an existing Conary database without revalidating its schema epoch.
///
/// This is identical to [`open`] but skips [`schema::ensure_current`], making
/// it faster for server hot paths. An owning startup path must already have
/// validated the current schema epoch through [`open`] or [`init`].
///
/// # Arguments
///
/// * `path` - Path to the database file
///
/// # Returns
///
/// * `Result<Connection>` - Database connection if successful
pub fn open_fast(path: impl AsRef<Path>) -> Result<Connection> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(Error::DatabaseNotFound(path.to_string_lossy().to_string()));
    }

    validate_wal_file(path)?;
    let conn = Connection::open(path)?;
    configure(&conn)?;
    remi_universe::attach_active_index(&conn, path)?;

    Ok(conn)
}

/// Execute a function within a transaction
///
/// If the function returns Ok, the transaction is committed.
/// If it returns Err, the transaction is rolled back.
///
/// # Arguments
///
/// * `conn` - Database connection
/// * `f` - Function to execute within the transaction
///
/// # Returns
///
/// * `Result<T>` - Result of the function
pub fn transaction<T, F>(conn: &mut Connection, f: F) -> Result<T>
where
    F: FnOnce(&rusqlite::Transaction) -> Result<T>,
{
    let tx = conn.transaction()?;
    let result = f(&tx)?;
    tx.commit()?;
    Ok(result)
}

/// Shared test utilities for database tests across the crate.
///
/// Provides [`create_test_db`] to eliminate the duplicated
/// `NamedTempFile` + `Connection::open` + `PRAGMA foreign_keys` +
/// `schema::ensure_current` boilerplate that appears in 20+ test modules.
#[cfg(test)]
pub(crate) mod testing {
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    use super::schema;

    /// Create a temporary SQLite database with the full Conary schema applied.
    ///
    /// Returns the `NamedTempFile` (which must be kept alive to prevent
    /// cleanup) and an open `Connection` with foreign keys enabled.
    pub fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();
        (temp_file, conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn directory_snapshot(path: &Path) -> Vec<(String, u64, String)> {
        let mut entries = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let bytes = std::fs::read(entry.path()).unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    bytes.len() as u64,
                    crate::hash::sha256(&bytes),
                )
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    #[test]
    fn test_init_creates_database() {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap().to_string();

        // Remove the temp file so init can create it
        drop(temp_file);

        let result = init(&db_path);
        assert!(result.is_ok());
        assert!(Path::new(&db_path).exists());
    }

    #[test]
    fn test_open_existing_database() {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap();

        // Initialize first
        init(db_path).unwrap();

        // Then open
        let result = open(db_path);
        assert!(result.is_ok());
    }

    #[test]
    fn open_read_only_validates_current_schema_without_database_side_effects() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("conary.db");
        init(&db_path).unwrap();
        let before = directory_snapshot(directory.path());

        let conn = open_read_only(&db_path).unwrap();
        let version: i32 = conn
            .query_row(
                "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, schema::SCHEMA_VERSION);
        assert!(
            conn.execute("INSERT INTO schema_version (version) VALUES (999)", [])
                .is_err()
        );
        assert_eq!(directory_snapshot(directory.path()), before);
        drop(conn);
        assert_eq!(directory_snapshot(directory.path()), before);
    }

    #[test]
    fn test_open_nonexistent_database() {
        let result = open("/nonexistent/path/db.sqlite");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::DatabaseNotFound(_)));
    }

    #[test]
    fn test_open_fast_uses_startup_validated_schema() {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap();

        // The owning startup path creates or validates the current schema.
        init(db_path).unwrap();

        // A hot-path open does not repeat that schema validation.
        let conn = open_fast(db_path).unwrap();

        // Verify the schema version is correct (tracked in schema_version table)
        let version: i32 = conn
            .query_row(
                "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            version,
            schema::SCHEMA_VERSION,
            "Schema version should match SCHEMA_VERSION after init()"
        );

        // Verify the current schema is available through the hot-path connection.
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='troves'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1, "troves table should exist");
    }

    #[test]
    fn test_open_rejects_corrupt_wal_sidecar() {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_path_buf();

        init(&db_path).unwrap();

        // Write corruption to the actual WAL sidecar path
        let mut wal_path = db_path.as_os_str().to_os_string();
        wal_path.push("-wal");
        std::fs::write(std::path::PathBuf::from(&wal_path), b"corrupt wal").unwrap();

        let err = open(&db_path).unwrap_err().to_string();
        assert!(err.contains("WAL appears corrupted") || err.contains("WAL"));
    }

    #[test]
    fn wal_validation_accepts_missing_and_empty_sidecars() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("conary.db");
        let wal_path = database_wal_path(&db_path);

        validate_wal_file(&db_path).unwrap();
        std::fs::write(&wal_path, []).unwrap();
        validate_wal_file(&db_path).unwrap();
    }

    #[test]
    fn wal_validation_uses_the_open_descriptor_after_unlink() {
        let directory = tempfile::tempdir().unwrap();
        let wal_path = directory.path().join("conary.db-wal");
        let mut bytes = [0_u8; SQLITE_WAL_HEADER_SIZE as usize];
        bytes[..4].copy_from_slice(&SQLITE_WAL_MAGIC_BE[0].to_be_bytes());
        std::fs::write(&wal_path, bytes).unwrap();

        let file = File::open(&wal_path).unwrap();
        std::fs::remove_file(&wal_path).unwrap();

        validate_open_wal_file(&wal_path, file).unwrap();
    }

    #[test]
    fn wal_validation_rejects_a_full_header_with_invalid_magic() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("conary.db");
        std::fs::write(
            database_wal_path(&db_path),
            [0_u8; SQLITE_WAL_HEADER_SIZE as usize],
        )
        .unwrap();

        let error = validate_wal_file(&db_path).unwrap_err().to_string();
        assert!(error.contains("invalid header"), "{error}");
    }
}
