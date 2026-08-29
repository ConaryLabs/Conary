// conary-core/src/filesystem/cas/ephemeral.rs

//! Exact object staging whose complete root is private to one operation.

use super::{CasStore, stream::verify_existing_object};
use crate::error::Result;
use crate::hash::{HashAlgorithm, Hasher};
use crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

/// Operation-private content-addressed staging with no durability authority.
pub(crate) struct EphemeralObjectStore {
    store: CasStore,
}

/// Exact work performed while staging one disposable object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EphemeralObjectStoreMetrics {
    pub(crate) incoming_bytes_hashed: u64,
    pub(crate) temporary_bytes_written: u64,
    pub(crate) canonical_bytes_reread: u64,
    pub(crate) hits: u64,
    pub(crate) misses: u64,
}

impl EphemeralObjectStore {
    pub(crate) fn new(root: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: CasStore::new(root)?,
        })
    }

    /// Stage one exact object below an operation-private, ephemeral CAS root.
    ///
    /// This retains the bounded read, exact size and SHA-256 checks, and atomic
    /// private-name publication of durable CAS ingestion. It deliberately does
    /// not synchronize the object or its shard: the caller must own the entire
    /// root as disposable same-process staging and must not return object
    /// authority from it. The CCS archive writer is the sole production caller;
    /// its signed output archive is a separate persistence boundary.
    pub(crate) fn store_reader_expected_with_metrics(
        &self,
        reader: &mut dyn Read,
        expected_size: u64,
        expected_sha256: &str,
    ) -> Result<(String, EphemeralObjectStoreMetrics)> {
        if self.store.algorithm() != HashAlgorithm::Sha256 {
            return Err(crate::Error::ConfigError(format!(
                "ephemeral expected-reader storage requires SHA-256 CAS authority, found {}",
                self.store.algorithm()
            )));
        }
        let path = self.store.hash_to_path(expected_sha256)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_extension = format!(
            "tmp.{}.{}.ephemeral",
            std::process::id(),
            CasStore::next_temp_id()
        );
        let temp_path = path.with_extension(temp_extension);
        let mut metrics = EphemeralObjectStoreMetrics::default();
        let result = (|| -> Result<()> {
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
                    crate::Error::IoError(
                        "payload size overflow while staging ephemeral object".into(),
                    )
                })?;
                if total > expected_size {
                    return Err(crate::Error::IoError(format!(
                        "payload exceeds declared size {expected_size} while staging {expected_sha256}"
                    )));
                }
                hasher.update(&buffer[..read]);
                output.write_all(&buffer[..read])?;
                metrics.incoming_bytes_hashed += read as u64;
                metrics.temporary_bytes_written += read as u64;
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
            drop(output);

            match fs::hard_link(&temp_path, &path) {
                Ok(()) => metrics.misses = 1,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    verify_existing_object(&path, expected_size, expected_sha256)?;
                    metrics.hits = 1;
                    metrics.canonical_bytes_reread = expected_size;
                }
                Err(error) => return Err(error.into()),
            }
            Ok(())
        })();
        let _ = fs::remove_file(&temp_path);
        result?;
        Ok((expected_sha256.to_string(), metrics))
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

    #[test]
    fn ephemeral_expected_reader_is_exact_and_reuses_verified_identity() {
        let temp = tempfile::tempdir().unwrap();
        let store = EphemeralObjectStore::new(temp.path().join("objects")).unwrap();
        let bytes = vec![0x5a; PAYLOAD_IO_BUFFER_SIZE * 3 + 17];
        let sha256 = crate::hash::sha256(&bytes);
        let mut reader = RecordingReader {
            inner: Cursor::new(&bytes),
            max_requested: 0,
        };

        let (stored, first) = store
            .store_reader_expected_with_metrics(&mut reader, bytes.len() as u64, &sha256)
            .unwrap();
        assert_eq!(stored, sha256);
        assert_eq!(
            first,
            EphemeralObjectStoreMetrics {
                incoming_bytes_hashed: bytes.len() as u64,
                temporary_bytes_written: bytes.len() as u64,
                canonical_bytes_reread: 0,
                hits: 0,
                misses: 1,
            }
        );
        assert!(reader.max_requested <= PAYLOAD_IO_BUFFER_SIZE);
        let (stored, second) = store
            .store_reader_expected_with_metrics(
                &mut Cursor::new(&bytes),
                bytes.len() as u64,
                &sha256,
            )
            .unwrap();
        assert_eq!(stored, sha256);
        assert_eq!(
            second,
            EphemeralObjectStoreMetrics {
                incoming_bytes_hashed: bytes.len() as u64,
                temporary_bytes_written: bytes.len() as u64,
                canonical_bytes_reread: bytes.len() as u64,
                hits: 1,
                misses: 0,
            }
        );
        assert_eq!(
            fs::read(store.object_path(&sha256).unwrap()).unwrap(),
            bytes
        );
        assert!(temporary_entries(store.root()).is_empty());
    }

    #[test]
    fn ephemeral_expected_reader_rejects_lies_and_cleans_private_names() {
        let temp = tempfile::tempdir().unwrap();
        let store = EphemeralObjectStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"payload";
        let sha256 = crate::hash::sha256(bytes);

        assert!(
            store
                .store_reader_expected_with_metrics(&mut Cursor::new(bytes), 6, &sha256)
                .is_err()
        );
        assert!(
            store
                .store_reader_expected_with_metrics(&mut Cursor::new(bytes), 8, &sha256)
                .is_err()
        );
        assert!(
            store
                .store_reader_expected_with_metrics(
                    &mut Cursor::new(bytes),
                    bytes.len() as u64,
                    &"0".repeat(64),
                )
                .is_err()
        );
        assert!(temporary_entries(store.root()).is_empty());
    }

    #[test]
    fn ephemeral_expected_reader_rejects_conflicting_existing_object() {
        let temp = tempfile::tempdir().unwrap();
        let store = EphemeralObjectStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"payload";
        let sha256 = crate::hash::sha256(bytes);
        let path = store.object_path(&sha256).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"wrong!!").unwrap();

        assert!(
            store
                .store_reader_expected_with_metrics(
                    &mut Cursor::new(bytes),
                    bytes.len() as u64,
                    &sha256,
                )
                .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), b"wrong!!");
        assert!(temporary_entries(store.root()).is_empty());
    }

    #[test]
    fn ephemeral_expected_reader_rejects_non_file_collision() {
        let temp = tempfile::tempdir().unwrap();
        let store = EphemeralObjectStore::new(temp.path().join("objects")).unwrap();
        let bytes = b"payload";
        let sha256 = crate::hash::sha256(bytes);
        let path = store.object_path(&sha256).unwrap();
        fs::create_dir_all(&path).unwrap();

        assert!(
            store
                .store_reader_expected_with_metrics(
                    &mut Cursor::new(bytes),
                    bytes.len() as u64,
                    &sha256,
                )
                .is_err()
        );
        assert!(path.is_dir());
        assert!(temporary_entries(store.root()).is_empty());
    }
}
