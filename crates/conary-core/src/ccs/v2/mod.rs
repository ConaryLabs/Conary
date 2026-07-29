// conary-core/src/ccs/v2/mod.rs
//! CCS v2 native package authority.

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
    ProjectedV2Package, V2AuthoringInput, project_build_result_authority_to_v2,
    project_build_result_to_v2,
};
pub use diagnostics::{V2Diagnostic, V2DiagnosticCode, V2ValidationError};
pub use identity::{
    ContentIdentityProjectionV2, compute_v2_content_identity, compute_v2_file_merkle_root,
};
pub(crate) use manifest_projection::project_manifest_identity;
pub use reader::{ReadAuthorityV2, read_authority_document};
pub use schema::{
    AuthorityDocumentV2, DependencyEntryV2, FORMAT_VERSION_V2, PackageKindTagV2, PackageKindV2,
    ProvidedCapabilityV2,
};
pub use validation::validate_authority;
pub use validation::{authority_census, validate_authority_structure};
