// conary-core/src/filesystem/cas/verified_batch.rs

//! Transaction-owned ingestion of signed objects into permanent CAS.

use super::{CasStore, stream::verify_existing_object};
use crate::error::Result;
use crate::hash::{HashAlgorithm, Hasher};
use crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Observable work performed by one verified-object transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerifiedObjectBatchMetrics {
    pub incoming_bytes_hashed: u64,
    pub persistent_bytes_written: u64,
    pub objects_hashed: u64,
    pub hits: u64,
    pub misses: u64,
    pub race_losers: u64,
    pub object_syncs: u64,
    pub shard_syncs: u64,
    pub root_syncs: u64,
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
            self.metrics.object_syncs += 1;
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
        if let Some(output) = output {
            output.sync_data()?;
        }
        Ok(())
    }

    /// Publish the complete verified set and make all names durable.
    pub fn commit(mut self) -> Result<VerifiedObjectSet> {
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

        let durability_result = self.sync_publication(&touched_shards);
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

    fn sync_publication(&mut self, touched_shards: &BTreeSet<PathBuf>) -> Result<()> {
        for shard in touched_shards {
            fs::File::open(shard)?.sync_all()?;
            self.metrics.shard_syncs += 1;
        }
        if !self.new_shards.is_empty() {
            fs::File::open(self.cas.objects_dir())?.sync_all()?;
            self.metrics.root_syncs += 1;
        }
        Ok(())
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
    if sha256.len() != HashAlgorithm::Sha256.hex_len()
        || !sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(crate::Error::InvalidPath(format!(
            "verified object identity must be canonical lowercase SHA-256: {sha256}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct RecordingReader<R> {
        inner: R,
        max_requested: usize,
    }

    impl<R: Read> Read for RecordingReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.max_requested = self.max_requested.max(buffer.len());
            self.inner.read(buffer)
        }
    }

    fn temp_entries(root: &Path) -> Vec<PathBuf> {
        fn visit(path: &Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = fs::read_dir(path) else {
                return;
            };
            for entry in entries {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    visit(&path, found);
                } else if entry.file_name().to_string_lossy().contains(".tmp.") {
                    found.push(path);
                }
            }
        }

        let mut found = Vec::new();
        visit(root, &mut found);
        found
    }

    #[test]
    fn cold_batch_writes_hashes_and_syncs_each_missing_object_once() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let first = b"first signed object";
        let second = vec![0xa5; PAYLOAD_IO_BUFFER_SIZE * 2 + 31];
        let first_hash = crate::hash::sha256(first);
        let second_hash = crate::hash::sha256(&second);
        let mut batch = cas
            .verified_object_batch([
                (first_hash.clone(), first.len() as u64),
                (second_hash.clone(), second.len() as u64),
            ])
            .unwrap();

        assert_eq!(
            batch
                .ingest(&second_hash, &mut Cursor::new(&second))
                .unwrap(),
            VerifiedObjectDisposition::Staged
        );
        assert_eq!(
            batch.ingest(&first_hash, &mut Cursor::new(first)).unwrap(),
            VerifiedObjectDisposition::Staged
        );
        assert!(!cas.exists(&first_hash));
        assert!(!cas.exists(&second_hash));

        let verified = batch.commit().unwrap();
        assert_eq!(
            fs::read(verified.object_path(&first_hash).unwrap()).unwrap(),
            first
        );
        assert_eq!(
            fs::read(verified.object_path(&second_hash).unwrap()).unwrap(),
            second
        );
        assert_eq!(verified.objects().len(), 2);
        assert_eq!(
            verified.metrics(),
            VerifiedObjectBatchMetrics {
                incoming_bytes_hashed: (first.len() + second.len()) as u64,
                persistent_bytes_written: (first.len() + second.len()) as u64,
                objects_hashed: 2,
                hits: 0,
                misses: 2,
                race_losers: 0,
                object_syncs: 2,
                shard_syncs: 2,
                root_syncs: 1,
                canonical_bytes_reread: 0,
            }
        );
    }

    #[test]
    fn warm_batch_hashes_incoming_once_without_write_or_canonical_reread() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let bytes = vec![0x7c; PAYLOAD_IO_BUFFER_SIZE + 9];
        let hash = crate::hash::sha256(&bytes);
        cas.store_reader_expected(&mut Cursor::new(&bytes), bytes.len() as u64, &hash)
            .unwrap();
        let mut reader = RecordingReader {
            inner: Cursor::new(&bytes),
            max_requested: 0,
        };
        let mut batch = cas
            .verified_object_batch([(hash.clone(), bytes.len() as u64)])
            .unwrap();

        assert_eq!(
            batch.ingest(&hash, &mut reader).unwrap(),
            VerifiedObjectDisposition::TrustedHit
        );
        let verified = batch.commit().unwrap();
        assert!(reader.max_requested <= PAYLOAD_IO_BUFFER_SIZE);
        assert_eq!(
            verified.metrics(),
            VerifiedObjectBatchMetrics {
                incoming_bytes_hashed: bytes.len() as u64,
                persistent_bytes_written: 0,
                objects_hashed: 1,
                hits: 1,
                misses: 0,
                race_losers: 0,
                object_syncs: 0,
                shard_syncs: 0,
                root_syncs: 0,
                canonical_bytes_reread: 0,
            }
        );
    }

    #[test]
    fn duplicate_signed_identity_is_deduplicated_but_conflicting_size_fails() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"deduplicated authority";
        let hash = crate::hash::sha256(bytes);
        let mut batch = cas
            .verified_object_batch([
                (hash.clone(), bytes.len() as u64),
                (hash.clone(), bytes.len() as u64),
            ])
            .unwrap();
        assert_eq!(batch.metrics().misses, 1);
        batch.ingest(&hash, &mut Cursor::new(bytes)).unwrap();
        assert_eq!(batch.commit().unwrap().objects().len(), 1);

        let conflict_cas = CasStore::new(temp.path().join("conflict-objects")).unwrap();
        let error = conflict_cas
            .verified_object_batch([(hash.clone(), 1), (hash, 2)])
            .err()
            .unwrap();
        assert!(error.to_string().contains("conflicting sizes"));
    }

    #[test]
    fn invalid_or_non_sha256_authority_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let xxh =
            CasStore::with_algorithm(temp.path().join("xxh"), crate::hash::HashAlgorithm::Xxh128)
                .unwrap();
        assert!(xxh.verified_object_batch([("abcd", 0)]).is_err());

        let cas = CasStore::new(temp.path().join("sha256")).unwrap();
        assert!(cas.verified_object_batch([("A".repeat(64), 0)]).is_err());
        assert!(cas.verified_object_batch([("0".repeat(63), 0)]).is_err());
    }

    fn assert_failed_stream_leaves_no_object(
        declared_size: u64,
        declared_hash: String,
        bytes: &[u8],
    ) {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let mut batch = cas
            .verified_object_batch([(declared_hash.clone(), declared_size)])
            .unwrap();
        assert!(
            batch
                .ingest(&declared_hash, &mut Cursor::new(bytes))
                .is_err()
        );
        assert!(batch.commit().is_err());
        assert!(!cas.exists(&declared_hash));
        assert!(temp_entries(cas.objects_dir()).is_empty());
    }

    #[test]
    fn partial_oversized_and_digest_mismatch_streams_poison_and_clean_batch() {
        let bytes = b"signed bytes";
        let hash = crate::hash::sha256(bytes);
        assert_failed_stream_leaves_no_object(bytes.len() as u64 + 1, hash.clone(), bytes);
        assert_failed_stream_leaves_no_object(bytes.len() as u64 - 1, hash, bytes);
        assert_failed_stream_leaves_no_object(bytes.len() as u64, "0".repeat(64), bytes);
    }

    #[test]
    fn one_failure_removes_other_staged_objects() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let good = b"good";
        let bad = b"bad";
        let good_hash = crate::hash::sha256(good);
        let bad_hash = crate::hash::sha256(b"different");
        let mut batch = cas
            .verified_object_batch([
                (good_hash.clone(), good.len() as u64),
                (bad_hash.clone(), bad.len() as u64),
            ])
            .unwrap();
        batch.ingest(&good_hash, &mut Cursor::new(good)).unwrap();
        assert!(batch.ingest(&bad_hash, &mut Cursor::new(bad)).is_err());
        assert!(!cas.exists(&good_hash));
        assert!(temp_entries(cas.objects_dir()).is_empty());
    }

    #[test]
    fn concurrent_publication_loser_validates_exact_winner() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"same authenticated object";
        let hash = crate::hash::sha256(bytes);
        let expected = [(hash.clone(), bytes.len() as u64)];
        let mut first = cas.verified_object_batch(expected.clone()).unwrap();
        let mut second = cas.verified_object_batch(expected).unwrap();
        first.ingest(&hash, &mut Cursor::new(bytes)).unwrap();
        second.ingest(&hash, &mut Cursor::new(bytes)).unwrap();

        first.commit().unwrap();
        let loser = second.commit().unwrap();
        assert_eq!(loser.metrics().race_losers, 1);
        assert_eq!(loser.metrics().canonical_bytes_reread, bytes.len() as u64);
        assert_eq!(fs::read(loser.object_path(&hash).unwrap()).unwrap(), bytes);
        assert!(temp_entries(cas.objects_dir()).is_empty());
    }

    #[test]
    fn corrupt_publication_winner_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"expected bytes";
        let hash = crate::hash::sha256(bytes);
        let mut batch = cas
            .verified_object_batch([(hash.clone(), bytes.len() as u64)])
            .unwrap();
        batch.ingest(&hash, &mut Cursor::new(bytes)).unwrap();
        let path = cas.hash_to_path(&hash).unwrap();
        fs::write(&path, b"wronged bytes!").unwrap();

        let error = batch.commit().unwrap_err();
        assert!(error.to_string().contains("Checksum mismatch"));
        assert!(temp_entries(cas.objects_dir()).is_empty());
    }

    #[test]
    fn dropped_or_incomplete_batch_exposes_no_canonical_objects() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let first = b"staged then abandoned";
        let second = b"never provided";
        let first_hash = crate::hash::sha256(first);
        let second_hash = crate::hash::sha256(second);
        {
            let mut batch = cas
                .verified_object_batch([
                    (first_hash.clone(), first.len() as u64),
                    (second_hash, second.len() as u64),
                ])
                .unwrap();
            batch.ingest(&first_hash, &mut Cursor::new(first)).unwrap();
            assert!(batch.commit().is_err());
        }
        assert!(!cas.exists(&first_hash));
        assert!(temp_entries(cas.objects_dir()).is_empty());
    }

    #[test]
    fn ingestion_requests_only_the_shared_payload_buffer_size() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let bytes = vec![0x42; PAYLOAD_IO_BUFFER_SIZE * 3 + 1];
        let hash = crate::hash::sha256(&bytes);
        let mut reader = RecordingReader {
            inner: Cursor::new(&bytes),
            max_requested: 0,
        };
        let mut batch = cas
            .verified_object_batch([(hash.clone(), bytes.len() as u64)])
            .unwrap();
        batch.ingest(&hash, &mut reader).unwrap();
        batch.commit().unwrap();
        assert!(reader.max_requested <= PAYLOAD_IO_BUFFER_SIZE);
    }
}
