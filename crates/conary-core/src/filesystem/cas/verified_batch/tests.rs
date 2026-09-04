// conary-core/src/filesystem/cas/verified_batch/tests.rs

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObservedBarrier {
    StagedData,
    CanonicalNames,
}

#[derive(Default)]
struct InspectingDurability {
    events: Vec<ObservedBarrier>,
    fail_at: Option<ObservedBarrier>,
}

impl VerifiedObjectDurability for InspectingDurability {
    fn sync_staged_data(
        &mut self,
        _cas: &CasStore,
        staged: &BTreeMap<String, StagedObject>,
        metrics: &mut VerifiedObjectBatchMetrics,
    ) -> Result<()> {
        assert!(!staged.is_empty());
        for object in staged.values() {
            assert!(object.temp_path.is_file());
            assert!(!object.canonical_path.exists());
        }
        self.events.push(ObservedBarrier::StagedData);
        if self.fail_at == Some(ObservedBarrier::StagedData) {
            return Err(crate::Error::IoError(
                "injected staged-data barrier failure".into(),
            ));
        }
        metrics.staged_data_barriers += 1;
        Ok(())
    }

    fn sync_canonical_names(
        &mut self,
        _cas: &CasStore,
        staged: &BTreeMap<String, StagedObject>,
        _touched_shards: &BTreeSet<PathBuf>,
        _new_shards: &BTreeSet<PathBuf>,
        metrics: &mut VerifiedObjectBatchMetrics,
    ) -> Result<()> {
        assert!(!staged.is_empty());
        for object in staged.values() {
            assert!(!object.temp_path.exists());
            assert!(object.canonical_path.is_file());
        }
        self.events.push(ObservedBarrier::CanonicalNames);
        if self.fail_at == Some(ObservedBarrier::CanonicalNames) {
            return Err(crate::Error::IoError(
                "injected canonical-name barrier failure".into(),
            ));
        }
        metrics.canonical_name_barriers += 1;
        Ok(())
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
fn cold_batch_writes_hashes_once_and_uses_two_batch_barriers() {
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
    let expected_metrics = VerifiedObjectBatchMetrics {
        incoming_bytes_hashed: (first.len() + second.len()) as u64,
        persistent_bytes_written: (first.len() + second.len()) as u64,
        objects_hashed: 2,
        hits: 0,
        misses: 2,
        race_losers: 0,
        staged_data_barriers: 1,
        canonical_name_barriers: 1,
        fallback_object_syncs: 0,
        fallback_directory_syncs: 0,
        canonical_bytes_reread: 0,
    };
    #[cfg(not(target_os = "linux"))]
    let expected_metrics = {
        let touched_shards = [first_hash[..2].to_string(), second_hash[..2].to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>()
            .len() as u64;
        let mut expected_metrics = expected_metrics;
        expected_metrics.fallback_object_syncs = 2;
        expected_metrics.fallback_directory_syncs = touched_shards + 1;
        expected_metrics
    };
    assert_eq!(verified.metrics(), expected_metrics);
}

#[test]
fn durability_barriers_surround_canonical_publication() {
    let temp = tempfile::tempdir().unwrap();
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    let bytes = b"barrier-ordered bytes";
    let hash = crate::hash::sha256(bytes);
    let mut batch = cas
        .verified_object_batch([(hash.clone(), bytes.len() as u64)])
        .unwrap();
    batch.ingest(&hash, &mut Cursor::new(bytes)).unwrap();
    let mut durability = InspectingDurability::default();

    let set = batch.commit_with_durability(&mut durability).unwrap();

    assert_eq!(
        durability.events,
        vec![ObservedBarrier::StagedData, ObservedBarrier::CanonicalNames]
    );
    assert_eq!(set.metrics().staged_data_barriers, 1);
    assert_eq!(set.metrics().canonical_name_barriers, 1);
    assert_eq!(fs::read(set.object_path(&hash).unwrap()).unwrap(), bytes);
}

#[test]
fn staged_data_barrier_failure_publishes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    let bytes = b"never published";
    let hash = crate::hash::sha256(bytes);
    let mut batch = cas
        .verified_object_batch([(hash.clone(), bytes.len() as u64)])
        .unwrap();
    batch.ingest(&hash, &mut Cursor::new(bytes)).unwrap();
    let mut durability = InspectingDurability {
        fail_at: Some(ObservedBarrier::StagedData),
        ..Default::default()
    };

    assert!(batch.commit_with_durability(&mut durability).is_err());
    assert_eq!(durability.events, vec![ObservedBarrier::StagedData]);
    assert!(!cas.exists(&hash));
    assert!(temp_entries(cas.objects_dir()).is_empty());
}

#[test]
fn canonical_barrier_failure_returns_no_set_and_leaves_valid_object() {
    let temp = tempfile::tempdir().unwrap();
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    let bytes = b"valid unreachable object";
    let hash = crate::hash::sha256(bytes);
    let mut batch = cas
        .verified_object_batch([(hash.clone(), bytes.len() as u64)])
        .unwrap();
    batch.ingest(&hash, &mut Cursor::new(bytes)).unwrap();
    let mut durability = InspectingDurability {
        fail_at: Some(ObservedBarrier::CanonicalNames),
        ..Default::default()
    };

    assert!(batch.commit_with_durability(&mut durability).is_err());
    assert_eq!(
        durability.events,
        vec![ObservedBarrier::StagedData, ObservedBarrier::CanonicalNames]
    );
    assert_eq!(fs::read(cas.hash_to_path(&hash).unwrap()).unwrap(), bytes);
    assert!(temp_entries(cas.objects_dir()).is_empty());
}

#[test]
fn trusted_hit_can_commit_without_incoming_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    let bytes = b"already authenticated";
    let sha256 = cas.store(bytes).unwrap();
    let mut batch = cas
        .verified_object_batch([(sha256.clone(), bytes.len() as u64)])
        .unwrap();

    assert!(batch.reuse_trusted(&sha256).unwrap());
    let set = batch.commit().unwrap();
    assert!(set.contains(&sha256));
    assert_eq!(set.metrics().hits, 1);
    assert_eq!(set.metrics().incoming_bytes_hashed, 0);
    assert_eq!(set.metrics().persistent_bytes_written, 0);
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
            staged_data_barriers: 0,
            canonical_name_barriers: 0,
            fallback_object_syncs: 0,
            fallback_directory_syncs: 0,
            canonical_bytes_reread: 0,
        }
    );
}

#[test]
fn warm_authority_requires_a_regular_object_with_the_signed_size() {
    let temp = tempfile::tempdir().unwrap();
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    let bytes = b"trusted bytes";
    let hash = crate::hash::sha256(bytes);
    cas.store(bytes).unwrap();

    let size_error = cas
        .verified_object_batch([(hash.clone(), bytes.len() as u64 + 1)])
        .err()
        .unwrap();
    assert!(size_error.to_string().contains("has size"));

    let directory_hash = "0".repeat(64);
    fs::create_dir_all(cas.hash_to_path(&directory_hash).unwrap()).unwrap();
    let type_error = cas
        .verified_object_batch([(directory_hash, 0)])
        .err()
        .unwrap();
    assert!(type_error.to_string().contains("not a regular file"));
}

#[test]
fn warm_object_that_disappears_before_commit_exposes_no_verified_set() {
    let temp = tempfile::tempdir().unwrap();
    let cas = CasStore::new(temp.path().join("objects")).unwrap();
    let bytes = b"removed during verification";
    let hash = crate::hash::sha256(bytes);
    cas.store(bytes).unwrap();
    let mut batch = cas
        .verified_object_batch([(hash.clone(), bytes.len() as u64)])
        .unwrap();
    batch.ingest(&hash, &mut Cursor::new(bytes)).unwrap();
    fs::remove_file(cas.hash_to_path(&hash).unwrap()).unwrap();

    assert!(batch.commit().is_err());
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
    let xxh = CasStore::with_algorithm(temp.path().join("xxh"), crate::hash::HashAlgorithm::Xxh128)
        .unwrap();
    assert!(xxh.verified_object_batch([("abcd", 0)]).is_err());

    let cas = CasStore::new(temp.path().join("sha256")).unwrap();
    assert!(cas.verified_object_batch([("A".repeat(64), 0)]).is_err());
    assert!(cas.verified_object_batch([("0".repeat(63), 0)]).is_err());
}

fn assert_failed_stream_leaves_no_object(declared_size: u64, declared_hash: String, bytes: &[u8]) {
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
