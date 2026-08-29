// crates/conary-core/src/generation/artifact/cas.rs

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use super::{sha256_file, validate_sha256_hex};
use crate::filesystem::CasObjectLivenessLease;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CasManifest {
    pub version: u32,
    pub generation: i64,
    pub architecture: String,
    pub objects: Vec<CasObjectRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CasObjectRef {
    pub sha256: String,
    pub size: u64,
}

/// Exact evidence that every object existed at its authoritative path and size.
///
/// Only [`verify_cas_object_presence`] can mint this value. Its private bindings
/// let the artifact writer reuse a just-completed runtime preflight without
/// turning a caller assertion into CAS authority. The value also retains a
/// shared object-liveness lease; runtime builders keep it through completion so
/// Conary garbage collection cannot invalidate the proof before publication.
#[derive(Debug)]
pub struct VerifiedCasObjectPresence<'objects> {
    canonical_cas_dir: PathBuf,
    objects: &'objects [CasObjectRef],
    _liveness: CasObjectLivenessLease,
}

impl VerifiedCasObjectPresence<'_> {
    pub(super) fn require_exact_match(
        &self,
        canonical_cas_dir: &Path,
        objects: &[CasObjectRef],
    ) -> crate::Result<()> {
        if self.canonical_cas_dir != canonical_cas_dir {
            return Err(crate::Error::InvalidPath(format!(
                "CAS presence proof is bound to {}, not {}",
                self.canonical_cas_dir.display(),
                canonical_cas_dir.display()
            )));
        }
        if self.objects != objects {
            return Err(crate::Error::ConflictError(
                "CAS presence proof object set does not match the artifact object set".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum CasObjectVerification<'a> {
    /// Reopen and hash every referenced CAS object.
    Deep,
    /// Reopen every authoritative CAS path and verify its recorded size.
    AlreadyVerified,
    /// Reuse one exact presence-and-size check under its retained liveness lease.
    VerifiedPresence(&'a VerifiedCasObjectPresence<'a>),
}

pub fn deduplicate_sort_cas_objects(
    objects: Vec<CasObjectRef>,
) -> crate::Result<Vec<CasObjectRef>> {
    let mut by_hash = BTreeMap::new();
    for object in objects {
        validate_sha256_hex("CAS object sha256", &object.sha256)?;
        match by_hash.insert(object.sha256.clone(), object.size) {
            Some(existing_size) if existing_size != object.size => {
                return Err(crate::Error::ConflictError(format!(
                    "CAS object {} has conflicting sizes: {existing_size} and {}",
                    object.sha256, object.size
                )));
            }
            _ => {}
        }
    }

    Ok(by_hash
        .into_iter()
        .map(|(sha256, size)| CasObjectRef { sha256, size })
        .collect())
}

pub(crate) fn verify_cas_object_presence<'objects>(
    cas_dir: &Path,
    objects: &'objects [CasObjectRef],
) -> crate::Result<VerifiedCasObjectPresence<'objects>> {
    let liveness = CasObjectLivenessLease::acquire(cas_dir)?;
    let canonical_cas_dir = liveness.canonical_objects_dir().to_path_buf();
    require_deduplicated_sorted_cas_objects(objects)?;
    verify_cas_object_files_exist_with_expected_sizes(&canonical_cas_dir, objects)?;
    Ok(VerifiedCasObjectPresence {
        canonical_cas_dir,
        objects,
        _liveness: liveness,
    })
}

fn require_deduplicated_sorted_cas_objects(objects: &[CasObjectRef]) -> crate::Result<()> {
    let mut previous = None;
    for object in objects {
        validate_sha256_hex("CAS object sha256", &object.sha256)?;
        if previous.is_some_and(|sha256: &str| sha256 >= object.sha256.as_str()) {
            return Err(crate::Error::ConflictError(
                "CAS presence proof requires an exact deduplicated, sorted object set".to_string(),
            ));
        }
        previous = Some(object.sha256.as_str());
    }
    Ok(())
}

pub(crate) fn verify_cas_objects(cas_dir: &Path, objects: &[CasObjectRef]) -> crate::Result<()> {
    let mut seen = HashSet::new();
    for object in objects {
        validate_sha256_hex("CAS object sha256", &object.sha256)?;
        if !seen.insert(object.sha256.clone()) {
            return Err(crate::Error::ConflictError(format!(
                "duplicate CAS manifest entry for {}",
                object.sha256
            )));
        }

        let object_path = crate::filesystem::object_path(cas_dir, &object.sha256)?;
        let metadata = std::fs::metadata(&object_path).map_err(|e| {
            crate::Error::NotFound(format!(
                "missing CAS object {} at {}: {e}",
                object.sha256,
                object_path.display()
            ))
        })?;
        if metadata.len() != object.size {
            return Err(crate::Error::InvalidPath(format!(
                "CAS object {} size mismatch: expected {}, got {}",
                object.sha256,
                object.size,
                metadata.len()
            )));
        }
        let actual = sha256_file(&object_path)?;
        if actual != object.sha256 {
            return Err(crate::Error::ChecksumMismatch {
                expected: format!("CAS object SHA-256 {}", object.sha256),
                actual,
            });
        }
    }
    Ok(())
}

pub(crate) fn verify_cas_object_files_exist_with_expected_sizes(
    cas_dir: &Path,
    objects: &[CasObjectRef],
) -> crate::Result<()> {
    for object in objects {
        let object_path = crate::filesystem::object_path(cas_dir, &object.sha256)?;
        let metadata = std::fs::metadata(&object_path).map_err(|e| {
            crate::Error::NotFound(format!(
                "missing CAS object {} at {}: {e}",
                object.sha256,
                object_path.display()
            ))
        })?;
        if metadata.len() != object.size {
            return Err(crate::Error::InvalidPath(format!(
                "CAS object {} size mismatch: expected {}, got {}",
                object.sha256,
                object.size,
                metadata.len()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
