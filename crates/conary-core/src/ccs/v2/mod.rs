// conary-core/src/ccs/v2/mod.rs
//! CCS v2 native package authority.

pub mod diagnostics;
pub mod legacy;
pub mod schema;
pub mod validation;

pub use diagnostics::{V2Diagnostic, V2DiagnosticCode, V2ValidationError};
pub use legacy::{ManifestFormatClassification, classify_manifest_format};
pub use schema::{
    AuthorityDocumentV2, DependencyEntryV2, FORMAT_VERSION_V2, PackageKindTagV2, PackageKindV2,
};
pub use validation::{
    M4aNoProfileFacts, ProfileConstraintStatus, TargetProfileQuery, validate_authority,
    validate_authority_with_profile,
};
