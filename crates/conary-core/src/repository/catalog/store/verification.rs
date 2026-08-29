// crates/conary-core/src/repository/catalog/store/verification.rs

//! Immutable reopen checks and non-serializable exact-artifact proofs.

use std::fs;
use std::path::Path;
use std::time::Instant;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::util::{
    conversion_error, hash_file, parse_json_column, read_u64, reject_nonempty_sidecars,
};
use super::{CATALOG_APPLICATION_ID, CATALOG_CONTENT_SCHEMA_V1, CatalogBindingV1, CatalogReader};
use crate::error::{Error, Result};
use crate::repository::catalog::{CatalogArtifactV1, CatalogCountsV1};

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

/// Proof that one exact catalog binding has already passed the complete
/// logical row replay in this process.
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

impl CatalogReader {
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
        expected.validate()?;
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            Error::IoError(format!(
                "inspect immutable catalog {}: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(Error::InvalidPath(format!(
                "immutable catalog {} must be a regular file, never a symlink",
                path.display()
            )));
        }
        reject_nonempty_sidecars(path)?;
        if metadata.len() != expected.artifact.size {
            return Err(Error::ChecksumMismatch {
                expected: format!("{} bytes", expected.artifact.size),
                actual: format!("{} bytes", metadata.len()),
            });
        }
        let actual_sha256 = hash_file(path)?;
        if actual_sha256 != expected.artifact.sha256 {
            return Err(Error::ChecksumMismatch {
                expected: expected.artifact.sha256.clone(),
                actual: actual_sha256,
            });
        }

        let canonical_path = path.canonicalize()?;
        let mut uri = url::Url::from_file_path(&canonical_path).map_err(|_| {
            Error::InvalidPath(format!(
                "catalog path {} cannot be represented as an immutable SQLite URI",
                canonical_path.display()
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
        let application_id: i64 =
            connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
        if application_id != CATALOG_APPLICATION_ID {
            return Err(Error::ConfigError(format!(
                "catalog {} has application id {application_id:#x}; expected {CATALOG_APPLICATION_ID:#x}",
                path.display()
            )));
        }
        let integrity: String =
            connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(Error::InitError(format!(
                "catalog {} failed SQLite integrity_check: {integrity}",
                path.display()
            )));
        }
        let stored = read_binding(&connection, expected.artifact.clone())?;
        if &stored != expected {
            return Err(Error::ConflictError(format!(
                "catalog {} metadata does not match its exact manifest binding",
                path.display()
            )));
        }
        let mut reader = Self {
            path: canonical_path,
            binding: expected.clone(),
            connection,
            verification_proof: None,
        };
        if verify_logical_content {
            reader.verify_logical_content()?;
            reader.verification_proof = Some(CatalogVerificationProofV1::new(expected));
        }
        #[cfg(test)]
        super::PHYSICAL_VERIFICATION_PASSES.set(super::PHYSICAL_VERIFICATION_PASSES.get() + 1);
        tracing::info!(
            catalog = %reader.path.display(),
            catalog_bytes = expected.artifact.size,
            verify_logical_content,
            elapsed_ms = started.elapsed().as_millis(),
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

    /// Reopen exact bytes covered by a versioned durable logical attestation.
    ///
    /// File type and sidecars, byte size and SHA-256, SQLite application/schema
    /// identity and integrity plus the stored binding are all checked at this
    /// path. The exact-byte attestation carries the row cardinalities,
    /// foreign-key rejection, and logical-row verification required by its
    /// publisher, so none of those relation passes is repeated.
    pub(in crate::repository) fn open_verified_with_durable_attestation(
        path: impl AsRef<Path>,
        expected: &CatalogBindingV1,
        attestation: &CatalogDurableLogicalAttestationV1,
    ) -> Result<Self> {
        attestation.require_binding(expected)?;
        let mut reader = Self::open_verified_inner(path.as_ref(), expected, false)?;
        reader.verification_proof = Some(CatalogVerificationProofV1::new(expected));
        Ok(reader)
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
