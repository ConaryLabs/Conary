// crates/conary-core/src/repository/sync/immutable_catalog/reuse.rs

//! Typed ownership carried by durable native-catalog reuse.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::repository::catalog::{
    CatalogReader, PortableManifestAttestationV1, SourceSnapshotV1, portable_chunk_count_v1,
    portable_manifest_size_v1,
};

/// One exact registered source snapshot eligible for reuse after the current
/// upstream metadata independently authenticates to the same authority.
#[derive(Debug, Clone)]
pub struct DurableSourceCatalogReuseV1 {
    manifest: SourceSnapshotV1,
    bundle_path: PathBuf,
    portable_manifest_attestation: PortableManifestAttestationV1,
}

impl DurableSourceCatalogReuseV1 {
    pub fn new(
        manifest: SourceSnapshotV1,
        bundle_path: PathBuf,
        portable_manifest_attestation: PortableManifestAttestationV1,
    ) -> Result<Self> {
        manifest.validate()?;
        portable_manifest_attestation
            .validate()
            .map_err(|error| Error::ConfigError(error.to_string()))?;
        let chunk_count = portable_chunk_count_v1(manifest.catalog.size)
            .map_err(|error| Error::ConfigError(error.to_string()))?;
        let expected_manifest_size = portable_manifest_size_v1(chunk_count)
            .map_err(|error| Error::ConfigError(error.to_string()))?;
        if portable_manifest_attestation.size != expected_manifest_size {
            return Err(Error::ConfigError(format!(
                "durable source portable manifest has {} bytes; expected {expected_manifest_size}",
                portable_manifest_attestation.size
            )));
        }
        Ok(Self {
            manifest,
            bundle_path,
            portable_manifest_attestation,
        })
    }

    #[must_use]
    pub fn manifest(&self) -> &SourceSnapshotV1 {
        &self.manifest
    }

    #[must_use]
    pub fn bundle_path(&self) -> &Path {
        &self.bundle_path
    }

    #[must_use]
    pub fn portable_manifest_attestation(&self) -> &PortableManifestAttestationV1 {
        &self.portable_manifest_attestation
    }
}

/// How the verified source reader reached the profile-composition boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceCatalogMaterializationV1 {
    /// A new private catalog was constructed from authenticated native bytes.
    PrivateCandidate,
    /// An exact projection-cache entry was copied into a private candidate.
    ProjectionCacheCandidate,
    /// An exact registered durable bundle was reopened in place.
    DurableReuse {
        bundle_path: PathBuf,
        portable_manifest_attestation: PortableManifestAttestationV1,
    },
}

/// One source manifest paired with a process-local reader carrying its exact
/// logical binding. Private candidates complete logical verification in this
/// run; durable reuse authenticates the persisted proof and catalog bytes
/// through the registered portable VFS without replaying the full projection.
pub struct VerifiedSourceCatalogCandidateV1 {
    pub manifest: SourceSnapshotV1,
    pub reader: CatalogReader,
    pub materialization: SourceCatalogMaterializationV1,
}
