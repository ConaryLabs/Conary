// conary-core/src/db/mod.rs

//! Database layer for Conary
//!
//! This module handles all SQLite operations including:
//! - Database initialization and schema creation
//! - Connection management
//! - Transaction handling
//! - CRUD operations for troves, changesets, files, etc.

pub mod migrations;
pub mod models;
pub mod paths;
pub mod schema;

use crate::error::{Error, Result};
use rusqlite::Connection;
use std::fs::File;
use std::io::Read;
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

const SQLITE_WAL_HEADER_SIZE: u64 = 32;
const SQLITE_WAL_MAGIC_BE: [u32; 2] = [0x377f0682, 0x377f0683];

/// Apply standard PRAGMAs to a connection
fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch(CONNECTION_PRAGMAS)?;
    Ok(())
}

fn validate_wal_file(path: &Path) -> Result<()> {
    let wal_path = path.with_extension(format!(
        "{}-wal",
        path.extension().and_then(|ext| ext.to_str()).unwrap_or_default()
    ));
    if !wal_path.exists() {
        return Ok(());
    }

    let metadata = std::fs::metadata(&wal_path)?;
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
    let mut file = File::open(&wal_path)?;
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
    schema::migrate(&conn)?;

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
    schema::migrate(&conn)?;

    Ok(conn)
}

/// Open an existing Conary database without running migrations
///
/// This is identical to [`open`] but skips `schema::migrate()`, making it
/// faster for server hot paths where the schema is already known-good from
/// startup. The caller is responsible for ensuring migrations have already
/// been applied (e.g., via a prior `open()` or `init()` call).
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

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
    fn test_open_nonexistent_database() {
        let result = open("/nonexistent/path/db.sqlite");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::DatabaseNotFound(_)));
    }

    #[test]
    fn test_open_fast_skips_migration() {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap();

        // Initialize the database (runs migrations)
        init(db_path).unwrap();

        // Open with open_fast (no migration check)
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

        // Verify we can query a table that only exists after migration
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
        std::fs::write(db_path.with_extension("db-wal"), b"corrupt wal").unwrap();

        let err = open(&db_path).unwrap_err().to_string();
        assert!(err.contains("WAL appears corrupted") || err.contains("WAL"));
    }
}
