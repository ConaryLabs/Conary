// src/ccs/enhancement/error.rs
//! Error types for the enhancement framework

use thiserror::Error;

/// Result type for enhancement operations
pub type EnhancementResult<T> = std::result::Result<T, EnhancementError>;

/// Errors that can occur during package enhancement
#[derive(Error, Debug)]
pub enum EnhancementError {
    /// Database operation failed
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Package not found in converted_packages table
    #[error("converted package not found: trove_id={0}")]
    PackageNotFound(i64),

    /// Enhancement already in progress for this package
    #[error("enhancement already in progress for trove_id={0}")]
    AlreadyInProgress(i64),

    /// Capability inference failed
    #[error("capability inference failed: {0}")]
    InferenceFailed(String),

    /// Provenance extraction failed
    #[error("provenance extraction failed: {0}")]
    ProvenanceFailed(String),

    /// No enhancer registered for this type
    #[error("no enhancer registered for type: {0}")]
    NoEnhancer(String),

    /// Enhancement was cancelled
    #[error("enhancement cancelled")]
    Cancelled,

    /// IO error during file operations
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Generic enhancement error
    #[error("{0}")]
    Other(String),
}

impl EnhancementError {
    /// Create a new "other" error with a message
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
