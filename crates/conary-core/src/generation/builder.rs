// conary-core/src/generation/builder.rs

//! Generation builder - creates EROFS images from system state.
//!
//! Public APIs are re-exported from focused child modules so callers can keep
//! using `conary_core::generation::builder::*`.

mod activation;
mod boot_assets;
mod carrier_capabilities;
mod cas;
mod create;
mod file_capabilities;
mod initramfs;
mod kernel;
mod rebuild;
mod root_validation;
mod runtime_inputs;
mod sysroot;

#[cfg(test)]
pub(crate) mod test_support;

pub use activation::GenerationActivation;
pub use create::{
    build_generation_from_captured_root_with_boot_root_and_activation, build_generation_from_db,
    build_generation_from_db_with_activation, build_generation_from_db_with_boot_root,
    build_generation_from_db_with_boot_root_and_activation, materialize_selected_root_from_db,
    materialize_selected_root_from_db_with_authority,
};
pub(crate) use file_capabilities::SECURITY_CAPABILITY_XATTR;
pub use file_capabilities::encode_security_capability_xattr;
pub(crate) use rebuild::rebuild_generation_image;

/// Result of serializing one exact generation-root manifest to EROFS.
#[derive(Debug, Clone)]
pub struct BuildResult {
    pub image_path: std::path::PathBuf,
    pub image_size: u64,
    pub cas_objects_referenced: u64,
    pub erofs_verity_digest: Option<String>,
}

/// Whether this build can serialize exact xattrs into composefs/EROFS images.
#[must_use]
pub fn erofs_xattr_image_support_available() -> bool {
    cfg!(feature = "composefs-rs")
}
