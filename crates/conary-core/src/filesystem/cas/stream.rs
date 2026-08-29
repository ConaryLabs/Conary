// conary-core/src/filesystem/cas/stream.rs

//! Exact bounded-memory CAS ingestion.

use super::{CasStore, sync_parent_dir};
use crate::error::Result;
use crate::hash::{HashAlgorithm, Hasher};
use crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE;
use std::fs;
use std::io::{Read, Write};

/// Exact work performed while staging one already-authoritative object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExpectedReaderStoreMetrics {
    pub incoming_bytes_hashed: u64,
    pub temporary_bytes_written: u64,
    pub canonical_bytes_reread: u64,
    pub hits: u64,
    pub misses: u64,
    pub object_file_syncs: u64,
    pub shard_syncs: u64,
}

impl CasStore {
    /// Store an exact private copy from a reader with bounded memory.
    pub fn store_private_reader(
        &self,
        reader: &mut dyn Read,
        expected_size: u64,
    ) -> Result<String> {
        let temp_path = self.objects_dir().join(format!(
            ".tmp.{}.{}.private-reader",
            std::process::id(),
            Self::next_temp_id()
        ));
        let result = (|| -> Result<String> {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)?;
            let mut hasher = Hasher::new(self.algorithm());
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
                output.write_all(&buffer[..read])?;
            }
            if total != expected_size {
                return Err(crate::Error::IoError(format!(
                    "CAS capture size mismatch: expected {expected_size}, got {total}"
                )));
            }
            output.sync_all()?;
            drop(output);

            let hash = hasher.finalize().value;
            let path = self.hash_to_path(&hash)?;
            if path.exists() && !super::cas_object_appears_shared(&path)? {
                return Ok(hash);
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&temp_path, &path)?;
            sync_parent_dir(&path)?;
            Ok(hash)
        })();
        let _ = fs::remove_file(&temp_path);
        result
    }

    /// Store one reader under an already authoritative SHA-256 identity.
    ///
    /// The source is consumed with a fixed buffer, and both exact size and
    /// digest are checked before the temporary file can become a CAS object.
    pub fn store_reader_expected(
        &self,
        reader: &mut dyn Read,
        expected_size: u64,
        expected_sha256: &str,
    ) -> Result<String> {
        self.store_reader_expected_with_metrics(reader, expected_size, expected_sha256)
            .map(|(hash, _metrics)| hash)
    }

    /// Store one authoritative object and return the exact temporary-write,
    /// hash, hit/miss, reread, and durability work performed.
    pub fn store_reader_expected_with_metrics(
        &self,
        reader: &mut dyn Read,
        expected_size: u64,
        expected_sha256: &str,
    ) -> Result<(String, ExpectedReaderStoreMetrics)> {
        if self.algorithm() != HashAlgorithm::Sha256 {
            return Err(crate::Error::ConfigError(format!(
                "expected-reader storage requires SHA-256 CAS authority, found {}",
                self.algorithm()
            )));
        }
        let path = self.hash_to_path(expected_sha256)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_extension = format!("tmp.{}.{}", std::process::id(), Self::next_temp_id());
        let temp_path = path.with_extension(temp_extension);
        let result = (|| -> Result<ExpectedReaderStoreMetrics> {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)?;
            let mut hasher = Hasher::new(HashAlgorithm::Sha256);
            let mut total = 0_u64;
            let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
            loop {
                let read = reader.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                total = total.checked_add(read as u64).ok_or_else(|| {
                    crate::Error::IoError("payload size overflow while storing CAS object".into())
                })?;
                if total > expected_size {
                    return Err(crate::Error::IoError(format!(
                        "payload exceeds declared size {expected_size} while storing {expected_sha256}"
                    )));
                }
                hasher.update(&buffer[..read]);
                output.write_all(&buffer[..read])?;
            }
            if total != expected_size {
                return Err(crate::Error::IoError(format!(
                    "payload size mismatch while storing {expected_sha256}: expected {expected_size}, got {total}"
                )));
            }
            let actual = hasher.finalize().value;
            if actual != expected_sha256 {
                return Err(crate::Error::ChecksumMismatch {
                    expected: expected_sha256.to_string(),
                    actual,
                });
            }
            output.sync_all()?;

            let mut metrics = ExpectedReaderStoreMetrics {
                incoming_bytes_hashed: total,
                temporary_bytes_written: total,
                object_file_syncs: 1,
                ..Default::default()
            };
            match fs::hard_link(&temp_path, &path) {
                Ok(()) => {
                    sync_parent_dir(&path)?;
                    metrics.misses = 1;
                    metrics.shard_syncs = 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    verify_existing_object(&path, expected_size, expected_sha256)?;
                    metrics.hits = 1;
                    metrics.canonical_bytes_reread = expected_size;
                }
                Err(error) => return Err(error.into()),
            }
            Ok(metrics)
        })();
        let _ = fs::remove_file(&temp_path);
        let metrics = result?;
        Ok((expected_sha256.to_string(), metrics))
    }
}

pub(super) fn verify_existing_object(
    path: &std::path::Path,
    size: u64,
    sha256: &str,
) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(crate::Error::IoError(format!(
            "existing CAS object {} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() != size {
        return Err(crate::Error::IoError(format!(
            "existing CAS object {} has size {}, expected {size}",
            path.display(),
            metadata.len()
        )));
    }
    let mut file = fs::File::open(path)?;
    let actual = crate::hash::sha256_reader_hex(&mut file)?;
    if actual != sha256 {
        return Err(crate::Error::ChecksumMismatch {
            expected: sha256.to_string(),
            actual,
        });
    }
    Ok(())
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

    #[test]
    fn expected_reader_storage_is_exact_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::new(temp.path().join("objects")).unwrap();
        let bytes = vec![0x5a; PAYLOAD_IO_BUFFER_SIZE * 3 + 17];
        let sha256 = crate::hash::sha256(&bytes);
        let mut reader = RecordingReader {
            inner: Cursor::new(&bytes),
            max_requested: 0,
        };

        let (stored, metrics) = store
            .store_reader_expected_with_metrics(&mut reader, bytes.len() as u64, &sha256)
            .unwrap();
        assert_eq!(stored, sha256);
        assert_eq!(metrics.incoming_bytes_hashed, bytes.len() as u64);
        assert_eq!(metrics.temporary_bytes_written, bytes.len() as u64);
        assert_eq!(metrics.misses, 1);
        assert_eq!(metrics.hits, 0);
        assert_eq!(metrics.object_file_syncs, 1);
        assert_eq!(metrics.shard_syncs, 1);
        assert_eq!(metrics.canonical_bytes_reread, 0);
        assert!(reader.max_requested <= PAYLOAD_IO_BUFFER_SIZE);
        assert_eq!(
            fs::read(store.hash_to_path(&sha256).unwrap()).unwrap(),
            bytes
        );

        let (_, hit) = store
            .store_reader_expected_with_metrics(
                &mut Cursor::new(&bytes),
                bytes.len() as u64,
                &sha256,
            )
            .unwrap();
        assert_eq!(hit.hits, 1);
        assert_eq!(hit.misses, 0);
        assert_eq!(hit.shard_syncs, 0);
        assert_eq!(hit.canonical_bytes_reread, bytes.len() as u64);
    }

    #[test]
    fn expected_reader_rejects_size_and_hash_lies() {
        let temp = tempfile::tempdir().unwrap();
        let store = CasStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"payload";
        let sha256 = crate::hash::sha256(bytes);

        assert!(
            store
                .store_reader_expected(&mut Cursor::new(bytes), 6, &sha256)
                .is_err()
        );
        assert!(
            store
                .store_reader_expected(&mut Cursor::new(bytes), 7, &"0".repeat(64))
                .is_err()
        );
    }
}
