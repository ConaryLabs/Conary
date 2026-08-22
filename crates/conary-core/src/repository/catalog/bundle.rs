// crates/conary-core/src/repository/catalog/bundle.rs

//! Durable candidate manifests and atomic immutable catalog publication.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::contract::validate_storage_component;
use super::{
    CatalogBindingV1, CatalogReader, CatalogScopeV1, CatalogSourceEvidenceV1, ProfileRevisionV1,
    SourceSnapshotV1,
};
use crate::error::{Error, Result};

pub const CATALOG_FILE_NAME: &str = "catalog.sqlite";
pub const CATALOG_MANIFEST_FILE_NAME: &str = "manifest.json";

/// The durable filesystem result of publishing one immutable catalog bundle.
///
/// `newly_created` is the publication provenance needed by a caller that may
/// have to roll back a larger publication. A false value means the exact
/// content-addressed destination already existed and was verified; callers
/// must never remove that destination as part of their rollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedCatalogBundle {
    pub path: PathBuf,
    pub newly_created: bool,
}

impl PublishedCatalogBundle {
    /// Consume the publication result and return its destination path.
    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

pub fn write_source_catalog_manifest(
    candidate_directory: impl AsRef<Path>,
    manifest: &SourceSnapshotV1,
) -> Result<()> {
    manifest.validate()?;
    verify_source_catalog_binding(candidate_directory.as_ref(), manifest)?;
    write_manifest(candidate_directory.as_ref(), manifest)
}

pub fn write_profile_catalog_manifest(
    candidate_directory: impl AsRef<Path>,
    manifest: &ProfileRevisionV1,
) -> Result<()> {
    manifest.validate()?;
    verify_profile_catalog_binding(candidate_directory.as_ref(), manifest)?;
    write_manifest(candidate_directory.as_ref(), manifest)
}

pub fn verify_source_catalog_bundle(
    directory: impl AsRef<Path>,
    expected: &SourceSnapshotV1,
) -> Result<CatalogReader> {
    expected.validate()?;
    verify_exact_directory(directory.as_ref())?;
    verify_manifest_file(directory.as_ref(), expected)?;
    verify_source_catalog_binding(directory.as_ref(), expected)
}

pub fn verify_profile_catalog_bundle(
    directory: impl AsRef<Path>,
    expected: &ProfileRevisionV1,
) -> Result<CatalogReader> {
    expected.validate()?;
    verify_exact_directory(directory.as_ref())?;
    verify_manifest_file(directory.as_ref(), expected)?;
    verify_profile_catalog_binding(directory.as_ref(), expected)
}

pub fn publish_source_catalog_bundle(
    candidate_directory: impl AsRef<Path>,
    catalog_root: impl AsRef<Path>,
    manifest: &SourceSnapshotV1,
) -> Result<PathBuf> {
    publish_source_catalog_bundle_with_provenance(candidate_directory, catalog_root, manifest)
        .map(PublishedCatalogBundle::into_path)
}

/// Publish a source bundle and report whether this call created its immutable
/// content-addressed destination.
pub fn publish_source_catalog_bundle_with_provenance(
    candidate_directory: impl AsRef<Path>,
    catalog_root: impl AsRef<Path>,
    manifest: &SourceSnapshotV1,
) -> Result<PublishedCatalogBundle> {
    let candidate_directory = candidate_directory.as_ref();
    verify_source_catalog_bundle(candidate_directory, manifest)?;
    let parent = ensure_real_subdirectory(catalog_root.as_ref(), "sources")?;
    publish_verified_directory(
        candidate_directory,
        &parent,
        &manifest.manifest_sha256()?,
        |path| verify_source_catalog_bundle(path, manifest).map(|_| ()),
    )
}

pub fn publish_profile_catalog_bundle(
    candidate_directory: impl AsRef<Path>,
    catalog_root: impl AsRef<Path>,
    manifest: &ProfileRevisionV1,
) -> Result<PathBuf> {
    publish_profile_catalog_bundle_with_provenance(candidate_directory, catalog_root, manifest)
        .map(PublishedCatalogBundle::into_path)
}

/// Publish a profile bundle and report whether this call created its
/// immutable content-addressed destination.
pub fn publish_profile_catalog_bundle_with_provenance(
    candidate_directory: impl AsRef<Path>,
    catalog_root: impl AsRef<Path>,
    manifest: &ProfileRevisionV1,
) -> Result<PublishedCatalogBundle> {
    let candidate_directory = candidate_directory.as_ref();
    verify_profile_catalog_bundle(candidate_directory, manifest)?;
    validate_storage_component(&manifest.profile, "profile catalog storage identity")?;
    let profiles = ensure_real_subdirectory(catalog_root.as_ref(), "profiles")?;
    let parent = ensure_real_subdirectory(&profiles, &manifest.profile)?;
    publish_verified_directory(
        candidate_directory,
        &parent,
        &manifest.manifest_sha256()?,
        |path| verify_profile_catalog_bundle(path, manifest).map(|_| ()),
    )
}

fn verify_source_catalog_binding(
    directory: &Path,
    manifest: &SourceSnapshotV1,
) -> Result<CatalogReader> {
    let binding = CatalogBindingV1 {
        scope: CatalogScopeV1::Source {
            source_profile: manifest.source_profile.clone(),
            source_identity: manifest.source_identity.clone(),
            repository_identity: manifest.repository_identity.clone(),
        },
        artifact: manifest.catalog.clone(),
        logical_digest_sha256: manifest.logical_digest_sha256.clone(),
        counts: manifest.counts,
    };
    let reader = CatalogReader::open_verified(directory.join(CATALOG_FILE_NAME), &binding)?;
    let expected_evidence = manifest
        .authenticated_objects
        .iter()
        .map(|object| CatalogSourceEvidenceV1::AuthenticatedObject {
            role: object.role.clone(),
            source_path: object.source_path.clone(),
            sha256: object.sha256.clone(),
            size: object.size,
        })
        .collect::<Vec<_>>();
    if reader.source_evidence()? != expected_evidence {
        return Err(Error::ConflictError(format!(
            "source catalog {} evidence does not match its authenticated object manifest",
            directory.display()
        )));
    }
    Ok(reader)
}

fn verify_profile_catalog_binding(
    directory: &Path,
    manifest: &ProfileRevisionV1,
) -> Result<CatalogReader> {
    let binding = CatalogBindingV1 {
        scope: CatalogScopeV1::Profile {
            profile: manifest.profile.clone(),
        },
        artifact: manifest.catalog.clone(),
        logical_digest_sha256: manifest.logical_digest_sha256.clone(),
        counts: manifest.counts,
    };
    let reader = CatalogReader::open_verified(directory.join(CATALOG_FILE_NAME), &binding)?;
    let expected_evidence = manifest
        .members
        .iter()
        .map(|member| CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: member.ordinal,
            source_identity: member.source_identity.clone(),
            repository_identity: member.repository_identity.clone(),
            source_snapshot_sha256: member.source_snapshot_sha256.clone(),
        })
        .collect::<Vec<_>>();
    if reader.source_evidence()? != expected_evidence {
        return Err(Error::ConflictError(format!(
            "profile catalog {} members do not match its revision manifest",
            directory.display()
        )));
    }
    Ok(reader)
}

fn write_manifest(directory: &Path, manifest: &impl serde::Serialize) -> Result<()> {
    require_real_directory(directory)?;
    let path = directory.join(CATALOG_MANIFEST_FILE_NAME);
    let bytes = crate::json::canonical_json(manifest).map_err(|error| {
        Error::ParseError(format!("serialize immutable catalog manifest: {error}"))
    })?;
    if path.exists() {
        if fs::read(&path)? == bytes {
            File::open(&path)?.sync_all()?;
            sync_directory(directory)?;
            return Ok(());
        }
        return Err(Error::ConflictError(format!(
            "catalog candidate manifest {} already exists with different bytes",
            path.display()
        )));
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    sync_directory(directory)
}

fn verify_manifest_file<T>(directory: &Path, expected: &T) -> Result<()>
where
    T: serde::Serialize,
{
    let path = directory.join(CATALOG_MANIFEST_FILE_NAME);
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(Error::InvalidPath(format!(
            "catalog manifest {} must be a regular file",
            path.display()
        )));
    }
    let expected_bytes = crate::json::canonical_json(expected).map_err(|error| {
        Error::ParseError(format!("serialize expected catalog manifest: {error}"))
    })?;
    let actual = fs::read(&path)?;
    if actual != expected_bytes {
        return Err(Error::ChecksumMismatch {
            expected: crate::hash::sha256(&expected_bytes),
            actual: crate::hash::sha256(&actual),
        });
    }
    Ok(())
}

fn publish_verified_directory<F>(
    candidate: &Path,
    parent: &Path,
    identity: &str,
    verify: F,
) -> Result<PublishedCatalogBundle>
where
    F: Fn(&Path) -> Result<()>,
{
    require_real_directory(candidate)?;
    require_real_directory(parent)?;
    let destination = parent.join(identity);
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            verify(&destination)?;
            return Ok(PublishedCatalogBundle {
                path: destination,
                newly_created: false,
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
    if let Err(error) = sync_directory(parent).and_then(|()| verify(&destination)) {
        return Err(cleanup_failed_publication(&destination, error));
    }
    Ok(PublishedCatalogBundle {
        path: destination,
        newly_created: true,
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

fn verify_exact_directory(directory: &Path) -> Result<()> {
    require_real_directory(directory)?;
    let mut names = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name()).map_err(Error::from))
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    let mut expected = vec![
        std::ffi::OsString::from(CATALOG_FILE_NAME),
        std::ffi::OsString::from(CATALOG_MANIFEST_FILE_NAME),
    ];
    expected.sort();
    if names != expected {
        return Err(Error::ConflictError(format!(
            "catalog bundle {} must contain exactly {} and {}",
            directory.display(),
            CATALOG_FILE_NAME,
            CATALOG_MANIFEST_FILE_NAME
        )));
    }
    Ok(())
}

fn ensure_real_subdirectory(parent: &Path, name: &str) -> Result<PathBuf> {
    require_real_directory(parent)?;
    validate_storage_component(name, "catalog storage directory")?;
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => {
            sync_directory(parent)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    require_real_directory(&path)?;
    Ok(path)
}

fn require_real_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(Error::InvalidPath(format!(
            "catalog path {} must be a real directory",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn require_same_filesystem(left: &Path, right: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let left_device = fs::metadata(left)?.dev();
    let right_device = fs::metadata(right)?.dev();
    if left_device != right_device {
        return Err(Error::ConflictError(format!(
            "catalog candidate {} and immutable parent {} are on different filesystems",
            left.display(),
            right.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_same_filesystem(_left: &Path, _right: &Path) -> Result<()> {
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
