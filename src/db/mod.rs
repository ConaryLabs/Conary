// src/db/mod.rs

//! Database layer for Conary
//!
//! This module handles all SQLite operations including:
//! - Database initialization and schema creation
//! - Connection management
//! - Transaction handling
//! - CRUD operations for troves, changesets, files, etc.

mod migrations;
pub mod models;
pub mod schema;

use crate::error::{Error, Result};
use rusqlite::Connection;
use std::path::Path;
use tracing::{debug, info};

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
pub fn init(db_path: &str) -> Result<()> {
    debug!("Initializing database at: {}", db_path);

    // Create parent directories if they don't exist
    if let Some(parent) = Path::new(db_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::InitError(format!("Failed to create database directory: {}", e)))?;
    }

    // Open/create the database
    let conn = Connection::open(db_path)?;

    // Set pragmas for better performance and reliability
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 5000;
        ",
    )?;

    // Run schema migrations to set up or upgrade the database
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
pub fn open(db_path: &str) -> Result<Connection> {
    if !Path::new(db_path).exists() {
        return Err(Error::DatabaseNotFound(db_path.to_string()));
    }

    let conn = Connection::open(db_path)?;

    // Set pragmas
    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 5000;
        ",
    )?;

    // Apply any pending migrations
    schema::migrate(&conn)?;

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
}
