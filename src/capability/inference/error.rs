// src/capability/inference/error.rs
//! Error types for capability inference

use thiserror::Error;

/// Errors that can occur during capability inference
#[derive(Error, Debug)]
pub enum InferenceError {
    /// Failed to parse binary file
    #[error("Failed to parse binary '{path}': {reason}")]
    BinaryParseError { path: String, reason: String },

    /// Binary format not supported
    #[error("Unsupported binary format for '{path}': {format}")]
    UnsupportedFormat { path: String, format: String },

    /// I/O error during inference
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Regex error
    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),

    /// Timeout during analysis
    #[error("Analysis timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// No files to analyze
    #[error("No files provided for analysis")]
    NoFiles,

    /// Other error
    #[error("{0}")]
    Other(String),
}
