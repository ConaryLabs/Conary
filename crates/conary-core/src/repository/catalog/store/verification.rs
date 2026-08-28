// crates/conary-core/src/repository/catalog/store/verification.rs

//! Non-serializable proof carried between same-process immutable reopens.

use std::path::Path;

use super::{CatalogBindingV1, CatalogReader};
use crate::error::{Error, Result};

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
    /// Reopen one independently addressed physical artifact while carrying
    /// forward a full logical verification of the exact same binding.
    ///
    /// This still checks file type, size, SHA-256, SQLite application/schema
    /// identity, integrity, stored binding, relational counts, and foreign
    /// keys at the new path. Only the redundant Rust row reconstruction and
    /// logical re-digest are omitted.
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
    /// identity and integrity, stored binding, relational counts, and foreign
    /// keys are all checked at this path. The attestation permits omitting only
    /// the normalized-row reconstruction and logical re-digest already required
    /// by its publisher.
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
