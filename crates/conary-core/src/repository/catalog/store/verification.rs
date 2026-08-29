// crates/conary-core/src/repository/catalog/store/verification.rs

//! Immutable reopen checks and non-serializable exact-artifact proofs.

use std::fs::{self, File, OpenOptions};
use std::io::BufReader;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::util::{conversion_error, parse_json_column, read_u64, reject_nonempty_sidecars};
use super::{
    CATALOG_APPLICATION_ID, CATALOG_CONTENT_SCHEMA_V1, CatalogBindingV1, CatalogConnectionOwner,
    CatalogReader, portable_catalog_result,
};
use crate::error::{Error, Result};
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCountsV1, PortableCatalogConnection, PortableChunkManifestV1,
    PortableIntegrityError, portable_manifest_size_v1,
};

/// Production evidence for the exact work one catalog reader performed while
/// establishing authority.
///
/// Timings are monotonic wall-clock microseconds. Deterministic counters name
/// complete verification passes and bytes covered; observed process I/O is
/// recorded separately by the Remi benchmark because page-cache behavior is
/// intentionally not authority.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogVerificationEvidenceV1 {
    pub catalog_bytes: u64,
    pub total_wall_us: u64,
    pub layout_sidecars_wall_us: u64,
    pub artifact_identity_wall_us: u64,
    pub sqlite_open_header_wall_us: u64,
    pub sqlite_integrity_wall_us: u64,
    pub stored_binding_wall_us: u64,
    pub logical_replay_wall_us: u64,
    pub userspace_sha256_passes: u64,
    pub userspace_sha256_bytes: u64,
    pub portable_manifest_validation_passes: u64,
    pub portable_manifest_validation_bytes: u64,
    pub sqlite_integrity_passes: u64,
    pub sqlite_integrity_bytes_covered: u64,
    pub stored_binding_checks: u64,
    pub logical_replay_passes: u64,
}

/// Read the embedded binding without treating it as independent authority.
pub(super) fn read_binding(
    connection: &Connection,
    artifact: CatalogArtifactV1,
) -> Result<CatalogBindingV1> {
    connection
        .query_row(
            "SELECT schema_version, scope_json, logical_digest_sha256,
                    package_count, provide_count, requirement_group_count,
                    requirement_atom_count, source_evidence_count
             FROM catalog_metadata WHERE singleton = 1",
            [],
            |row| {
                let schema_version: i64 = row.get(0)?;
                if schema_version != i64::from(CATALOG_CONTENT_SCHEMA_V1) {
                    return Err(conversion_error(
                        0,
                        format!("unsupported catalog schema {schema_version}"),
                    ));
                }
                Ok(CatalogBindingV1 {
                    scope: parse_json_column(row, 1)?,
                    artifact,
                    logical_digest_sha256: row.get(2)?,
                    counts: CatalogCountsV1 {
                        packages: read_u64(row, 3, "package count")?,
                        provides: read_u64(row, 4, "provide count")?,
                        requirement_groups: read_u64(row, 5, "requirement group count")?,
                        requirement_atoms: read_u64(row, 6, "requirement atom count")?,
                        source_evidence: read_u64(row, 7, "source evidence count")?,
                    },
                })
            },
        )
        .optional()?
        .ok_or_else(|| Error::InitError("catalog metadata singleton is missing".to_string()))
}

/// Opaque authority minted after a versioned durable owner proves that its
/// exact binding was published only from a complete logical replay.
///
/// The token is intentionally non-serializable. Durable owners validate their
/// own canonical attestation and exact input identity before constructing it;
/// the catalog store then binds the token to the expected artifact again.
#[derive(Debug)]
pub(in crate::repository) struct CatalogDurableLogicalAttestationV1 {
    binding: CatalogBindingV1,
}

impl CatalogDurableLogicalAttestationV1 {
    pub(in crate::repository) fn new(binding: &CatalogBindingV1) -> Self {
        Self {
            binding: binding.clone(),
        }
    }

    fn require_binding(&self, expected: &CatalogBindingV1) -> Result<()> {
        if &self.binding != expected {
            return Err(Error::ConflictError(
                "durable catalog logical attestation does not match the exact artifact binding"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// Process-local proof that one exact catalog binding owns complete logical
/// row-replay authority.
///
/// The fields are private and this type is not serializable. It can be minted
/// by a full `CatalogReader::open_verified` or by reopening an exact durable
/// logical attestation whose owning schema required that full replay before
/// publication. An unsigned or unversioned manifest cannot mint it.
#[derive(Debug, Clone)]
pub(in crate::repository) struct CatalogVerificationProofV1 {
    binding: CatalogBindingV1,
}

impl CatalogVerificationProofV1 {
    pub(super) fn new(binding: &CatalogBindingV1) -> Self {
        Self {
            binding: binding.clone(),
        }
    }

    fn require_binding(&self, expected: &CatalogBindingV1) -> Result<()> {
        if &self.binding != expected {
            return Err(Error::ConflictError(
                "catalog verification proof does not match the exact artifact binding".to_string(),
            ));
        }
        Ok(())
    }
}

fn elapsed_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn catalog_display_path(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        Error::InvalidPath(format!(
            "immutable catalog {} has no parent directory",
            path.display()
        ))
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        Error::InvalidPath(format!(
            "immutable catalog {} has no file name",
            path.display()
        ))
    })?;
    Ok(parent.canonicalize()?.join(file_name))
}

fn open_catalog_anchor(path: &Path) -> Result<(PathBuf, File, fs::Metadata)> {
    let path_metadata = fs::symlink_metadata(path).map_err(|error| {
        Error::IoError(format!(
            "inspect immutable catalog {}: {error}",
            path.display()
        ))
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.file_type().is_file() {
        return Err(Error::InvalidPath(format!(
            "immutable catalog {} must be a regular file, never a symlink",
            path.display()
        )));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let file = options.open(path).map_err(|error| {
        Error::IoError(format!(
            "open immutable catalog {} without following symlinks: {error}",
            path.display()
        ))
    })?;
    let file_metadata = file.metadata()?;
    if !file_metadata.file_type().is_file() {
        return Err(Error::InvalidPath(format!(
            "immutable catalog {} did not open as a regular file",
            path.display()
        )));
    }
    require_same_open_file(path, &path_metadata, &file_metadata)?;
    Ok((catalog_display_path(path)?, file, file_metadata))
}

#[cfg(unix)]
fn require_same_open_file(
    path: &Path,
    path_metadata: &fs::Metadata,
    file_metadata: &fs::Metadata,
) -> Result<()> {
    if path_metadata.dev() != file_metadata.dev() || path_metadata.ino() != file_metadata.ino() {
        return Err(Error::ConflictError(format!(
            "immutable catalog {} changed while its file descriptor was opened",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_same_open_file(
    _path: &Path,
    _path_metadata: &fs::Metadata,
    _file_metadata: &fs::Metadata,
) -> Result<()> {
    Ok(())
}

fn require_path_still_names_anchor(path: &Path, file: &File) -> Result<()> {
    let path_metadata = fs::symlink_metadata(path).map_err(|error| {
        Error::IoError(format!(
            "reinspect immutable catalog {} after verification: {error}",
            path.display()
        ))
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.file_type().is_file() {
        return Err(Error::InvalidPath(format!(
            "immutable catalog {} changed file type during verification",
            path.display()
        )));
    }
    require_same_open_file(path, &path_metadata, &file.metadata()?)
}

fn hash_catalog_anchor(file: &File) -> Result<String> {
    let mut reader = BufReader::new(file.try_clone()?);
    Ok(crate::hash::sha256_reader_hex(&mut reader)?)
}

#[cfg(target_os = "linux")]
fn sqlite_anchor_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(not(target_os = "linux"))]
fn sqlite_anchor_path(_file: &File) -> PathBuf {
    PathBuf::new()
}

fn open_sqlite_anchor(file: &File, _display_path: &Path) -> Result<Connection> {
    #[cfg(target_os = "linux")]
    let sqlite_path = sqlite_anchor_path(file);
    #[cfg(not(target_os = "linux"))]
    let sqlite_path = _display_path.to_path_buf();

    let mut uri = url::Url::from_file_path(&sqlite_path).map_err(|_| {
        Error::InvalidPath(format!(
            "catalog anchor {} cannot be represented as an immutable SQLite URI",
            sqlite_path.display()
        ))
    })?;
    uri.query_pairs_mut().append_pair("immutable", "1");
    let connection = Connection::open_with_flags(
        uri.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    connection.execute_batch(
        "PRAGMA query_only = ON; PRAGMA foreign_keys = ON; PRAGMA trusted_schema = OFF;",
    )?;
    Ok(connection)
}

impl CatalogReader {
    /// Require the catalog pathname to still name this reader's retained
    /// no-follow file descriptor.
    ///
    /// Durable handoffs use this immediately before carrying a previously
    /// verified reader into operational state, so an inode replacement cannot
    /// be mistaken for the exact artifact that the reader authenticated.
    pub fn require_path_unchanged(&self) -> Result<()> {
        require_path_still_names_anchor(&self.path, &self.file_anchor)
    }

    pub fn open_verified(path: impl AsRef<Path>, expected: &CatalogBindingV1) -> Result<Self> {
        Self::open_verified_inner(path.as_ref(), expected, true)
    }

    /// Open an exact signed catalog artifact without replaying its complete
    /// logical projection through Rust values.
    ///
    /// The Remi publisher performs the logical/schema replay before its
    /// dedicated universe role signs the artifact. A universe client has
    /// already verified that signature and the exact file SHA-256; it repeats
    /// the physical schema, integrity, and embedded-binding checks here, then
    /// copies normalized rows directly between SQLite databases.
    /// This avoids turning one arbitrarily large presentation or expression
    /// field into synchronization memory.
    pub(in crate::repository) fn open_verified_signed_artifact(
        path: impl AsRef<Path>,
        expected: &CatalogBindingV1,
    ) -> Result<Self> {
        Self::open_verified_inner(path.as_ref(), expected, false)
    }

    fn open_verified_inner(
        path: &Path,
        expected: &CatalogBindingV1,
        verify_logical_content: bool,
    ) -> Result<Self> {
        let started = Instant::now();
        let mut evidence = CatalogVerificationEvidenceV1 {
            catalog_bytes: expected.artifact.size,
            ..CatalogVerificationEvidenceV1::default()
        };
        expected.validate()?;

        let phase_started = Instant::now();
        let (canonical_path, file_anchor, metadata) = open_catalog_anchor(path)?;
        reject_nonempty_sidecars(path)?;
        if metadata.len() != expected.artifact.size {
            return Err(Error::ChecksumMismatch {
                expected: format!("{} bytes", expected.artifact.size),
                actual: format!("{} bytes", metadata.len()),
            });
        }
        evidence.layout_sidecars_wall_us = elapsed_us(phase_started.elapsed());

        let phase_started = Instant::now();
        let actual_sha256 = hash_catalog_anchor(&file_anchor)?;
        if actual_sha256 != expected.artifact.sha256 {
            return Err(Error::ChecksumMismatch {
                expected: expected.artifact.sha256.clone(),
                actual: actual_sha256,
            });
        }
        evidence.userspace_sha256_passes = 1;
        evidence.userspace_sha256_bytes = expected.artifact.size;
        evidence.artifact_identity_wall_us = elapsed_us(phase_started.elapsed());

        let phase_started = Instant::now();
        let connection = open_sqlite_anchor(&file_anchor, &canonical_path)?;
        let application_id: i64 =
            connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
        if application_id != CATALOG_APPLICATION_ID {
            return Err(Error::ConfigError(format!(
                "catalog {} has application id {application_id:#x}; expected {CATALOG_APPLICATION_ID:#x}",
                path.display()
            )));
        }
        evidence.sqlite_open_header_wall_us = elapsed_us(phase_started.elapsed());

        let phase_started = Instant::now();
        let integrity: String =
            connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(Error::InitError(format!(
                "catalog {} failed SQLite integrity_check: {integrity}",
                path.display()
            )));
        }
        evidence.sqlite_integrity_wall_us = elapsed_us(phase_started.elapsed());
        evidence.sqlite_integrity_passes = 1;
        evidence.sqlite_integrity_bytes_covered = expected.artifact.size;

        let phase_started = Instant::now();
        let stored = read_binding(&connection, expected.artifact.clone())?;
        if &stored != expected {
            return Err(Error::ConflictError(format!(
                "catalog {} metadata does not match its exact manifest binding",
                path.display()
            )));
        }
        evidence.stored_binding_wall_us = elapsed_us(phase_started.elapsed());
        evidence.stored_binding_checks = 1;
        let mut reader = Self {
            path: canonical_path,
            binding: expected.clone(),
            file_anchor,
            connection: CatalogConnectionOwner::Direct(connection),
            verification_proof: None,
            verification_evidence: evidence,
        };
        if verify_logical_content {
            let phase_started = Instant::now();
            reader.verify_logical_content()?;
            reader.verification_evidence.logical_replay_wall_us =
                elapsed_us(phase_started.elapsed());
            reader.verification_evidence.logical_replay_passes = 1;
            reader.verification_proof = Some(CatalogVerificationProofV1::new(expected));
        }
        require_path_still_names_anchor(path, &reader.file_anchor)?;
        reader.verification_evidence.total_wall_us = elapsed_us(started.elapsed());
        #[cfg(test)]
        super::PHYSICAL_VERIFICATION_PASSES.set(super::PHYSICAL_VERIFICATION_PASSES.get() + 1);
        tracing::info!(
            catalog = %reader.path.display(),
            catalog_bytes = expected.artifact.size,
            verify_logical_content,
            userspace_sha256_passes = reader.verification_evidence.userspace_sha256_passes,
            sqlite_integrity_passes = reader.verification_evidence.sqlite_integrity_passes,
            logical_replay_passes = reader.verification_evidence.logical_replay_passes,
            elapsed_us = reader.verification_evidence.total_wall_us,
            "Immutable catalog reopen completed"
        );
        Ok(reader)
    }

    /// Reopen one independently addressed physical artifact while carrying
    /// forward a full logical verification of the exact same binding.
    ///
    /// This still checks file type, size, SHA-256, SQLite application/schema
    /// identity, integrity, and stored binding at the new path. The exact-byte
    /// proof carries the already-completed row cardinalities, foreign-key
    /// rejection, and logical-row verification, so none of those relation
    /// passes is repeated.
    pub(in crate::repository) fn open_verified_with_proof(
        path: impl AsRef<Path>,
        expected: &CatalogBindingV1,
        proof: &CatalogVerificationProofV1,
    ) -> Result<Self> {
        proof.require_binding(expected)?;
        let mut reader = Self::open_verified_inner(path.as_ref(), expected, false)?;
        reader.verification_proof = Some(proof.clone());
        Ok(reader)
    }

    /// Reopen one native projection-cache entry after its versioned cache
    /// manifest minted an exact durable logical attestation.
    ///
    /// Projection caches are not registered serving authority. This path keeps
    /// the complete userspace artifact hash and SQLite integrity check while
    /// carrying only the already-completed logical replay. Registered source
    /// and profile bundles must use [`Self::open_registered_portable`].
    pub(in crate::repository) fn open_verified_projection_cache_entry(
        path: impl AsRef<Path>,
        expected: &CatalogBindingV1,
        attestation: &CatalogDurableLogicalAttestationV1,
    ) -> Result<Self> {
        attestation.require_binding(expected)?;
        let mut reader = Self::open_verified_inner(path.as_ref(), expected, false)?;
        reader.verification_proof = Some(CatalogVerificationProofV1::new(expected));
        Ok(reader)
    }

    /// Reopen one registered catalog exclusively through its decoded portable
    /// chunk authority.
    ///
    /// The bundle owner must first authenticate and decode the mandatory proof
    /// sidecar against `expected.artifact`, then pass that exact manifest here.
    /// This constructor still anchors the no-follow carrier descriptor, rejects
    /// SQLite sidecars, validates size, application identity, and the complete
    /// embedded binding, but performs no whole-artifact SHA-256, SQLite
    /// integrity scan, or logical replay. Every byte SQLite receives is instead
    /// authenticated by [`PortableCatalogConnection`].
    pub(in crate::repository) fn open_registered_portable(
        path: impl AsRef<Path>,
        expected: &CatalogBindingV1,
        attestation: &CatalogDurableLogicalAttestationV1,
        portable_manifest: PortableChunkManifestV1,
    ) -> Result<Self> {
        let started = Instant::now();
        let mut evidence = CatalogVerificationEvidenceV1 {
            catalog_bytes: expected.artifact.size,
            ..CatalogVerificationEvidenceV1::default()
        };
        expected.validate()?;
        attestation.require_binding(expected)?;
        let path = path.as_ref();

        let phase_started = Instant::now();
        let (canonical_path, file_anchor, metadata) = open_catalog_anchor(path)?;
        reject_nonempty_sidecars(path)?;
        if metadata.len() != expected.artifact.size {
            return Err(Error::ChecksumMismatch {
                expected: format!("{} bytes", expected.artifact.size),
                actual: format!("{} bytes", metadata.len()),
            });
        }
        evidence.layout_sidecars_wall_us = elapsed_us(phase_started.elapsed());

        let phase_started = Instant::now();
        if portable_manifest.catalog_size() != expected.artifact.size {
            return Err(Error::ChecksumMismatch {
                expected: format!("{} bytes", expected.artifact.size),
                actual: format!("{} bytes", portable_manifest.catalog_size()),
            });
        }
        let portable_artifact_sha256 = portable_manifest.artifact_sha256();
        if portable_artifact_sha256 != expected.artifact.sha256 {
            return Err(Error::ChecksumMismatch {
                expected: expected.artifact.sha256.clone(),
                actual: portable_artifact_sha256,
            });
        }
        evidence.portable_manifest_validation_passes = 1;
        evidence.portable_manifest_validation_bytes =
            portable_manifest_size_v1(portable_manifest.chunk_count()).map_err(|error| {
                Error::ConflictError(format!(
                    "decoded portable chunk authority has no canonical encoded size: {error}"
                ))
            })?;
        evidence.artifact_identity_wall_us = elapsed_us(phase_started.elapsed());

        let phase_started = Instant::now();
        let connection =
            PortableCatalogConnection::open(file_anchor.try_clone()?, portable_manifest)?;
        let application_id: i64 = portable_catalog_result(
            &connection,
            connection
                .connection()
                .query_row("PRAGMA application_id", [], |row| row.get(0))
                .map_err(Error::from),
        )?;
        if application_id != CATALOG_APPLICATION_ID {
            return Err(Error::ConfigError(format!(
                "catalog {} has application id {application_id:#x}; expected {CATALOG_APPLICATION_ID:#x}",
                path.display()
            )));
        }
        evidence.sqlite_open_header_wall_us = elapsed_us(phase_started.elapsed());

        let phase_started = Instant::now();
        let stored = portable_catalog_result(
            &connection,
            read_binding(connection.connection(), expected.artifact.clone()),
        )?;
        if &stored != expected {
            return Err(Error::ConflictError(format!(
                "catalog {} metadata does not match its exact manifest binding",
                path.display()
            )));
        }
        evidence.stored_binding_wall_us = elapsed_us(phase_started.elapsed());
        evidence.stored_binding_checks = 1;

        let mut reader = Self {
            path: canonical_path,
            binding: expected.clone(),
            file_anchor,
            connection: CatalogConnectionOwner::Portable(connection),
            verification_proof: Some(CatalogVerificationProofV1::new(expected)),
            verification_evidence: evidence,
        };
        require_path_still_names_anchor(path, &reader.file_anchor)?;
        reader.verification_evidence.total_wall_us = elapsed_us(started.elapsed());
        #[cfg(test)]
        super::PHYSICAL_VERIFICATION_PASSES.set(super::PHYSICAL_VERIFICATION_PASSES.get() + 1);
        tracing::info!(
            catalog = %reader.path.display(),
            catalog_bytes = expected.artifact.size,
            portable_manifest_validation_passes = reader.verification_evidence.portable_manifest_validation_passes,
            portable_manifest_validation_bytes = reader.verification_evidence.portable_manifest_validation_bytes,
            userspace_sha256_passes = 0,
            sqlite_integrity_passes = 0,
            logical_replay_passes = 0,
            elapsed_us = reader.verification_evidence.total_wall_us,
            "Portable registered catalog reopen completed"
        );
        Ok(reader)
    }

    /// Consume one directly verified publication candidate and build the
    /// canonical portable chunk authority from its retained exact descriptor.
    ///
    /// A signed-only reader cannot enter this path: publication must carry a
    /// complete logical proof for the exact binding before chunk authority is
    /// derived. The path is checked against the retained descriptor both
    /// before and after the single whole-artifact build pass.
    pub(in crate::repository) fn into_portable_chunk_manifest(
        self,
    ) -> Result<PortableChunkManifestV1> {
        self.verification_proof()?.require_binding(&self.binding)?;
        if !matches!(&self.connection, CatalogConnectionOwner::Direct(_)) {
            return Err(Error::ConflictError(
                "portable chunk authority may be built only from a directly verified publication candidate"
                    .to_string(),
            ));
        }
        require_path_still_names_anchor(&self.path, &self.file_anchor)?;
        let manifest = PortableChunkManifestV1::build(&self.file_anchor, &self.binding.artifact)
            .map_err(|error| match error {
                PortableIntegrityError::Io(error) => Error::Io(error),
                error => Error::ConflictError(format!(
                    "build portable chunk authority for catalog {}: {error}",
                    self.path.display()
                )),
            })?;
        require_path_still_names_anchor(&self.path, &self.file_anchor)?;
        Ok(manifest)
    }

    pub(in crate::repository) fn verification_proof(&self) -> Result<&CatalogVerificationProofV1> {
        self.verification_proof.as_ref().ok_or_else(|| {
            Error::ConflictError(
                "catalog reader has signed-artifact authority but no local logical replay proof"
                    .to_string(),
            )
        })
    }
}
