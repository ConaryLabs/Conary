// conary-core/src/trust/mod.rs

//! TUF (The Update Framework) supply chain trust
//!
//! Implements the TUF specification for securing package repository metadata.
//! This provides protection against:
//! - Rollback attacks (downgrading to older vulnerable versions)
//! - Freeze attacks (serving stale metadata indefinitely)
//! - Arbitrary package attacks (replacing packages with malicious ones)
//! - Mix-and-match attacks (combining metadata from different versions)

pub mod ceremony;
pub mod client;
#[cfg(feature = "server")]
pub mod generate;
pub mod keys;
pub mod metadata;
pub mod verify;

pub use keys::{canonical_json, compute_key_id, sign_tuf_metadata, signing_keypair_to_tuf_key};
pub use metadata::{
    KeyVal, MetaFile, Role, RoleDefinition, RootMetadata, Signed, SnapshotMetadata,
    TUF_SPEC_VERSION, TargetDescription, TargetsMetadata, TimestampMetadata, TufKey, TufSignature,
};

/// Errors specific to TUF trust operations
#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    /// TUF verification failed
    #[error("TUF verification failed: {0}")]
    VerificationFailed(String),

    /// TUF metadata has expired
    #[error("TUF metadata expired: {role} expired at {expires}")]
    MetadataExpired {
        /// The role whose metadata expired
        role: String,
        /// The expiration timestamp
        expires: String,
    },

    /// Rollback attack detected (version going backwards)
    #[error("TUF rollback detected: {role} version {new} <= stored {stored}")]
    RollbackAttack {
        /// The role being updated
        role: String,
        /// The new version presented
        new: u64,
        /// The stored version
        stored: u64,
    },

    /// Signature threshold not met
    #[error("TUF threshold not met: {role} needs {threshold} signatures, got {got}")]
    ThresholdNotMet {
        /// The role being verified
        role: String,
        /// The required threshold
        threshold: u64,
        /// The number of valid signatures found
        got: u64,
    },

    /// Key-related error
    #[error("TUF key error: {0}")]
    KeyError(String),

    /// Error fetching TUF metadata
    #[error("TUF fetch error: {0}")]
    FetchError(String),

    /// Consistency check failed
    #[error("TUF consistency error: {0}")]
    ConsistencyError(String),

    /// TUF is not enabled for this repository
    #[error("TUF not enabled for repository")]
    NotEnabled,

    /// Database error
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// JSON serialization/deserialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// I/O error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for TUF operations
pub type TrustResult<T> = std::result::Result<T, TrustError>;
