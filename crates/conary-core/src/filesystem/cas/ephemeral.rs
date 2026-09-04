// crates/conary-core/src/filesystem/cas/ephemeral.rs

//! Exact object staging whose complete root is private to one operation.

use super::CasStore;
use crate::error::Result;
use crate::hash::{HashAlgorithm, Hasher};
use crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

/// Operation-private content-addressed staging with no durability authority.
pub(crate) struct EphemeralObjectStore {
    store: CasStore,
    authored_inventory: BTreeMap<String, u64>,
    authored_identities: BTreeMap<String, StagedObjectIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StagedObjectIdentity {
    size: u64,
    device: u64,
    inode: u64,
}

/// Exact physical work for one object admitted to operation-private staging.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EphemeralObjectStageMetrics {
    /// Bytes hashed here to establish object identity. Canonical chunks report
    /// this work from `Chunker` instead, so [`EphemeralObjectStore::stage_chunk`]
    /// leaves it at zero.
    pub(crate) object_identity_bytes_hashed: u64,
    pub(crate) unique_bytes_written: u64,
    pub(crate) deduplicated_bytes_avoided: u64,
    pub(crate) canonical_bytes_reread: u64,
    pub(crate) hits: u64,
    pub(crate) misses: u64,
}

impl EphemeralObjectStore {
    pub(crate) fn new(root: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: CasStore::new(root)?,
            authored_inventory: BTreeMap::new(),
            authored_identities: BTreeMap::new(),
        })
    }

    /// Stage one opaque chunk authenticated by the canonical chunker.
    ///
    /// The digest is not accepted from the caller: it is carried by [`Chunk`]
    /// together with the exact bytes over which the chunker computed it. The
    /// first occurrence is written through a private name and atomically linked
    /// into this operation's object namespace. Later occurrences reuse the
    /// in-memory identity established by that publication, so they perform no
    /// duplicate write, hash, or canonical-content reread.
    ///
    /// An on-disk object that is absent from this store's authored inventory is
    /// a collision, even if its bytes happen to hash correctly. Staging remains
    /// disposable and conveys no durable or publication authority.
    pub(crate) fn stage_chunk(
        &mut self,
        chunk: &crate::ccs::chunking::Chunk,
    ) -> Result<EphemeralObjectStageMetrics> {
        let sha256 = chunk.hash_hex();
        let size = u64::from(chunk.length());
        let data_size = u64::try_from(chunk.data().len()).map_err(|_| {
            crate::Error::IoError("authenticated chunk size exceeds u64".to_string())
        })?;
        if data_size != size {
            return Err(crate::Error::InternalError(format!(
                "authenticated chunk {sha256} length {size} disagrees with its {data_size} bytes"
            )));
        }
        let path = self.store.hash_to_path(&sha256)?;

        if let Some(expected_size) = self.authored_inventory.get(&sha256) {
            if *expected_size != size {
                return Err(crate::Error::InternalError(format!(
                    "authenticated chunk {sha256} has conflicting sizes {} and {size}",
                    expected_size
                )));
            }
            self.require_authored_identity(&sha256, &path, size)?;
            return Ok(EphemeralObjectStageMetrics {
                deduplicated_bytes_avoided: size,
                hits: 1,
                ..Default::default()
            });
        }

        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(untracked_collision(&path, &sha256)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_extension = format!(
            "tmp.{}.{}.authenticated-chunk",
            std::process::id(),
            CasStore::next_temp_id()
        );
        let temp_path = path.with_extension(temp_extension);
        let result = (|| -> Result<StagedObjectIdentity> {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)?;
            output.write_all(chunk.data())?;
            drop(output);
            fs::hard_link(&temp_path, &path)?;
            staged_object_identity(&path, size, &sha256)
        })();
        let cleanup = fs::remove_file(&temp_path);
        let identity = match result {
            Ok(identity) => {
                cleanup?;
                identity
            }
            Err(error) => {
                let _ = cleanup;
                return Err(error);
            }
        };
        self.record_authored(sha256, size, identity)?;
        Ok(EphemeralObjectStageMetrics {
            unique_bytes_written: size,
            misses: 1,
            ..Default::default()
        })
    }

    /// Hash and stage one whole object exactly once within this private owner.
    ///
    /// A known duplicate is still consumed and checked against the expected
    /// size and SHA-256 so caller-provided bytes never bypass authentication. It
    /// avoids both a duplicate write and a reread of the canonical staged file.
    pub(crate) fn stage_reader_expected_once(
        &mut self,
        reader: &mut dyn Read,
        expected_size: u64,
        expected_sha256: &str,
    ) -> Result<EphemeralObjectStageMetrics> {
        if self.store.algorithm() != HashAlgorithm::Sha256 {
            return Err(crate::Error::ConfigError(format!(
                "ephemeral expected-reader staging requires SHA-256 CAS authority, found {}",
                self.store.algorithm()
            )));
        }
        let path = self.store.hash_to_path(expected_sha256)?;
        if let Some(authored_size) = self.authored_inventory.get(expected_sha256) {
            if *authored_size != expected_size {
                return Err(crate::Error::InternalError(format!(
                    "authored object {expected_sha256} has conflicting sizes {authored_size} and {expected_size}"
                )));
            }
            self.require_authored_identity(expected_sha256, &path, expected_size)?;
            consume_expected_reader(reader, expected_size, expected_sha256, None)?;
            return Ok(EphemeralObjectStageMetrics {
                object_identity_bytes_hashed: expected_size,
                deduplicated_bytes_avoided: expected_size,
                hits: 1,
                ..Default::default()
            });
        }

        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(untracked_collision(&path, expected_sha256)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_extension = format!(
            "tmp.{}.{}.expected-once",
            std::process::id(),
            CasStore::next_temp_id()
        );
        let temp_path = path.with_extension(temp_extension);
        let result = (|| -> Result<StagedObjectIdentity> {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)?;
            consume_expected_reader(reader, expected_size, expected_sha256, Some(&mut output))?;
            drop(output);
            fs::hard_link(&temp_path, &path)?;
            staged_object_identity(&path, expected_size, expected_sha256)
        })();
        let cleanup = fs::remove_file(&temp_path);
        let identity = match result {
            Ok(identity) => {
                cleanup?;
                identity
            }
            Err(error) => {
                let _ = cleanup;
                return Err(error);
            }
        };
        self.record_authored(expected_sha256.to_string(), expected_size, identity)?;
        Ok(EphemeralObjectStageMetrics {
            object_identity_bytes_hashed: expected_size,
            unique_bytes_written: expected_size,
            misses: 1,
            ..Default::default()
        })
    }

    /// Exact digest/size census authored by this operation.
    pub(crate) fn inventory(&self) -> &BTreeMap<String, u64> {
        &self.authored_inventory
    }

    /// Open one object only when this operation authored its exact identity.
    pub(crate) fn open_object(&self, sha256: &str) -> Result<Box<dyn Read + Send>> {
        let expected_size = *self.authored_inventory.get(sha256).ok_or_else(|| {
            crate::Error::NotFound(format!(
                "object {sha256} is absent from operation-private authored inventory"
            ))
        })?;
        let path = self.store.hash_to_path(sha256)?;
        let expected = self.require_authored_identity(sha256, &path, expected_size)?;
        let file = fs::File::open(&path)?;
        let metadata = file.metadata()?;
        let opened = StagedObjectIdentity {
            size: metadata.len(),
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        if !metadata.file_type().is_file() || opened != expected {
            return Err(crate::Error::IoError(format!(
                "authored object {sha256} changed while opening operation-private staging"
            )));
        }
        Ok(Box::new(file))
    }

    fn record_authored(
        &mut self,
        sha256: String,
        size: u64,
        identity: StagedObjectIdentity,
    ) -> Result<()> {
        if self.authored_inventory.contains_key(&sha256)
            || self.authored_identities.contains_key(&sha256)
        {
            return Err(crate::Error::InternalError(format!(
                "operation-private object {sha256} was authored twice"
            )));
        }
        self.authored_inventory.insert(sha256.clone(), size);
        self.authored_identities.insert(sha256, identity);
        Ok(())
    }

    fn require_authored_identity(
        &self,
        sha256: &str,
        path: &Path,
        expected_size: u64,
    ) -> Result<StagedObjectIdentity> {
        let expected = *self.authored_identities.get(sha256).ok_or_else(|| {
            crate::Error::InternalError(format!(
                "authored object {sha256} has no staged file identity"
            ))
        })?;
        let observed = staged_object_identity(path, expected_size, sha256)?;
        if observed != expected {
            return Err(crate::Error::IoError(format!(
                "authored object {sha256} changed after operation-private staging"
            )));
        }
        Ok(expected)
    }

    #[cfg(test)]
    fn object_path(&self, sha256: &str) -> Result<PathBuf> {
        self.store.hash_to_path(sha256)
    }

    #[cfg(test)]
    fn root(&self) -> &Path {
        self.store.objects_dir()
    }
}

fn staged_object_identity(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<StagedObjectIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(crate::Error::IoError(format!(
            "operation-private object path {} for {expected_sha256} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() != expected_size {
        return Err(crate::Error::IoError(format!(
            "operation-private object {expected_sha256} has conflicting staged size {} instead of {expected_size}",
            metadata.len()
        )));
    }
    Ok(StagedObjectIdentity {
        size: metadata.len(),
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn untracked_collision(path: &Path, sha256: &str) -> crate::Error {
    crate::Error::ConflictError(format!(
        "operation-private object path {} for {sha256} exists outside authored inventory",
        path.display()
    ))
}

fn consume_expected_reader(
    reader: &mut dyn Read,
    expected_size: u64,
    expected_sha256: &str,
    mut output: Option<&mut dyn Write>,
) -> Result<u64> {
    let mut hasher = Hasher::new(HashAlgorithm::Sha256);
    let mut total = 0_u64;
    let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            crate::Error::IoError(
                "payload size overflow while staging operation-private object".to_string(),
            )
        })?;
        if total > expected_size {
            return Err(crate::Error::IoError(format!(
                "payload exceeds declared size {expected_size} while staging {expected_sha256}"
            )));
        }
        hasher.update(&buffer[..read]);
        if let Some(output) = output.as_deref_mut() {
            output.write_all(&buffer[..read])?;
        }
    }
    if total != expected_size {
        return Err(crate::Error::IoError(format!(
            "payload size mismatch while staging {expected_sha256}: expected {expected_size}, got {total}"
        )));
    }
    let actual = hasher.finalize().value;
    if actual != expected_sha256 {
        return Err(crate::Error::ChecksumMismatch {
            expected: expected_sha256.to_string(),
            actual,
        });
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

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

    fn temporary_entries(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                } else if super::super::is_temporary_object_name(&entry.file_name()) {
                    found.push(entry.path());
                }
            }
        }
        found
    }

    fn one_chunk(bytes: &[u8]) -> crate::ccs::chunking::Chunk {
        let mut chunks = crate::ccs::chunking::Chunker::new().chunk_bytes(bytes);
        assert_eq!(chunks.len(), 1);
        chunks.pop().unwrap()
    }

    #[test]
    fn authenticated_chunk_staging_writes_once_and_trusts_only_its_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = EphemeralObjectStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"authenticated chunk";
        let chunk = one_chunk(bytes);
        let sha256 = chunk.hash_hex();

        let first = store.stage_chunk(&chunk).unwrap();
        assert_eq!(
            first,
            EphemeralObjectStageMetrics {
                object_identity_bytes_hashed: 0,
                unique_bytes_written: bytes.len() as u64,
                deduplicated_bytes_avoided: 0,
                canonical_bytes_reread: 0,
                hits: 0,
                misses: 1,
            }
        );
        assert_eq!(store.inventory().get(&sha256), Some(&(bytes.len() as u64)));

        let second = store.stage_chunk(&chunk).unwrap();
        assert_eq!(
            second,
            EphemeralObjectStageMetrics {
                object_identity_bytes_hashed: 0,
                unique_bytes_written: 0,
                deduplicated_bytes_avoided: bytes.len() as u64,
                canonical_bytes_reread: 0,
                hits: 1,
                misses: 0,
            }
        );
        let mut reopened = Vec::new();
        store
            .open_object(&sha256)
            .unwrap()
            .read_to_end(&mut reopened)
            .unwrap();
        assert_eq!(reopened, bytes);
        assert!(temporary_entries(store.root()).is_empty());
    }

    #[test]
    fn expected_reader_staging_hashes_every_occurrence_but_writes_once() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = EphemeralObjectStore::new(temp.path().join("objects")).unwrap();
        let bytes = vec![0x5a; PAYLOAD_IO_BUFFER_SIZE * 2 + 19];
        let sha256 = crate::hash::sha256(&bytes);

        let first = store
            .stage_reader_expected_once(&mut Cursor::new(&bytes), bytes.len() as u64, &sha256)
            .unwrap();
        assert_eq!(
            first,
            EphemeralObjectStageMetrics {
                object_identity_bytes_hashed: bytes.len() as u64,
                unique_bytes_written: bytes.len() as u64,
                deduplicated_bytes_avoided: 0,
                canonical_bytes_reread: 0,
                hits: 0,
                misses: 1,
            }
        );

        let mut duplicate = RecordingReader {
            inner: Cursor::new(&bytes),
            max_requested: 0,
        };
        let second = store
            .stage_reader_expected_once(&mut duplicate, bytes.len() as u64, &sha256)
            .unwrap();
        assert_eq!(
            second,
            EphemeralObjectStageMetrics {
                object_identity_bytes_hashed: bytes.len() as u64,
                unique_bytes_written: 0,
                deduplicated_bytes_avoided: bytes.len() as u64,
                canonical_bytes_reread: 0,
                hits: 1,
                misses: 0,
            }
        );
        assert!(duplicate.max_requested <= PAYLOAD_IO_BUFFER_SIZE);

        let wrong = vec![0x33; bytes.len()];
        assert!(
            store
                .stage_reader_expected_once(&mut Cursor::new(wrong), bytes.len() as u64, &sha256,)
                .is_err()
        );
        assert_eq!(
            fs::read(store.object_path(&sha256).unwrap()).unwrap(),
            bytes
        );
        assert!(temporary_entries(store.root()).is_empty());
    }

    #[test]
    fn authored_staging_rejects_untracked_and_non_file_collisions() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = EphemeralObjectStore::new(temp.path().join("objects")).unwrap();
        let chunk = one_chunk(b"collision");
        let sha256 = chunk.hash_hex();
        let path = store.object_path(&sha256).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, chunk.data()).unwrap();

        let error = store.stage_chunk(&chunk).unwrap_err().to_string();
        assert!(error.contains("outside authored inventory"));
        assert!(store.inventory().is_empty());

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let error = store.stage_chunk(&chunk).unwrap_err().to_string();
        assert!(error.contains("outside authored inventory"));
        assert!(path.is_dir());
    }

    #[test]
    fn authored_staging_rejects_conflicting_size_and_replaced_object() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = EphemeralObjectStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"stable object";
        let chunk = one_chunk(bytes);
        let sha256 = chunk.hash_hex();
        store.stage_chunk(&chunk).unwrap();

        let error = store
            .stage_reader_expected_once(&mut Cursor::new(bytes), bytes.len() as u64 + 1, &sha256)
            .unwrap_err()
            .to_string();
        assert!(error.contains("conflicting sizes"));

        let path = store.object_path(&sha256).unwrap();
        let displaced = path.with_extension("displaced");
        fs::rename(&path, &displaced).unwrap();
        fs::write(&path, bytes).unwrap();
        let error = store.stage_chunk(&chunk).unwrap_err().to_string();
        assert!(error.contains("changed after operation-private staging"));
    }

    #[test]
    fn expected_reader_once_rejects_short_extra_and_digest_mismatch_without_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = EphemeralObjectStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"payload";
        let sha256 = crate::hash::sha256(bytes);

        for (input, size, digest) in [
            (bytes.as_slice(), 6, sha256.as_str()),
            (bytes.as_slice(), 8, sha256.as_str()),
            (bytes.as_slice(), bytes.len() as u64, "0000"),
        ] {
            assert!(
                store
                    .stage_reader_expected_once(&mut Cursor::new(input), size, digest)
                    .is_err()
            );
        }
        assert!(store.inventory().is_empty());
        assert!(temporary_entries(store.root()).is_empty());
    }
}
