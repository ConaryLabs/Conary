// crates/conary-core/src/generation/builder/verity.rs

//! Finalize image verification for both publication and recovery.

use std::path::Path;

use crate::filesystem::fsverity::{FsVerityError, enable_fsverity};
use crate::generation::metadata::GenerationMetadata;
use crate::generation::verity_policy::VerityPolicy;
use crate::{Error, Result};

/// Enable the image's fs-verity protection, then persist the readiness flag
/// alongside the digest produced by the EROFS builder. Never advertise a
/// verified generation when the ioctl or metadata persistence fails.
pub fn enable_generation_rootfs_verity(gen_dir: &Path, image_path: &Path) -> Result<()> {
    enable_generation_rootfs_verity_with(gen_dir, image_path, enable_fsverity)
}

pub(crate) fn enable_generation_rootfs_verity_with(
    gen_dir: &Path,
    image_path: &Path,
    enable: impl FnOnce(&Path) -> std::result::Result<bool, FsVerityError>,
) -> Result<()> {
    let mut metadata = GenerationMetadata::read_from(gen_dir)?;
    metadata.fsverity_enabled = true;
    // Validate the builder's digest before enabling or persisting anything.
    VerityPolicy::Verified.mount_requirements(&metadata)?;
    let newly_enabled = enable(image_path).map_err(|error| match error {
        FsVerityError::NotSupported(path) => Error::IoError(format!(
            "Generation image {} is on a filesystem without fs-verity support; refusing to finalize an unverified generation",
            path.display()
        )),
        error => Error::IoError(format!(
            "Failed to enable fs-verity on generation image {}: {error}",
            image_path.display()
        )),
    })?;
    metadata.write_to(gen_dir)?;
    tracing::debug!(image = %image_path.display(), newly_enabled, "Generation fs-verity finalized");
    Ok(())
}

#[cfg(test)]
mod tests;
