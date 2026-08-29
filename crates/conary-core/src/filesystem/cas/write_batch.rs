// conary-core/src/filesystem/cas/write_batch.rs

//! Batched durability for exact private CAS captures.

use super::{CasStore, cas_object_appears_shared, durability::sync_filesystem};
use crate::error::Result;
use crate::hash::Hasher;
use crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

/// Byte-capture interface shared by immediate and transaction-scoped CAS use.
pub trait PrivateCasWriter {
    fn store_private_copy(&self, content: &[u8]) -> Result<String>;

    /// Copy exactly `expected_size` bytes from an already-open source.
    ///
    /// Implementations must consume the source with bounded memory and publish
    /// a private CAS inode rather than sharing the mutable source inode.
    fn store_private_reader(&self, reader: &mut dyn Read, expected_size: u64) -> Result<String>;
}

impl PrivateCasWriter for CasStore {
    fn store_private_copy(&self, content: &[u8]) -> Result<String> {
        CasStore::store_private_copy(self, content)
    }

    fn store_private_reader(&self, reader: &mut dyn Read, expected_size: u64) -> Result<String> {
        CasStore::store_private_reader(self, reader, expected_size)
    }
}

#[derive(Debug)]
struct PendingObject {
    path: PathBuf,
    temp_path: PathBuf,
}

/// A set of private CAS objects published with two durability barriers.
///
/// Objects are written beneath their final prefix directory but retain an
/// ignored `.tmp.` name. The first filesystem sync makes every staged byte
/// durable. Only then are objects renamed to their content-addressed names;
/// the second sync makes those names durable before a caller may commit its
/// database references.
pub struct PrivateCopyBatch<'a> {
    cas: &'a CasStore,
    pending: RefCell<BTreeMap<String, PendingObject>>,
}

impl<'a> PrivateCopyBatch<'a> {
    pub(super) fn new(cas: &'a CasStore) -> Self {
        Self {
            cas,
            pending: RefCell::new(BTreeMap::new()),
        }
    }

    /// Make every staged object durable and publish its canonical CAS name.
    pub fn commit(self) -> Result<()> {
        if self.pending.borrow().is_empty() {
            return Ok(());
        }

        sync_filesystem(self.cas.objects_dir())?;
        let publish_result = self.publish_objects();
        // Even on a partial publication failure, make any completed renames
        // durable. They remain valid, unreachable objects because the caller
        // must not commit database references after this error.
        let final_sync = sync_filesystem(self.cas.objects_dir());
        publish_result?;
        final_sync?;
        self.pending.borrow_mut().clear();
        Ok(())
    }

    fn publish_objects(&self) -> Result<()> {
        for pending in self.pending.borrow().values() {
            fs::rename(&pending.temp_path, &pending.path)?;
        }
        Ok(())
    }
}

impl PrivateCasWriter for PrivateCopyBatch<'_> {
    fn store_private_copy(&self, content: &[u8]) -> Result<String> {
        let hash = self.cas.compute_hash(content);
        if self.pending.borrow().contains_key(&hash) {
            return Ok(hash);
        }

        let path = self.cas.hash_to_path(&hash)?;
        if path.exists() && !cas_object_appears_shared(&path)? {
            return Ok(hash);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let temp_extension = format!(
            "tmp.{}.{}.private-batch",
            std::process::id(),
            CasStore::next_temp_id()
        );
        let temp_path = path.with_extension(temp_extension);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(content)?;
        drop(file);

        self.pending
            .borrow_mut()
            .insert(hash.clone(), PendingObject { path, temp_path });
        Ok(hash)
    }

    fn store_private_reader(&self, reader: &mut dyn Read, expected_size: u64) -> Result<String> {
        let root_temp_path = self.cas.objects_dir().join(format!(
            ".tmp.{}.{}.private-batch",
            std::process::id(),
            CasStore::next_temp_id()
        ));
        let result = (|| -> Result<(String, PendingObject)> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&root_temp_path)?;
            let mut hasher = Hasher::new(self.cas.algorithm());
            let mut total = 0_u64;
            let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
            loop {
                let read = reader.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                total = total.checked_add(read as u64).ok_or_else(|| {
                    crate::Error::IoError("CAS capture size overflow".to_string())
                })?;
                if total > expected_size {
                    return Err(crate::Error::IoError(format!(
                        "CAS capture exceeds expected size {expected_size}"
                    )));
                }
                hasher.update(&buffer[..read]);
                file.write_all(&buffer[..read])?;
            }
            if total != expected_size {
                return Err(crate::Error::IoError(format!(
                    "CAS capture size mismatch: expected {expected_size}, got {total}"
                )));
            }
            drop(file);

            let hash = hasher.finalize().value;
            let path = self.cas.hash_to_path(&hash)?;
            Ok((
                hash,
                PendingObject {
                    path,
                    temp_path: root_temp_path.clone(),
                },
            ))
        })();

        let (hash, pending) = match result {
            Ok(value) => value,
            Err(error) => {
                let _ = fs::remove_file(&root_temp_path);
                return Err(error);
            }
        };
        if self.pending.borrow().contains_key(&hash) {
            fs::remove_file(&pending.temp_path)?;
            return Ok(hash);
        }
        if pending.path.exists() && !cas_object_appears_shared(&pending.path)? {
            fs::remove_file(&pending.temp_path)?;
            return Ok(hash);
        }
        if let Some(parent) = pending.path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.pending.borrow_mut().insert(hash.clone(), pending);
        Ok(hash)
    }
}

impl Drop for PrivateCopyBatch<'_> {
    fn drop(&mut self) {
        for pending in self.pending.get_mut().values() {
            let _ = fs::remove_file(&pending.temp_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn batch_publishes_only_after_staged_bytes_are_durable() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"one exact object";
        let hash = cas.compute_hash(bytes);

        let batch = cas.private_copy_batch();
        assert_eq!(batch.store_private_copy(bytes).unwrap(), hash);
        assert_eq!(batch.store_private_copy(bytes).unwrap(), hash);
        assert!(!cas.exists(&hash));

        batch.commit().unwrap();
        assert_eq!(cas.retrieve(&hash).unwrap(), bytes);
        assert_eq!(cas.iter_objects().count(), 1);
    }

    #[test]
    fn dropped_batch_leaves_no_canonical_object_or_temp() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"uncommitted object";
        let hash = cas.compute_hash(bytes);

        {
            let batch = cas.private_copy_batch();
            batch.store_private_copy(bytes).unwrap();
        }

        assert!(!cas.exists(&hash));
        assert_eq!(cas.iter_objects().count(), 0);
    }

    #[test]
    fn batch_reader_capture_is_bounded_and_publishes_exact_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let bytes = vec![0x5a; PAYLOAD_IO_BUFFER_SIZE * 3 + 17];
        let hash = cas.compute_hash(&bytes);
        let batch = cas.private_copy_batch();

        assert_eq!(
            batch
                .store_private_reader(&mut Cursor::new(&bytes), bytes.len() as u64)
                .unwrap(),
            hash
        );
        assert!(!cas.exists(&hash));
        batch.commit().unwrap();
        assert_eq!(cas.retrieve(&hash).unwrap(), bytes);
    }
}
