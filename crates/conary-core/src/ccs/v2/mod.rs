// conary-core/src/ccs/v2/mod.rs
//! CCS v2 native package authority.

pub mod legacy;
pub mod schema;

pub use legacy::{ManifestFormatClassification, classify_manifest_format};
pub use schema::{
    AuthorityDocumentV2, DependencyEntryV2, FORMAT_VERSION_V2, PackageKindTagV2, PackageKindV2,
};
