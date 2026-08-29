// crates/conary-core/src/repository/catalog/bundle.rs

//! Durable candidate manifests and atomic immutable catalog publication.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::contract::validate_storage_component;
use super::store::{CatalogDurableLogicalAttestationV1, CatalogVerificationProofV1};
use super::{
    CatalogArtifactV1, CatalogBindingV1, CatalogReader, CatalogScopeV1, CatalogSourceEvidenceV1,
    PortableChunkManifestV1, PortableIntegrityError, PortableManifestAttestationV1,
    ProfileRevisionV2, SourceSnapshotV1, read_portable_chunk_manifest_v1,
    write_portable_chunk_manifest_v1,
};
use crate::error::{Error, Result};

mod publication;

use publication::publish_verified_directory_for_registration;

pub const CATALOG_FILE_NAME: &str = "catalog.sqlite";
pub const CATALOG_MANIFEST_FILE_NAME: &str = "manifest.json";
pub const CATALOG_PORTABLE_MANIFEST_FILE_NAME: &str = "catalog.sqlite.chunks-v1";
pub const SOURCE_METADATA_DIRECTORY_NAME: &str = "native-metadata";

/// A durable catalog publication paired with its exact portable-manifest
/// attestation for operational registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedVerifiedCatalogBundle {
    pub path: PathBuf,
    pub newly_created: bool,
    pub portable_manifest_attestation: PortableManifestAttestationV1,
}

impl AsRef<Path> for PublishedVerifiedCatalogBundle {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for PublishedVerifiedCatalogBundle {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

/// Finalize a private source bundle and return the reader that proved its
/// catalog binding for immediate private-stage composition.
///
/// Verified durable publication consumes the returned reader and performs one
/// independent complete reopen after its atomic rename.
pub fn write_source_catalog_manifest(
    candidate_directory: impl AsRef<Path>,
    manifest: &SourceSnapshotV1,
) -> Result<CatalogReader> {
    let candidate_directory = candidate_directory.as_ref();
    manifest.validate()?;
    let reader = verify_source_catalog_binding(candidate_directory, manifest)?;
    verify_source_metadata_directory(candidate_directory, manifest)?;
    write_manifest(candidate_directory, manifest)?;
    verify_exact_source_directory(candidate_directory)?;
    verify_manifest_file(candidate_directory, manifest)?;
    Ok(reader)
}

/// Finalize a source bundle by consuming the reader that already proved this
/// exact private candidate and its same-process logical binding.
pub fn write_source_catalog_manifest_verified(
    candidate_directory: impl AsRef<Path>,
    manifest: &SourceSnapshotV1,
    verified: CatalogReader,
) -> Result<CatalogReader> {
    let candidate_directory = candidate_directory.as_ref();
    manifest.validate()?;
    require_source_catalog_binding(candidate_directory, manifest, &verified)?;
    verify_source_metadata_directory(candidate_directory, manifest)?;
    write_manifest(candidate_directory, manifest)?;
    verify_exact_source_directory(candidate_directory)?;
    verify_manifest_file(candidate_directory, manifest)?;
    Ok(verified)
}

/// Finalize a private profile bundle and return the reader that proved its
/// catalog binding.
///
/// Verified durable publication consumes the returned reader and performs one
/// independent complete reopen after its atomic rename.
pub fn write_profile_catalog_manifest(
    candidate_directory: impl AsRef<Path>,
    manifest: &ProfileRevisionV2,
) -> Result<CatalogReader> {
    let candidate_directory = candidate_directory.as_ref();
    manifest.validate()?;
    let reader = verify_profile_catalog_binding(candidate_directory, manifest)?;
    write_manifest(candidate_directory, manifest)?;
    verify_exact_profile_directory(candidate_directory)?;
    verify_manifest_file(candidate_directory, manifest)?;
    Ok(reader)
}

/// Finalize a profile bundle by consuming the reader that already proved this
/// exact private candidate and its same-process logical binding.
pub fn write_profile_catalog_manifest_verified(
    candidate_directory: impl AsRef<Path>,
    manifest: &ProfileRevisionV2,
    verified: CatalogReader,
) -> Result<CatalogReader> {
    let candidate_directory = candidate_directory.as_ref();
    manifest.validate()?;
    require_profile_catalog_binding(candidate_directory, manifest, &verified)?;
    write_manifest(candidate_directory, manifest)?;
    verify_exact_profile_directory(candidate_directory)?;
    verify_manifest_file(candidate_directory, manifest)?;
    Ok(verified)
}

pub fn verify_source_catalog_bundle(
    directory: impl AsRef<Path>,
    expected: &SourceSnapshotV1,
) -> Result<CatalogReader> {
    expected.validate()?;
    verify_exact_source_directory(directory.as_ref())?;
    verify_manifest_file(directory.as_ref(), expected)?;
    let reader = verify_source_catalog_binding(directory.as_ref(), expected)?;
    verify_source_metadata_directory(directory.as_ref(), expected)?;
    Ok(reader)
}

/// Completely verify a registered source bundle after authenticating its
/// portable sidecar against exact persisted physical authority.
///
/// Publication, promotion, and repair use this explicit path when they must
/// repeat the whole artifact hash, SQLite integrity check, and logical replay.
pub fn verify_registered_source_catalog_bundle_complete(
    directory: impl AsRef<Path>,
    expected: &SourceSnapshotV1,
    portable_manifest_attestation: &PortableManifestAttestationV1,
) -> Result<CatalogReader> {
    let directory = directory.as_ref();
    expected.validate()?;
    verify_exact_registered_source_directory(directory)?;
    verify_manifest_file(directory, expected)?;
    read_registered_portable_manifest(directory, portable_manifest_attestation, &expected.catalog)?;
    let reader = verify_source_catalog_binding(directory, expected)?;
    verify_source_metadata_directory(directory, expected)?;
    Ok(reader)
}

/// Reopen one registered source snapshot through its exact portable manifest.
pub fn verify_registered_source_catalog_bundle(
    directory: impl AsRef<Path>,
    expected: &SourceSnapshotV1,
    portable_manifest_attestation: &PortableManifestAttestationV1,
) -> Result<CatalogReader> {
    let directory = directory.as_ref();
    expected.validate()?;
    verify_exact_registered_source_directory(directory)?;
    verify_manifest_file(directory, expected)?;
    let portable_manifest = read_registered_portable_manifest(
        directory,
        portable_manifest_attestation,
        &expected.catalog,
    )?;
    let reader = verify_source_catalog_binding_with_registered_portable(
        directory,
        expected,
        portable_manifest,
    )?;
    verify_source_metadata_directory(directory, expected)?;
    Ok(reader)
}

fn verify_source_catalog_bundle_with_proof(
    directory: &Path,
    expected: &SourceSnapshotV1,
    proof: &CatalogVerificationProofV1,
    portable_manifest_attestation: &PortableManifestAttestationV1,
) -> Result<CatalogReader> {
    expected.validate()?;
    verify_exact_registered_source_directory(directory)?;
    verify_manifest_file(directory, expected)?;
    read_registered_portable_manifest(directory, portable_manifest_attestation, &expected.catalog)?;
    let reader = verify_source_catalog_binding_with_proof(directory, expected, proof)?;
    verify_source_metadata_directory(directory, expected)?;
    Ok(reader)
}

pub fn verify_profile_catalog_bundle(
    directory: impl AsRef<Path>,
    expected: &ProfileRevisionV2,
) -> Result<CatalogReader> {
    expected.validate()?;
    verify_exact_profile_directory(directory.as_ref())?;
    verify_manifest_file(directory.as_ref(), expected)?;
    verify_profile_catalog_binding(directory.as_ref(), expected)
}

/// Completely verify a registered profile bundle after authenticating its
/// portable sidecar against exact persisted physical authority.
///
/// Publication, promotion, and repair use this explicit path when they must
/// repeat the whole artifact hash, SQLite integrity check, and logical replay.
pub fn verify_registered_profile_catalog_bundle_complete(
    directory: impl AsRef<Path>,
    expected: &ProfileRevisionV2,
    portable_manifest_attestation: &PortableManifestAttestationV1,
) -> Result<CatalogReader> {
    let directory = directory.as_ref();
    expected.validate()?;
    verify_exact_registered_profile_directory(directory)?;
    verify_manifest_file(directory, expected)?;
    read_registered_portable_manifest(directory, portable_manifest_attestation, &expected.catalog)?;
    verify_profile_catalog_binding(directory, expected)
}

/// Reopen one registered profile revision through its exact portable manifest.
pub fn verify_registered_profile_catalog_bundle(
    directory: impl AsRef<Path>,
    expected: &ProfileRevisionV2,
    portable_manifest_attestation: &PortableManifestAttestationV1,
) -> Result<CatalogReader> {
    let directory = directory.as_ref();
    expected.validate()?;
    verify_exact_registered_profile_directory(directory)?;
    verify_manifest_file(directory, expected)?;
    let portable_manifest = read_registered_portable_manifest(
        directory,
        portable_manifest_attestation,
        &expected.catalog,
    )?;
    verify_profile_catalog_binding_with_registered_portable(directory, expected, portable_manifest)
}

fn verify_profile_catalog_bundle_with_proof(
    directory: &Path,
    expected: &ProfileRevisionV2,
    proof: &CatalogVerificationProofV1,
    portable_manifest_attestation: &PortableManifestAttestationV1,
) -> Result<CatalogReader> {
    expected.validate()?;
    verify_exact_registered_profile_directory(directory)?;
    verify_manifest_file(directory, expected)?;
    read_registered_portable_manifest(directory, portable_manifest_attestation, &expected.catalog)?;
    verify_profile_catalog_binding_with_proof(directory, expected, proof)
}

/// Move one exact parser-authenticated object into its source candidate.
///
/// The parser supplies the verified run-local file explicitly. Storage never
/// derives a filename from ecosystem, role, repository name, or URL.
pub(in crate::repository) fn retain_source_metadata_object(
    candidate_directory: &Path,
    work_directory: &Path,
    source: &Path,
    object: &super::SourceMetadataObjectV1,
) -> Result<()> {
    require_real_directory(candidate_directory)?;
    require_real_directory(work_directory)?;
    validate_metadata_digest(&object.sha256)?;
    if source.parent() != Some(work_directory) {
        return Err(Error::InvalidPath(format!(
            "authenticated metadata object {} is not a direct child of parser work directory {}",
            source.display(),
            work_directory.display()
        )));
    }
    verify_metadata_object_file(source, object, false)?;

    let metadata_directory = ensure_private_metadata_directory(candidate_directory)?;
    let destination = metadata_directory.join(&object.sha256);
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            verify_metadata_object_file(&destination, object, true)?;
            fs::remove_file(source)?;
            sync_directory(work_directory)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            require_same_filesystem(source, &metadata_directory)?;
            fs::rename(source, &destination).map_err(|error| {
                Error::IoError(format!(
                    "retain authenticated metadata {} as {}: {error}",
                    source.display(),
                    destination.display()
                ))
            })?;
            set_private_file_permissions(&destination)?;
            File::open(&destination)?.sync_all()?;
            sync_directory(work_directory)?;
            sync_directory(&metadata_directory)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Resolve and revalidate one exact retained native metadata object.
pub fn source_metadata_object_path(
    source_bundle: impl AsRef<Path>,
    object: &super::SourceMetadataObjectV1,
) -> Result<PathBuf> {
    validate_metadata_digest(&object.sha256)?;
    let metadata_directory = source_bundle.as_ref().join(SOURCE_METADATA_DIRECTORY_NAME);
    require_private_metadata_directory(&metadata_directory)?;
    let path = metadata_directory.join(&object.sha256);
    verify_metadata_object_file(&path, object, true)?;
    Ok(path)
}

/// Publish a source bundle by consuming its exact candidate reader, then
/// independently reopening the durable destination after atomic rename.
pub fn publish_source_catalog_bundle_verified(
    candidate_directory: impl AsRef<Path>,
    catalog_root: impl AsRef<Path>,
    manifest: &SourceSnapshotV1,
    verified: CatalogReader,
) -> Result<PublishedVerifiedCatalogBundle> {
    let candidate_directory = candidate_directory.as_ref();
    manifest.validate()?;
    require_source_catalog_binding(candidate_directory, manifest, &verified)?;
    verify_exact_source_directory(candidate_directory)?;
    verify_manifest_file(candidate_directory, manifest)?;
    let proof = verified.verification_proof()?.clone();
    let portable_manifest = verified.into_portable_chunk_manifest()?;
    let portable_manifest_attestation = write_portable_chunk_manifest_v1(
        &candidate_directory.join(CATALOG_PORTABLE_MANIFEST_FILE_NAME),
        &portable_manifest,
    )
    .map_err(portable_integrity_error)?;
    verify_exact_registered_source_directory(candidate_directory)?;
    let parent = ensure_real_subdirectory(catalog_root.as_ref(), "sources")?;
    publish_verified_directory_for_registration(
        candidate_directory,
        &parent,
        &manifest.manifest_sha256()?,
        portable_manifest_attestation,
        |path, attestation| {
            verify_source_catalog_bundle_with_proof(path, manifest, &proof, attestation)
        },
        |path, attestation| verify_registered_source_catalog_bundle(path, manifest, attestation),
    )
}

/// Publish a profile bundle by consuming its exact candidate reader, then
/// independently reopening the durable destination after atomic rename.
pub fn publish_profile_catalog_bundle_verified(
    candidate_directory: impl AsRef<Path>,
    catalog_root: impl AsRef<Path>,
    manifest: &ProfileRevisionV2,
    verified: CatalogReader,
) -> Result<PublishedVerifiedCatalogBundle> {
    let candidate_directory = candidate_directory.as_ref();
    manifest.validate()?;
    require_profile_catalog_binding(candidate_directory, manifest, &verified)?;
    verify_exact_profile_directory(candidate_directory)?;
    verify_manifest_file(candidate_directory, manifest)?;
    let proof = verified.verification_proof()?.clone();
    let portable_manifest = verified.into_portable_chunk_manifest()?;
    let portable_manifest_attestation = write_portable_chunk_manifest_v1(
        &candidate_directory.join(CATALOG_PORTABLE_MANIFEST_FILE_NAME),
        &portable_manifest,
    )
    .map_err(portable_integrity_error)?;
    verify_exact_registered_profile_directory(candidate_directory)?;
    validate_storage_component(&manifest.profile, "profile catalog storage identity")?;
    let profiles = ensure_real_subdirectory(catalog_root.as_ref(), "profiles")?;
    let parent = ensure_real_subdirectory(&profiles, &manifest.profile)?;
    publish_verified_directory_for_registration(
        candidate_directory,
        &parent,
        &manifest.manifest_sha256()?,
        portable_manifest_attestation,
        |path, attestation| {
            verify_profile_catalog_bundle_with_proof(path, manifest, &proof, attestation)
        },
        |path, attestation| verify_registered_profile_catalog_bundle(path, manifest, attestation),
    )
}

fn read_registered_portable_manifest(
    directory: &Path,
    portable_manifest_attestation: &PortableManifestAttestationV1,
    expected_artifact: &CatalogArtifactV1,
) -> Result<PortableChunkManifestV1> {
    read_portable_chunk_manifest_v1(
        &directory.join(CATALOG_PORTABLE_MANIFEST_FILE_NAME),
        portable_manifest_attestation,
        expected_artifact,
    )
    .map_err(portable_integrity_error)
}

fn portable_integrity_error(error: PortableIntegrityError) -> Error {
    match error {
        PortableIntegrityError::Io(error) => Error::Io(error),
        error => Error::ConflictError(error.to_string()),
    }
}

fn verify_source_catalog_binding(
    directory: &Path,
    manifest: &SourceSnapshotV1,
) -> Result<CatalogReader> {
    let binding = source_catalog_binding(manifest);
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

fn require_source_catalog_binding(
    directory: &Path,
    manifest: &SourceSnapshotV1,
    reader: &CatalogReader,
) -> Result<()> {
    let binding = source_catalog_binding(manifest);
    require_candidate_reader(directory, &binding, reader)?;
    verify_source_evidence(directory, manifest, reader)
}

fn verify_source_catalog_binding_with_proof(
    directory: &Path,
    manifest: &SourceSnapshotV1,
    proof: &CatalogVerificationProofV1,
) -> Result<CatalogReader> {
    let binding = source_catalog_binding(manifest);
    let reader = CatalogReader::open_verified_with_proof(
        directory.join(CATALOG_FILE_NAME),
        &binding,
        proof,
    )?;
    verify_source_evidence(directory, manifest, &reader)?;
    Ok(reader)
}

fn verify_profile_catalog_binding(
    directory: &Path,
    manifest: &ProfileRevisionV2,
) -> Result<CatalogReader> {
    let binding = profile_catalog_binding(manifest);
    let reader = CatalogReader::open_verified(directory.join(CATALOG_FILE_NAME), &binding)?;
    verify_profile_evidence(directory, manifest, &reader)?;
    Ok(reader)
}

fn require_profile_catalog_binding(
    directory: &Path,
    manifest: &ProfileRevisionV2,
    reader: &CatalogReader,
) -> Result<()> {
    let binding = profile_catalog_binding(manifest);
    require_candidate_reader(directory, &binding, reader)?;
    verify_profile_evidence(directory, manifest, reader)
}

fn require_candidate_reader(
    directory: &Path,
    expected: &CatalogBindingV1,
    reader: &CatalogReader,
) -> Result<()> {
    expected.validate()?;
    require_real_directory(directory)?;
    let path = directory.join(CATALOG_FILE_NAME);
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() != expected.artifact.size
    {
        return Err(Error::InvalidPath(format!(
            "verified catalog candidate {} has the wrong file type or size",
            path.display()
        )));
    }
    let canonical_path = path.canonicalize()?;
    if reader.path() != canonical_path {
        return Err(Error::ConflictError(format!(
            "catalog reader {} does not own exact candidate {}",
            reader.path().display(),
            canonical_path.display()
        )));
    }
    if reader.binding() != expected {
        return Err(Error::ConflictError(
            "catalog reader does not match the candidate manifest binding".to_string(),
        ));
    }
    reader.verification_proof()?;
    Ok(())
}

fn verify_profile_catalog_binding_with_proof(
    directory: &Path,
    manifest: &ProfileRevisionV2,
    proof: &CatalogVerificationProofV1,
) -> Result<CatalogReader> {
    let binding = profile_catalog_binding(manifest);
    let reader = CatalogReader::open_verified_with_proof(
        directory.join(CATALOG_FILE_NAME),
        &binding,
        proof,
    )?;
    verify_profile_evidence(directory, manifest, &reader)?;
    Ok(reader)
}

fn verify_profile_catalog_binding_with_registered_portable(
    directory: &Path,
    manifest: &ProfileRevisionV2,
    portable_manifest: PortableChunkManifestV1,
) -> Result<CatalogReader> {
    let binding = profile_catalog_binding(manifest);
    let attestation = CatalogDurableLogicalAttestationV1::new(&binding);
    let reader = CatalogReader::open_registered_portable(
        directory.join(CATALOG_FILE_NAME),
        &binding,
        &attestation,
        portable_manifest,
    )?;
    verify_profile_evidence(directory, manifest, &reader)?;
    Ok(reader)
}

fn verify_source_catalog_binding_with_registered_portable(
    directory: &Path,
    manifest: &SourceSnapshotV1,
    portable_manifest: PortableChunkManifestV1,
) -> Result<CatalogReader> {
    let binding = source_catalog_binding(manifest);
    let attestation = CatalogDurableLogicalAttestationV1::new(&binding);
    let reader = CatalogReader::open_registered_portable(
        directory.join(CATALOG_FILE_NAME),
        &binding,
        &attestation,
        portable_manifest,
    )?;
    verify_source_evidence(directory, manifest, &reader)?;
    Ok(reader)
}

fn profile_catalog_binding(manifest: &ProfileRevisionV2) -> CatalogBindingV1 {
    CatalogBindingV1 {
        scope: CatalogScopeV1::Profile {
            profile: manifest.profile.clone(),
        },
        artifact: manifest.catalog.clone(),
        logical_digest_sha256: manifest.logical_digest_sha256.clone(),
        counts: manifest.counts,
    }
}

fn source_catalog_binding(manifest: &SourceSnapshotV1) -> CatalogBindingV1 {
    CatalogBindingV1 {
        scope: CatalogScopeV1::Source {
            source_profile: manifest.source_profile.clone(),
            source_identity: manifest.source_identity.clone(),
            repository_identity: manifest.repository_identity.clone(),
        },
        artifact: manifest.catalog.clone(),
        logical_digest_sha256: manifest.logical_digest_sha256.clone(),
        counts: manifest.counts,
    }
}

fn verify_source_evidence(
    directory: &Path,
    manifest: &SourceSnapshotV1,
    reader: &CatalogReader,
) -> Result<()> {
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
    Ok(())
}

fn verify_profile_evidence(
    directory: &Path,
    manifest: &ProfileRevisionV2,
    reader: &CatalogReader,
) -> Result<()> {
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
    Ok(())
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

fn verify_exact_source_directory(directory: &Path) -> Result<()> {
    verify_exact_directory_entries(
        directory,
        &[
            CATALOG_FILE_NAME,
            CATALOG_MANIFEST_FILE_NAME,
            SOURCE_METADATA_DIRECTORY_NAME,
        ],
        "source catalog bundle",
    )
}

fn verify_exact_registered_source_directory(directory: &Path) -> Result<()> {
    verify_exact_directory_entries(
        directory,
        &[
            CATALOG_FILE_NAME,
            CATALOG_MANIFEST_FILE_NAME,
            CATALOG_PORTABLE_MANIFEST_FILE_NAME,
            SOURCE_METADATA_DIRECTORY_NAME,
        ],
        "registered source catalog bundle",
    )
}

fn verify_exact_profile_directory(directory: &Path) -> Result<()> {
    verify_exact_directory_entries(
        directory,
        &[CATALOG_FILE_NAME, CATALOG_MANIFEST_FILE_NAME],
        "profile catalog bundle",
    )
}

fn verify_exact_registered_profile_directory(directory: &Path) -> Result<()> {
    verify_exact_directory_entries(
        directory,
        &[
            CATALOG_FILE_NAME,
            CATALOG_MANIFEST_FILE_NAME,
            CATALOG_PORTABLE_MANIFEST_FILE_NAME,
        ],
        "registered profile catalog bundle",
    )
}

fn verify_exact_directory_entries(directory: &Path, expected: &[&str], label: &str) -> Result<()> {
    require_real_directory(directory)?;
    let mut names = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name()).map_err(Error::from))
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    let mut expected = expected
        .iter()
        .map(|name| std::ffi::OsString::from(*name))
        .collect::<Vec<_>>();
    expected.sort();
    if names != expected {
        return Err(Error::ConflictError(format!(
            "{label} {} has incomplete or unexpected entries",
            directory.display()
        )));
    }
    Ok(())
}

fn verify_source_metadata_directory(directory: &Path, manifest: &SourceSnapshotV1) -> Result<()> {
    let metadata_directory = directory.join(SOURCE_METADATA_DIRECTORY_NAME);
    require_private_metadata_directory(&metadata_directory)?;

    let mut expected = BTreeMap::<String, u64>::new();
    for object in &manifest.authenticated_objects {
        if let Some(size) = expected.insert(object.sha256.clone(), object.size)
            && size != object.size
        {
            return Err(Error::ConflictError(format!(
                "authenticated metadata digest {} has conflicting sizes",
                object.sha256
            )));
        }
    }
    let mut actual = fs::read_dir(&metadata_directory)?
        .map(|entry| entry.map(|entry| entry.file_name()).map_err(Error::from))
        .collect::<Result<Vec<_>>>()?;
    actual.sort();
    let expected_names = expected
        .keys()
        .map(|name| std::ffi::OsString::from(name.as_str()))
        .collect::<Vec<_>>();
    if actual != expected_names {
        return Err(Error::ConflictError(format!(
            "source metadata directory {} has incomplete or unexpected entries",
            metadata_directory.display()
        )));
    }
    for object in &manifest.authenticated_objects {
        source_metadata_object_path(directory, object)?;
    }
    Ok(())
}

fn ensure_private_metadata_directory(candidate_directory: &Path) -> Result<PathBuf> {
    let path = candidate_directory.join(SOURCE_METADATA_DIRECTORY_NAME);
    match fs::create_dir(&path) {
        Ok(()) => {
            set_private_directory_permissions(&path)?;
            sync_directory(candidate_directory)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    require_private_metadata_directory(&path)?;
    Ok(path)
}

fn require_private_metadata_directory(path: &Path) -> Result<()> {
    require_real_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err(Error::InvalidPath(format!(
                "source metadata directory {} must not grant group or other access",
                path.display()
            )));
        }
    }
    Ok(())
}

fn verify_metadata_object_file(
    path: &Path,
    object: &super::SourceMetadataObjectV1,
    require_private_permissions: bool,
) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() != object.size
    {
        return Err(Error::InvalidPath(format!(
            "authenticated metadata object {} has the wrong file type or size",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if require_private_permissions && metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::InvalidPath(format!(
                "authenticated metadata object {} must not grant group or other access",
                path.display()
            )));
        }
    }
    crate::hash::verify_file_sha256(path, &object.sha256).map_err(|error| Error::ChecksumMismatch {
        expected: error.expected,
        actual: error.actual,
    })
}

fn set_private_directory_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn validate_metadata_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ConfigError(
            "authenticated metadata object requires a lowercase SHA-256 digest".to_string(),
        ));
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
