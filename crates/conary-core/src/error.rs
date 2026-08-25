// conary-core/src/error.rs

use thiserror::Error;

/// Exact failure class attached where scriptlet execution fails.
///
/// This is transaction authority. Human-readable error text must never be
/// inspected to recover the class later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptletFailureKind {
    /// The script process ran and returned a non-zero status or signal.
    ScriptExited,
    /// The script process exceeded its configured timeout.
    ScriptTimedOut,
    /// A typed lifecycle command or scriptlet contract was malformed.
    ContractViolation,
    /// A required interpreter or exact lifecycle program was unavailable.
    ProgramUnavailable,
    /// Staging, spawning, waiting, or another process boundary failed.
    ProcessSetupFailed,
    /// Namespace, mount, root, or other sandbox setup failed.
    SandboxSetupUnavailable,
    /// Landlock, seccomp, or capability enforcement setup failed.
    EnforcementSetupFailed,
}

impl ScriptletFailureKind {
    /// Stable value for diagnostics and persisted changeset metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ScriptExited => "ScriptExited",
            Self::ScriptTimedOut => "ScriptTimedOut",
            Self::ContractViolation => "ContractViolation",
            Self::ProgramUnavailable => "ProgramUnavailable",
            Self::ProcessSetupFailed => "ProcessSetupFailed",
            Self::SandboxSetupUnavailable => "SandboxSetupUnavailable",
            Self::EnforcementSetupFailed => "EnforcementSetupFailed",
        }
    }

    /// Whether this failure is the source program's own result.
    ///
    /// Only a program that actually ran under the exact preflighted contract
    /// and the selected-root enforcement boundary produces a result the source
    /// format's failure table can speak about. Every other kind is a Conary
    /// contract, process, sandbox, or enforcement failure, so a source format's
    /// warning-only class must never suppress it.
    pub const fn is_source_program_result(self) -> bool {
        matches!(self, Self::ScriptExited | Self::ScriptTimedOut)
    }
}

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

    /// An existing database belongs to a retired pre-alpha schema.
    #[error(
        "Database schema rebuild required: database uses {observed}; this pre-alpha build supports only schema epoch {supported_epoch} revision {supported_revision}"
    )]
    SchemaRebuildRequired {
        observed: String,
        supported_epoch: String,
        supported_revision: i32,
    },

    /// Missing ID on model object (required for update/query operations)
    #[error("Missing ID: {0}")]
    MissingId(String),

    /// Version parse error
    #[error("Version parse error: {0}")]
    VersionParse(String),

    /// Exact native or Conary version parsing/comparison failure
    #[error(transparent)]
    VersionComparison(#[from] crate::repository::versioning::VersionComparisonError),

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

    /// A repository declared a chunk servable but its durable object authority
    /// could not provide that exact object.
    #[error("Durable chunk {hash} is unavailable from {authority}")]
    DurableChunkUnavailable { hash: String, authority: String },

    /// HTTP response status from an exact repository URL
    #[error("HTTP {status} from {url}")]
    HttpStatus { status: u16, url: String },

    /// Resource conflict (e.g., duplicate name)
    #[error("Conflict: {0}")]
    ConflictError(String),

    /// Eligible package candidates remain tied without a comparable version
    /// contract or an explicit repository/priority winner.
    #[error(
        "Ambiguous package selection for '{package}': {candidates:?}. \
         Select a repository explicitly or assign distinct repository priorities"
    )]
    AmbiguousPackageSelection {
        package: String,
        candidates: Vec<String>,
    },

    /// Checksum mismatch
    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    /// Parse error
    #[error("Parse error: {0}")]
    ParseError(String),

    /// Structural or operator-resource budget refusal.
    #[error(transparent)]
    Budget(#[from] crate::ccs::BudgetError),

    /// Immutable catalog construction cannot reserve exact finalization space.
    #[error(transparent)]
    CatalogScratchCapacity(#[from] crate::repository::catalog::CatalogScratchCapacityError),

    /// Delta operation error
    #[error("Delta operation failed: {0}")]
    DeltaError(String),

    /// GPG signature verification failed
    #[error("GPG verification failed: {0}")]
    GpgVerificationFailed(String),

    /// Scriptlet execution error with an exact failure class.
    #[error("Scriptlet error ({kind:?}): {message}")]
    ScriptletExecution {
        kind: ScriptletFailureKind,
        message: String,
    },

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

impl Error {
    /// Build a scriptlet error whose semantic class is independent of its text.
    pub fn scriptlet(kind: ScriptletFailureKind, message: impl Into<String>) -> Self {
        Self::ScriptletExecution {
            kind,
            message: message.into(),
        }
    }
}

impl From<crate::capability::CapabilityError> for Error {
    fn from(value: crate::capability::CapabilityError) -> Self {
        Self::Capability(value.to_string())
    }
}
