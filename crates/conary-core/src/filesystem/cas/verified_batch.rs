// crates/conary-core/src/filesystem/cas/verified_batch.rs

//! Transaction-owned ingestion of signed objects into permanent CAS.

use super::{CasStore, stream::verify_existing_object};
use crate::error::Result;
use crate::hash::{HashAlgorithm, Hasher};
use crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

mod durability;

use durability::{FilesystemVerifiedObjectDurability, VerifiedObjectDurability};

/// Observable work performed by one verified-object transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerifiedObjectBatchMetrics {
    pub incoming_bytes_hashed: u64,
    pub persistent_bytes_written: u64,
    pub objects_hashed: u64,
    pub hits: u64,
    pub misses: u64,
    pub race_losers: u64,
    pub staged_data_barriers: u64,
    pub canonical_name_barriers: u64,
    pub fallback_object_syncs: u64,
    pub fallback_directory_syncs: u64,
    pub canonical_bytes_reread: u64,
}

/// Result of ingesting one authenticated object stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifiedObjectDisposition {
    Staged,
    TrustedHit,
}

/// Complete, durable object authority returned only by a committed batch.
#[derive(Clone, Debug)]
pub struct VerifiedObjectSet {
    cas: CasStore,
    objects: BTreeMap<String, u64>,
    metrics: VerifiedObjectBatchMetrics,
}

impl VerifiedObjectSet {
    pub fn contains(&self, sha256: &str) -> bool {
        self.objects.contains_key(sha256)
    }

    pub fn object_path(&self, sha256: &str) -> Result<PathBuf> {
        if !self.contains(sha256) {
            return Err(crate::Error::NotFound(format!(
                "SHA-256 object {sha256} is not part of the verified set"
            )));
        }
        self.cas.hash_to_path(sha256)
    }

    pub fn objects(&self) -> impl ExactSizeIterator<Item = (&str, u64)> {
        self.objects
            .iter()
            .map(|(sha256, size)| (sha256.as_str(), *size))
    }

    pub fn metrics(&self) -> VerifiedObjectBatchMetrics {
        self.metrics
    }

    pub(crate) fn authorizes(&self, cas: &CasStore, sha256: &str, size: u64) -> bool {
        self.cas.algorithm == cas.algorithm
            && self.cas.objects_dir == cas.objects_dir
            && self.objects.get(sha256) == Some(&size)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectState {
    Missing,
    TrustedHit,
    IngestedMissing,
    IngestedHit,
}

#[derive(Debug)]
struct ExpectedObject {
    size: u64,
    state: ObjectState,
}

#[derive(Debug)]
struct StagedObject {
    canonical_path: PathBuf,
    temp_path: PathBuf,
    size: u64,
}

/// A complete expected set of signed SHA-256 objects.
///
/// A failed ingest poisons the batch and removes all staged files. Dropping an
/// uncommitted batch likewise removes staged files and exposes no typed object
/// set that a database transaction could reference.
pub struct VerifiedObjectBatch<'a> {
    cas: &'a CasStore,
    expected: BTreeMap<String, ExpectedObject>,
    staged: BTreeMap<String, StagedObject>,
    new_shards: BTreeSet<PathBuf>,
    metrics: VerifiedObjectBatchMetrics,
    poisoned: bool,
}

impl<'a> VerifiedObjectBatch<'a> {
    pub(super) fn new<I, S>(cas: &'a CasStore, expected: I) -> Result<Self>
    where
        I: IntoIterator<Item = (S, u64)>,
        S: Into<String>,
    {
        if cas.algorithm() != HashAlgorithm::Sha256 {
            return Err(crate::Error::ConfigError(format!(
                "verified object batches require SHA-256 CAS authority, found {}",
                cas.algorithm()
            )));
        }

        let mut objects: BTreeMap<String, ExpectedObject> = BTreeMap::new();
        let mut metrics = VerifiedObjectBatchMetrics::default();
        for (sha256, size) in expected {
            let sha256 = sha256.into();
            validate_sha256(&sha256)?;
            if let Some(previous) = objects.get(&sha256) {
                if previous.size != size {
                    return Err(crate::Error::ConflictError(format!(
                        "signed object {sha256} has conflicting sizes {} and {size}",
                        previous.size
                    )));
                }
                continue;
            }

            let path = cas.hash_to_path(&sha256)?;
            let state = match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if !metadata.file_type().is_file() {
                        return Err(crate::Error::IoError(format!(
                            "trusted CAS object {} is not a regular file",
                            path.display()
                        )));
                    }
                    if metadata.len() != size {
                        return Err(crate::Error::IoError(format!(
                            "trusted CAS object {} has size {}, expected {size}",
                            path.display(),
                            metadata.len()
                        )));
                    }
                    metrics.hits += 1;
                    ObjectState::TrustedHit
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    metrics.misses += 1;
                    ObjectState::Missing
                }
                Err(error) => return Err(error.into()),
            };
            objects.insert(sha256, ExpectedObject { size, state });
        }

        Ok(Self {
            cas,
            expected: objects,
            staged: BTreeMap::new(),
            new_shards: BTreeSet::new(),
            metrics,
            poisoned: false,
        })
    }

    pub fn metrics(&self) -> VerifiedObjectBatchMetrics {
        self.metrics
    }

    /// Admit an exact-size canonical hit without downloading or rereading it.
    ///
    /// CAS object names are immutable verified-content authority. The batch
    /// still rechecks file type and size here and again at commit so a caller
    /// can fetch only missing repository objects without weakening the commit
    /// boundary.
    pub fn reuse_trusted(&mut self, sha256: &str) -> Result<bool> {
        if self.poisoned {
            return Err(crate::Error::ConflictError(
                "verified object batch is poisoned by an earlier failure".into(),
            ));
        }
        let (size, state) = self
            .expected
            .get(sha256)
            .map(|object| (object.size, object.state))
            .ok_or_else(|| {
                crate::Error::NotFound(format!(
                    "incoming SHA-256 object {sha256} was not declared by the signed authority"
                ))
            })?;
        match state {
            ObjectState::Missing => Ok(false),
            ObjectState::TrustedHit => {
                let path = self.cas.hash_to_path(sha256)?;
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.file_type().is_file() || metadata.len() != size {
                    return Err(crate::Error::IoError(format!(
                        "trusted CAS object {} changed before reuse",
                        path.display()
                    )));
                }
                self.expected.get_mut(sha256).expect("object exists").state =
                    ObjectState::IngestedHit;
                Ok(true)
            }
            ObjectState::IngestedMissing | ObjectState::IngestedHit => {
                Err(crate::Error::ConflictError(format!(
                    "signed object {sha256} was admitted more than once"
                )))
            }
        }
    }

    /// Consume and authenticate one expected stream with bounded memory.
    pub fn ingest(
        &mut self,
        sha256: &str,
        reader: &mut dyn Read,
    ) -> Result<VerifiedObjectDisposition> {
        if self.poisoned {
            return Err(crate::Error::ConflictError(
                "verified object batch is poisoned by an earlier failure".into(),
            ));
        }
        let result = self.ingest_inner(sha256, reader);
        if result.is_err() {
            self.poisoned = true;
            self.remove_staged();
        }
        result
    }

    fn ingest_inner(
        &mut self,
        sha256: &str,
        reader: &mut dyn Read,
    ) -> Result<VerifiedObjectDisposition> {
        let (size, state) = self
            .expected
            .get(sha256)
            .map(|object| (object.size, object.state))
            .ok_or_else(|| {
                crate::Error::NotFound(format!(
                    "incoming SHA-256 object {sha256} was not declared by the signed authority"
                ))
            })?;
        if matches!(
            state,
            ObjectState::IngestedMissing | ObjectState::IngestedHit
        ) {
            return Err(crate::Error::ConflictError(format!(
                "signed object {sha256} was ingested more than once"
            )));
        }

        let missing = state == ObjectState::Missing;
        let staged = if missing {
            Some(self.create_staged_object(sha256, size)?)
        } else {
            None
        };
        let temp_path = staged.as_ref().map(|object| object.temp_path.clone());
        let result = self.consume_stream(reader, size, sha256, temp_path.as_deref());
        if let Err(error) = result {
            if let Some(path) = temp_path {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }

        let disposition = if let Some(staged) = staged {
            self.staged.insert(sha256.to_string(), staged);
            self.expected.get_mut(sha256).expect("object exists").state =
                ObjectState::IngestedMissing;
            VerifiedObjectDisposition::Staged
        } else {
            self.expected.get_mut(sha256).expect("object exists").state = ObjectState::IngestedHit;
            VerifiedObjectDisposition::TrustedHit
        };
        Ok(disposition)
    }

    fn create_staged_object(&mut self, sha256: &str, size: u64) -> Result<StagedObject> {
        let canonical_path = self.cas.hash_to_path(sha256)?;
        let shard = canonical_path.parent().ok_or_else(|| {
            crate::Error::InternalError(format!("CAS object {sha256} has no shard directory"))
        })?;
        match fs::create_dir(shard) {
            Ok(()) => {
                self.new_shards.insert(shard.to_path_buf());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let temp_path = canonical_path.with_extension(format!(
            "tmp.{}.{}.verified-batch",
            std::process::id(),
            CasStore::next_temp_id()
        ));
        Ok(StagedObject {
            canonical_path,
            temp_path,
            size,
        })
    }

    fn consume_stream(
        &mut self,
        reader: &mut dyn Read,
        expected_size: u64,
        expected_sha256: &str,
        temp_path: Option<&Path>,
    ) -> Result<()> {
        let mut output = match temp_path {
            Some(path) => Some(OpenOptions::new().write(true).create_new(true).open(path)?),
            None => None,
        };
        let mut hasher = Hasher::new(HashAlgorithm::Sha256);
        let mut total = 0_u64;
        let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total = total
                .checked_add(read as u64)
                .ok_or_else(|| crate::Error::IoError("verified object size overflow".into()))?;
            if total > expected_size {
                return Err(crate::Error::IoError(format!(
                    "object {expected_sha256} exceeds signed size {expected_size}"
                )));
            }
            hasher.update(&buffer[..read]);
            self.metrics.incoming_bytes_hashed += read as u64;
            if let Some(output) = output.as_mut() {
                output.write_all(&buffer[..read])?;
                self.metrics.persistent_bytes_written += read as u64;
            }
        }
        if total != expected_size {
            return Err(crate::Error::IoError(format!(
                "object {expected_sha256} size mismatch: expected {expected_size}, got {total}"
            )));
        }
        let actual = hasher.finalize().value;
        self.metrics.objects_hashed += 1;
        if actual != expected_sha256 {
            return Err(crate::Error::ChecksumMismatch {
                expected: expected_sha256.to_string(),
                actual,
            });
        }
        drop(output);
        Ok(())
    }

    /// Publish the complete verified set and make all names durable.
    pub fn commit(self) -> Result<VerifiedObjectSet> {
        let mut durability = FilesystemVerifiedObjectDurability;
        self.commit_with_durability(&mut durability)
    }

    fn commit_with_durability(
        mut self,
        durability: &mut impl VerifiedObjectDurability,
    ) -> Result<VerifiedObjectSet> {
        if self.poisoned {
            return Err(crate::Error::ConflictError(
                "cannot commit a poisoned verified object batch".into(),
            ));
        }
        let missing = self
            .expected
            .iter()
            .find(|(_, object)| {
                matches!(object.state, ObjectState::Missing | ObjectState::TrustedHit)
            })
            .map(|(sha256, _)| sha256.clone());
        if let Some(sha256) = missing {
            return Err(crate::Error::ConflictError(format!(
                "cannot commit before signed object {sha256} is ingested"
            )));
        }

        durability.sync_staged_data(self.cas, &self.staged, &mut self.metrics)?;

        let mut touched_shards = BTreeSet::new();
        let mut publish_result = Ok(());
        for (sha256, staged) in &self.staged {
            let shard = staged
                .canonical_path
                .parent()
                .expect("CAS object has shard")
                .to_path_buf();
            touched_shards.insert(shard);
            match fs::hard_link(&staged.temp_path, &staged.canonical_path) {
                Ok(()) => {
                    if let Err(error) = fs::remove_file(&staged.temp_path) {
                        publish_result = Err(error.into());
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Err(error) =
                        verify_existing_object(&staged.canonical_path, staged.size, sha256)
                    {
                        publish_result = Err(error);
                        break;
                    }
                    self.metrics.race_losers += 1;
                    self.metrics.canonical_bytes_reread += staged.size;
                    if let Err(error) = fs::remove_file(&staged.temp_path) {
                        publish_result = Err(error.into());
                        break;
                    }
                }
                Err(error) => {
                    publish_result = Err(error.into());
                    break;
                }
            }
        }

        // Even when publication fails partway through, make every completed
        // canonical link durable. Those objects are valid but unreachable
        // because no complete VerifiedObjectSet can escape this error.
        let durability_result = durability.sync_canonical_names(
            self.cas,
            &self.staged,
            &touched_shards,
            &self.new_shards,
            &mut self.metrics,
        );
        publish_result?;
        durability_result?;

        for (sha256, object) in &self.expected {
            if object.state == ObjectState::IngestedHit {
                let path = self.cas.hash_to_path(sha256)?;
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.file_type().is_file() || metadata.len() != object.size {
                    return Err(crate::Error::IoError(format!(
                        "trusted CAS object {} changed before verified batch commit",
                        path.display()
                    )));
                }
            }
        }

        self.staged.clear();
        let objects = self
            .expected
            .iter()
            .map(|(sha256, object)| (sha256.clone(), object.size))
            .collect();
        Ok(VerifiedObjectSet {
            cas: self.cas.clone(),
            objects,
            metrics: self.metrics,
        })
    }

    fn remove_staged(&mut self) {
        for staged in self.staged.values() {
            let _ = fs::remove_file(&staged.temp_path);
        }
        self.staged.clear();
    }
}

impl Drop for VerifiedObjectBatch<'_> {
    fn drop(&mut self) {
        self.remove_staged();
    }
}

fn validate_sha256(sha256: &str) -> Result<()> {
    if !crate::hash::is_canonical_sha256(sha256) {
        return Err(crate::Error::InvalidPath(format!(
            "verified object identity must be canonical lowercase SHA-256: {sha256}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
