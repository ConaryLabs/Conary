// conary-core/src/corpus/failure.rs

//! Typed conversion-failure taxonomy for corpus reporting.
//!
//! The governing rule is that a failure's category comes from the concrete
//! error type that produced it, never from the wording of its message. Human
//! text stays attached as a diagnostic and is never a classifier, so rephrasing
//! an error cannot move a case between buckets or change a gate result.
//!
//! Classification walks an `anyhow` cause chain and downcasts each link. An
//! error whose concrete type carries no category maps to
//! [`ConversionFailure::InternalUnclassified`] with a deterministic incident id
//! derived from the type chain. That is a visible, countable defect rather than
//! a guess: a rising unclassified count means the underlying errors are too
//! coarse to report on, which is the condition the source-authority work exists
//! to remove.

use crate::error::Error;
use serde::{Deserialize, Serialize};

/// Why a case failed, categorized by the authority that rejected it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversionFailure {
    /// Signature, key, or repository-authentication authority rejected the input.
    Trust { detail: String },
    /// The source archive container itself was unreadable or malformed.
    SourceArchive { detail: String },
    /// Archive framing was readable but its declared metadata was rejected.
    SourceMetadata { detail: String },
    /// A source semantic exists that Conary does not yet model exactly.
    ///
    /// This is a required implementation defect, never a permanent unsupported
    /// class or a human-review queue.
    UnsupportedSemantic { detail: String },
    /// A declared or observed resource bound was exceeded.
    ResourceBudget { detail: String },
    /// Generated CCS authority failed its own structural or signing contract.
    CcsAuthority { detail: String },
    /// Publication or serving rejected an otherwise valid artifact.
    Publication { detail: String },
    /// Transaction planning, mutation, or rollback failed.
    Transaction { detail: String },
    /// No concrete type in the chain carries a category.
    ///
    /// `incident_id` is derived from the chain's type names, so the same
    /// unclassified error type always produces the same id. That makes these
    /// aggregate into countable buckets and correlate with logs without any
    /// message-text comparison.
    InternalUnclassified { incident_id: String, detail: String },
}

/// Stable machine-readable category code.
///
/// Reports, aggregation keys, and wire payloads use this. It is deliberately
/// separate from the human `detail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Trust,
    SourceArchive,
    SourceMetadata,
    UnsupportedSemantic,
    ResourceBudget,
    CcsAuthority,
    Publication,
    Transaction,
    InternalUnclassified,
}

impl FailureKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            FailureKind::Trust => "trust",
            FailureKind::SourceArchive => "source_archive",
            FailureKind::SourceMetadata => "source_metadata",
            FailureKind::UnsupportedSemantic => "unsupported_semantic",
            FailureKind::ResourceBudget => "resource_budget",
            FailureKind::CcsAuthority => "ccs_authority",
            FailureKind::Publication => "publication",
            FailureKind::Transaction => "transaction",
            FailureKind::InternalUnclassified => "internal_unclassified",
        }
    }

    /// Every category, for exhaustive report scaffolding.
    pub const ALL: [FailureKind; 9] = [
        FailureKind::Trust,
        FailureKind::SourceArchive,
        FailureKind::SourceMetadata,
        FailureKind::UnsupportedSemantic,
        FailureKind::ResourceBudget,
        FailureKind::CcsAuthority,
        FailureKind::Publication,
        FailureKind::Transaction,
        FailureKind::InternalUnclassified,
    ];
}

impl ConversionFailure {
    pub fn kind(&self) -> FailureKind {
        match self {
            ConversionFailure::Trust { .. } => FailureKind::Trust,
            ConversionFailure::SourceArchive { .. } => FailureKind::SourceArchive,
            ConversionFailure::SourceMetadata { .. } => FailureKind::SourceMetadata,
            ConversionFailure::UnsupportedSemantic { .. } => FailureKind::UnsupportedSemantic,
            ConversionFailure::ResourceBudget { .. } => FailureKind::ResourceBudget,
            ConversionFailure::CcsAuthority { .. } => FailureKind::CcsAuthority,
            ConversionFailure::Publication { .. } => FailureKind::Publication,
            ConversionFailure::Transaction { .. } => FailureKind::Transaction,
            ConversionFailure::InternalUnclassified { .. } => FailureKind::InternalUnclassified,
        }
    }

    /// Human-readable diagnostic. Never a classifier.
    pub fn detail(&self) -> &str {
        match self {
            ConversionFailure::Trust { detail }
            | ConversionFailure::SourceArchive { detail }
            | ConversionFailure::SourceMetadata { detail }
            | ConversionFailure::UnsupportedSemantic { detail }
            | ConversionFailure::ResourceBudget { detail }
            | ConversionFailure::CcsAuthority { detail }
            | ConversionFailure::Publication { detail }
            | ConversionFailure::Transaction { detail }
            | ConversionFailure::InternalUnclassified { detail, .. } => detail,
        }
    }

    /// Classify a typed `conary-core` error by its variant.
    ///
    /// Variants that cannot be attributed to one authority stay unclassified
    /// rather than being resolved by inspecting their message. `ParseError`, for
    /// example, is raised by both archive framing and metadata decoding, so it
    /// is deliberately not forced into either bucket here.
    pub fn from_core_error(error: &Error) -> Self {
        let detail = error.to_string();
        match error {
            Error::GpgVerificationFailed(_) => ConversionFailure::Trust { detail },
            Error::ChecksumMismatch { .. } => ConversionFailure::SourceArchive { detail },
            Error::DownloadError(_) | Error::HttpStatus { .. } | Error::TimeoutError(_) => {
                ConversionFailure::Publication { detail }
            }
            Error::VersionParse(_) | Error::VersionComparison(_) => {
                ConversionFailure::SourceMetadata { detail }
            }
            Error::NotImplemented(_) | Error::Capability(_) => {
                ConversionFailure::UnsupportedSemantic { detail }
            }
            Error::PathTraversal(_) | Error::InvalidPath(_) => {
                ConversionFailure::SourceMetadata { detail }
            }
            Error::ConflictError(_)
            | Error::ResolutionError(_)
            | Error::ScriptletExecution { .. }
            | Error::TriggerError(_)
            | Error::RecoveryFailed(_) => ConversionFailure::Transaction { detail },
            other => ConversionFailure::InternalUnclassified {
                incident_id: incident_id_for(&[core_variant_name(other)]),
                detail,
            },
        }
    }

    /// Classify an `anyhow` error by walking its cause chain and downcasting.
    ///
    /// Only concrete types are inspected. The first link carrying a category
    /// wins, so context wrapping does not hide the originating authority.
    pub fn classify(error: &anyhow::Error) -> Self {
        for cause in error.chain() {
            if let Some(core) = cause.downcast_ref::<Error>() {
                let classified = ConversionFailure::from_core_error(core);
                if classified.kind() != FailureKind::InternalUnclassified {
                    return classified;
                }
            }
        }

        // Nothing in the chain carries a category. Record the chain's concrete
        // type names so identical failures aggregate together and can be found
        // in logs, without ever comparing message text.
        let type_names: Vec<&'static str> = error
            .chain()
            .map(|cause| {
                cause
                    .downcast_ref::<Error>()
                    .map(core_variant_name)
                    .unwrap_or("opaque")
            })
            .collect();

        ConversionFailure::InternalUnclassified {
            incident_id: incident_id_for(&type_names),
            detail: error.to_string(),
        }
    }
}

/// Variant name of a core error. Structural, not its rendered message.
fn core_variant_name(error: &Error) -> &'static str {
    match error {
        Error::Database(_) => "Database",
        Error::Io(_) => "Io",
        Error::IoError(_) => "IoError",
        Error::InitError(_) => "InitError",
        Error::MissingId(_) => "MissingId",
        Error::VersionParse(_) => "VersionParse",
        Error::VersionComparison(_) => "VersionComparison",
        Error::HashError(_) => "HashError",
        Error::ConfigError(_) => "ConfigError",
        Error::DatabaseNotFound(_) => "DatabaseNotFound",
        Error::DownloadError(_) => "DownloadError",
        Error::HttpStatus { .. } => "HttpStatus",
        Error::ConflictError(_) => "ConflictError",
        Error::AmbiguousPackageSelection { .. } => "AmbiguousPackageSelection",
        Error::ChecksumMismatch { .. } => "ChecksumMismatch",
        Error::ParseError(_) => "ParseError",
        Error::DeltaError(_) => "DeltaError",
        Error::GpgVerificationFailed(_) => "GpgVerificationFailed",
        Error::ScriptletExecution { .. } => "ScriptletExecution",
        Error::TriggerError(_) => "TriggerError",
        Error::AlreadyExists(_) => "AlreadyExists",
        Error::InvalidPath(_) => "InvalidPath",
        Error::PathTraversal(_) => "PathTraversal",
        Error::NotFound(_) => "NotFound",
        Error::RecoveryFailed(_) => "RecoveryFailed",
        Error::TimeoutError(_) => "TimeoutError",
        Error::ResolutionError(_) => "ResolutionError",
        Error::NotImplemented(_) => "NotImplemented",
        Error::Json(_) => "Json",
        Error::Capability(_) => "Capability",
        _ => "Other",
    }
}

/// Deterministic short id over a chain of type names.
fn incident_id_for(type_names: &[&str]) -> String {
    let joined = type_names.join(">");
    crate::hash::sha256(joined.as_bytes())[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_variants_map_to_their_owning_authority() {
        assert_eq!(
            ConversionFailure::from_core_error(&Error::GpgVerificationFailed("k".into())).kind(),
            FailureKind::Trust
        );
        assert_eq!(
            ConversionFailure::from_core_error(&Error::NotImplemented("x".into())).kind(),
            FailureKind::UnsupportedSemantic
        );
        assert_eq!(
            ConversionFailure::from_core_error(&Error::PathTraversal("p".into())).kind(),
            FailureKind::SourceMetadata
        );
        assert_eq!(
            ConversionFailure::from_core_error(&Error::ResolutionError("r".into())).kind(),
            FailureKind::Transaction
        );
    }

    /// The central invariant: wording is a diagnostic, never a classifier.
    #[test]
    fn rewording_an_error_cannot_change_its_category() {
        let first = ConversionFailure::from_core_error(&Error::GpgVerificationFailed(
            "signature from unknown key".into(),
        ));
        let second = ConversionFailure::from_core_error(&Error::GpgVerificationFailed(
            "totally different wording here".into(),
        ));

        assert_eq!(first.kind(), second.kind());
        assert_ne!(
            first.detail(),
            second.detail(),
            "the diagnostic must still differ, proving the category ignored it"
        );
    }

    /// A message that reads like another category must not be reclassified.
    #[test]
    fn message_text_resembling_another_category_does_not_move_the_bucket() {
        let misleading =
            ConversionFailure::from_core_error(&Error::ResolutionError("trust failure".into()));

        assert_eq!(
            misleading.kind(),
            FailureKind::Transaction,
            "classification must follow the variant, not the words in the message"
        );
    }

    #[test]
    fn classify_finds_the_category_through_context_wrapping() {
        let wrapped = anyhow::Error::from(Error::GpgVerificationFailed("bad sig".into()))
            .context("while converting package")
            .context("while running prewarm");

        assert_eq!(
            ConversionFailure::classify(&wrapped).kind(),
            FailureKind::Trust,
            "context layers must not hide the originating authority"
        );
    }

    #[test]
    fn opaque_errors_are_unclassified_rather_than_guessed() {
        let opaque = anyhow::anyhow!("something went wrong in an untyped path");

        let failure = ConversionFailure::classify(&opaque);

        assert_eq!(failure.kind(), FailureKind::InternalUnclassified);
        match failure {
            ConversionFailure::InternalUnclassified { incident_id, .. } => {
                assert_eq!(incident_id.len(), 16);
            }
            other => panic!("expected InternalUnclassified, got {other:?}"),
        }
    }

    /// Unclassified failures must aggregate, so the id depends on the type
    /// chain and not on the message.
    #[test]
    fn identical_unclassified_type_chains_share_one_incident_id() {
        let first = ConversionFailure::classify(&anyhow::anyhow!("first wording"));
        let second = ConversionFailure::classify(&anyhow::anyhow!("second wording entirely"));

        match (first, second) {
            (
                ConversionFailure::InternalUnclassified {
                    incident_id: a,
                    detail: da,
                },
                ConversionFailure::InternalUnclassified {
                    incident_id: b,
                    detail: db,
                },
            ) => {
                assert_eq!(a, b, "same type chain must share an incident id");
                assert_ne!(da, db, "details must still differ");
            }
            other => panic!("expected two unclassified failures, got {other:?}"),
        }
    }

    #[test]
    fn failure_kind_codes_are_stable_and_unique() {
        let mut codes: Vec<&str> = FailureKind::ALL.iter().map(|k| k.as_str()).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total, "category codes must be unique");
        assert_eq!(FailureKind::Trust.as_str(), "trust");
    }

    #[test]
    fn failures_round_trip_through_serde_by_category_code() {
        let failure = ConversionFailure::SourceArchive {
            detail: "truncated payload stream".into(),
        };

        let json = serde_json::to_value(&failure).expect("serialize");
        assert_eq!(json["kind"], serde_json::json!("source_archive"));

        let restored: ConversionFailure = serde_json::from_value(json).expect("deserialize");
        assert_eq!(restored, failure);
    }
}
