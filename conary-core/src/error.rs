// conary-core/src/error.rs

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

    /// Missing ID on model object (required for update/query operations)
    #[error("Missing ID: {0}")]
    MissingId(String),

    /// Version parse error
    #[error("Version parse error: {0}")]
    VersionParse(String),

    /// Hash error
    #[error("Hash error: {0}")]
    HashError(#[from] crate::hash::HashError),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Database not found
    #[error("Database not found at path: {0}")]
    DatabaseNotFound(String),

    /// Download error
    #[error("Download failed: {0}")]
    DownloadError(String),

    /// Resource conflict (e.g., duplicate name)
    #[error("Conflict: {0}")]
    ConflictError(String),

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

    /// Trigger execution error
    #[error("Trigger error: {0}")]
    TriggerError(String),

    /// Path already exists
    #[error("Already exists: {0}")]
    AlreadyExists(String),

    /// Invalid path
    #[error("Invalid path: {0}")]
    InvalidPath(String),

    /// Path traversal attempt (security violation)
    #[error("Path traversal detected: {0}")]
    PathTraversal(String),

    /// Resource not found (generic)
    #[error("Not found: {0}")]
    NotFound(String),

    /// Transaction recovery failed
    #[error("Recovery failed: {0}")]
    RecoveryFailed(String),

    /// Operation timed out
    #[error("Timeout: {0}")]
    TimeoutError(String),

    /// Package resolution error
    #[error("Resolution error: {0}")]
    ResolutionError(String),

    /// Feature not yet implemented
    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// JSON serialization/deserialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Capability-related errors
    #[error("Capability error: {0}")]
    Capability(String),

    /// Federation-related errors
    #[error("Federation error: {0}")]
    Federation(String),

    /// Operation was cancelled
    #[error("Operation cancelled: {0}")]
    Cancelled(String),

    /// Internal error (invariant violation, corrupted state)
    #[error("Internal error: {0}")]
    InternalError(String),

    /// TUF trust verification error
    #[error("Trust error: {0}")]
    TrustError(String),

    /// Resolver pool overflow (too many interned items for u32 index)
    #[error("Resolver pool overflow: {0}")]
    PoolOverflow(String),
}

/// Result type alias using Conary's Error type
pub type Result<T> = std::result::Result<T, Error>;
