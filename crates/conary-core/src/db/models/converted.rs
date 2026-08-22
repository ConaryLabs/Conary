// conary-core/src/db/models/converted.rs

//! Converted package tracking model
//!
//! Tracks packages converted from native formats (RPM/DEB/Arch/eopkg) to CCS.
//! This enables:
//! - Skip re-conversion of same package artifact (checksum-based dedup)
//! - Persist typed lifecycle evidence
//! - Re-convert when conversion algorithm is upgraded

use strum_macros::{AsRefStr, Display, EnumString};

mod enhancement;
mod persistence;
mod repository;
mod validation;

/// Current conversion algorithm version
/// Bump this when making changes that require re-conversion of existing packages.
///
/// Revision 16 cuts the persisted CCS scriptlet contract: the redundant
/// `RpmRuntimeMetadata.critical` boolean leaves the contract entirely
/// (`deny_unknown_fields` rejects old manifests naming it), and
/// `NativeLifecycleEntry.native_slot` persists the typed `RpmScriptletSlot`
/// class with exact wire strings instead of a free string.
pub const CONVERSION_VERSION: i32 = 16;
/// Canonical digest of an empty repository-provide projection.
pub const EMPTY_REPOSITORY_PROVIDES_DIGEST: &str =
    "sha256:4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945";

/// Storage and ownership boundary for a converted package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum ConvertedArtifactKind {
    /// A conversion attached to an installed Conary trove.
    Installed,
    /// A Remi repository artifact with complete serving identity and storage.
    Repository,
}

/// Validated repository-serving view of a converted package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryConvertedArtifact<'a> {
    pub package_name: &'a str,
    pub package_version: &'a str,
    pub source_profile: &'a str,
    pub profile_revision_sha256: &'a str,
    pub package_architecture: &'a str,
    pub repository_provides_digest: Option<&'a str>,
    pub transport: crate::ccs::transport::CcsTransportEnvelopeV1,
    pub total_size: u64,
    pub content_hash: &'a str,
    pub ccs_path: &'a str,
}

/// A converted package record
#[derive(Debug, Clone)]
pub struct ConvertedPackage {
    pub id: Option<i64>,
    pub artifact_kind: ConvertedArtifactKind,
    /// Reference to the converted trove (CCS package that was installed)
    pub trove_id: Option<i64>,
    /// Original package format (rpm, deb, arch)
    pub original_format: String,
    /// Exact repository checksum identity for repository artifacts, or the
    /// verified native artifact checksum for installed conversions.
    pub original_checksum: String,
    /// Exact immutable profile revision that supplied a repository conversion.
    /// Installed conversions deliberately carry no profile revision.
    pub profile_revision_sha256: Option<String>,
    /// Optional diagnostic digest of the repository-provide projection. It is
    /// retained for indexed metadata but never decides conversion currentness.
    pub repository_provides_digest: Option<String>,
    /// Conversion algorithm version (re-convert if upgraded)
    pub conversion_version: i32,
    /// When the conversion occurred
    pub converted_at: Option<String>,

    /// Enhancement algorithm version (0 = not enhanced yet)
    pub enhancement_version: i32,
    /// Extracted provenance JSON (before DB insertion)
    pub extracted_provenance_json: Option<String>,
    /// Enhancement status: pending, in_progress, complete, failed, skipped
    pub enhancement_status: String,
    /// Error message if enhancement failed
    pub enhancement_error: Option<String>,
    /// When enhancement was last attempted
    pub enhancement_attempted_at: Option<String>,

    // Repository conversion identity plus durable installed-conversion output.
    /// Package name (for repository lookups)
    pub package_name: Option<String>,
    /// Package version (for server-side lookups)
    pub package_version: Option<String>,
    /// Exact public source profile for this repository conversion.
    pub source_profile: Option<String>,
    /// Native package architecture for server-side conversion cache identity.
    pub package_architecture: Option<String>,
    /// Versioned signed-control and exact-object repository transport.
    pub transport_json: Option<String>,
    /// Total size of the CCS package
    pub total_size: Option<i64>,
    /// Content hash of the CCS package
    pub content_hash: Option<String>,
    /// Path to the CCS package file. Repository records require it; installed
    /// records may retain the verified adopted-package conversion output.
    pub ccs_path: Option<String>,

    // Foreign-package scriptlet evidence fields.
    /// Aggregate scriptlet fidelity from passive bundle construction.
    pub scriptlet_fidelity: String,
    /// Digest of normalized scriptlet evidence.
    pub evidence_digest: Option<String>,
    /// JSON-encoded typed scriptlet summary for API/index projection.
    pub scriptlet_summary_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkConversionState {
    NoConvertedReference,
    CurrentConversion,
    StaleConversionOnly,
}

#[cfg(test)]
mod tests;
