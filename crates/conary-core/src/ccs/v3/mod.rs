// conary-core/src/ccs/v3/mod.rs
//! CCS v3 native package authority.

pub mod authoring;
pub mod component_view;
pub mod debug_projection;
pub mod diagnostics;
pub(crate) mod file_capabilities;
pub mod identity;
pub(crate) mod lifecycle;
pub(crate) mod manifest_projection;
pub mod reader;
pub mod schema;
#[cfg(test)]
pub(crate) mod test_support;
pub mod validation;

pub use authoring::{
    ProjectedV3Package, V3AuthoringInput, project_build_result_authority_to_v3,
    project_build_result_to_v3,
};
pub use diagnostics::{V3Diagnostic, V3DiagnosticCode, V3ValidationError};
pub use identity::{
    ContentIdentityProjectionV3, compute_v3_content_identity, compute_v3_file_merkle_root,
};
pub(crate) use manifest_projection::project_manifest_identity;
pub use reader::{ReadAuthorityV3, read_authority_document};
pub use schema::{
    AuthorityDocumentV3, DependencyEntryV3, FORMAT_VERSION_V3, PackageKindTagV3, PackageKindV3,
    ProvidedCapabilityV3,
};
pub use validation::validate_authority;
pub use validation::{authority_census, validate_authority_structure};
