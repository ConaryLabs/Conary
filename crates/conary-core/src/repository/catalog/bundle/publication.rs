// crates/conary-core/src/repository/catalog/bundle/publication.rs

//! Atomic publication of a completely verified catalog directory.

use std::fs;
use std::path::Path;

use super::{
    PublishedVerifiedCatalogBundle, require_real_directory, require_same_filesystem, sync_directory,
};
use crate::error::{Error, Result};
use crate::repository::catalog::{CatalogReader, PortableManifestAttestationV1};

pub(super) fn publish_verified_directory_for_registration<F, R>(
    candidate: &Path,
    parent: &Path,
    identity: &str,
    portable_manifest_attestation: PortableManifestAttestationV1,
    verify_full: F,
    verify_registered: R,
) -> Result<PublishedVerifiedCatalogBundle>
where
    F: Fn(&Path, &PortableManifestAttestationV1) -> Result<CatalogReader>,
    R: Fn(&Path, &PortableManifestAttestationV1) -> Result<CatalogReader>,
{
    require_real_directory(candidate)?;
    require_real_directory(parent)?;
    let verify = |destination: &Path| -> Result<()> {
        drop(verify_full(destination, &portable_manifest_attestation)?);
        drop(verify_registered(
            destination,
            &portable_manifest_attestation,
        )?);
        Ok(())
    };
    let destination = parent.join(identity);
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            verify(&destination)?;
            return Ok(PublishedVerifiedCatalogBundle {
                path: destination,
                newly_created: false,
                portable_manifest_attestation,
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    require_same_filesystem(candidate, parent)?;
    fs::rename(candidate, &destination).map_err(|error| {
        Error::IoError(format!(
            "atomically publish catalog {} as {}: {error}",
            candidate.display(),
            destination.display()
        ))
    })?;
    match sync_directory(parent).and_then(|()| verify(&destination)) {
        Ok(()) => {}
        Err(error) => return Err(cleanup_failed_publication(&destination, error)),
    }
    Ok(PublishedVerifiedCatalogBundle {
        path: destination,
        newly_created: true,
        portable_manifest_attestation,
    })
}

/// Remove only the destination created by the immediately preceding rename.
///
/// Existing destinations take the early return above and never reach this
/// helper. Refuse to remove anything that is no longer a real directory; that
/// keeps a replacement symlink or regular file out of the cleanup blast
/// radius.
fn cleanup_failed_publication(destination: &Path, publication_error: Error) -> Error {
    let cleanup_result = (|| -> Result<()> {
        require_real_directory(destination)?;
        fs::remove_dir_all(destination)?;
        if let Some(parent) = destination.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    })();
    match cleanup_result {
        Ok(()) => publication_error,
        Err(cleanup_error) => Error::IoError(format!(
            "catalog publication failed after creating {}; cleanup also failed ({cleanup_error}); the newly-created destination may remain: {publication_error}",
            destination.display()
        )),
    }
}
