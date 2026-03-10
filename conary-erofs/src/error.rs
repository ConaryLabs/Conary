// conary-erofs/src/error.rs
//! Error types for the EROFS image builder.

/// Errors that can occur during EROFS image construction or validation.
#[derive(Debug, thiserror::Error)]
pub enum ErofsError {
    /// Block size is not a power of two or is less than 512.
    #[error("invalid block size: must be a power of two >= 512, got {0}")]
    InvalidBlockSize(u32),

    /// Path contains a `..` traversal component.
    #[error("path contains traversal component: {0}")]
    PathTraversal(String),

    /// A numeric value exceeds its expected range.
    #[error("value out of range: {0}")]
    OutOfRange(String),

    /// An I/O error from the underlying writer or reader.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
