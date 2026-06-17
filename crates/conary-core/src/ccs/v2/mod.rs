// conary-core/src/ccs/v2/mod.rs
//! CCS v2 native package authority.

pub mod diagnostics;
pub mod identity;
pub mod legacy;
pub mod reader;
pub mod schema;
#[cfg(test)]
pub(crate) mod test_support;
pub mod validation;

pub use diagnostics::{V2Diagnostic, V2DiagnosticCode, V2ValidationError};
pub use identity::{
    ContentIdentityProjectionV2, compute_v2_content_identity, compute_v2_file_merkle_root,
};
pub use legacy::{ManifestFormatClassification, classify_manifest_format};
pub use reader::{ReadAuthorityV2, read_authority_document};
pub use schema::{
    AuthorityDocumentV2, DependencyEntryV2, FORMAT_VERSION_V2, PackageKindTagV2, PackageKindV2,
};
pub use validation::{
    M4aNoProfileFacts, ProfileConstraintStatus, TargetProfileQuery, validate_authority,
    validate_authority_with_profile,
};
