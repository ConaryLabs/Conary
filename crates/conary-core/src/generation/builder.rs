// conary-core/src/generation/builder.rs

//! Generation builder - creates EROFS images from system state.
//!
//! Public APIs are re-exported from focused child modules so callers can keep
//! using `conary_core::generation::builder::*`.

mod activation;
mod boot_assets;
mod cas;
mod create;
mod erofs;
mod initramfs;
mod kernel;
mod rebuild;
mod root_validation;
mod runtime_inputs;
mod sysroot;

#[cfg(test)]
pub(super) mod test_support;

pub use activation::GenerationActivation;
pub use create::{
    build_generation_from_db, build_generation_from_db_with_activation,
    build_generation_from_db_with_boot_root,
    build_generation_from_db_with_boot_root_and_activation,
};
pub use erofs::{BuildResult, FileEntryRef, SymlinkEntryRef, build_erofs_image, hex_to_digest};
pub use kernel::detect_kernel_version_from_troves;

pub(crate) use rebuild::rebuild_generation_image;
