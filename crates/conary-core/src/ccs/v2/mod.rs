// conary-core/src/ccs/v2/mod.rs
//! CCS v2 native package authority.

pub mod legacy;

pub use legacy::{ManifestFormatClassification, classify_manifest_format};
