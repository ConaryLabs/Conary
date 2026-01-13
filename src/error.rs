// src/error.rs

use thiserror::Error;

/// Core error types for Conary
#[derive(Error, Debug)]
pub enum Error {
    /// Database-related errors
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// I/O errors (automatic conversion)
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// I/O errors (manual)
    #[error("{0}")]
    IoError(String),

    /// Database initialization error
    #[error("Failed to initialize database: {0}")]
    InitError(String),

    /// Database not found
    #[error("Database not found at path: {0}")]
    DatabaseNotFound(String),

    /// Download error
    #[error("Download failed: {0}")]
    DownloadError(String),

    /// Resource conflict (e.g., duplicate name)
    #[error("Conflict: {0}")]
    ConflictError(String),

    /// Resource not found
    #[error("Not found: {0}")]
    NotFoundError(String),

    /// Checksum mismatch
    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    /// Parse error
    #[error("Parse error: {0}")]
    ParseError(String),

    /// Delta operation error
    #[error("Delta operation failed: {0}")]
    DeltaError(String),

    /// GPG signature verification failed
    #[error("GPG verification failed: {0}")]
    GpgVerificationFailed(String),

    /// Scriptlet execution error
    #[error("Scriptlet error: {0}")]
    ScriptletError(String),
}

/// Result type alias using Conary's Error type
pub type Result<T> = std::result::Result<T, Error>;
