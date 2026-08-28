// conary-core/src/repository/sync/immutable_catalog/spool.rs

//! Admitted, authenticated-input-derived spool for one native projection pass.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::repository::catalog::{
    CatalogMetadataStreamAdmission, CatalogPackageRecordV1, CatalogProvideRecordV1,
};
use crate::repository::parsers::{
    ArchPackageFragmentKind, SnapshotPackageIdentity, SnapshotPackageJoin,
};

const SPOOL_MAGIC: &[u8] = b"conary-native-projection-spool-v1\0";
const WRITE_BUFFER_BYTES: usize = 1024 * 1024;
pub(super) const PROJECTION_SPOOL_FILE_NAME: &str = "normalized-projection-v1.spool";
const PACKAGE_RECORD: u8 = 1;
const PROVIDE_MERGE_RECORD: u8 = 2;
const FINISH_JOIN_RECORD: u8 = 3;
const ARCH_FRAGMENT_RECORD: u8 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpoolProvideMergeV1 {
    pub(super) join: SnapshotPackageJoin,
    pub(super) identity: SnapshotPackageIdentity,
    pub(super) provides: Vec<CatalogProvideRecordV1>,
}

pub(super) enum ProjectionSpoolRecordV1 {
    Package(Box<CatalogPackageRecordV1>),
    ProvideMerge(SpoolProvideMergeV1),
    FinishJoin(SnapshotPackageJoin),
    ArchFragment {
        directory: String,
        kind: ArchPackageFragmentKind,
        content: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProjectionSpoolStatsV1 {
    pub(super) bytes: u64,
    pub(super) records: u64,
}

pub(super) struct ProjectionSpoolWriterV1 {
    path: PathBuf,
    file: File,
    admission: Box<dyn CatalogMetadataStreamAdmission>,
    buffer: Vec<u8>,
    digest: Sha256,
    bytes: u64,
    records: u64,
    handed_off: bool,
}

impl ProjectionSpoolWriterV1 {
    pub(super) fn create(
        path: &Path,
        admission: Box<dyn CatalogMetadataStreamAdmission>,
    ) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        let mut writer = Self {
            path: path.to_path_buf(),
            file,
            admission,
            buffer: Vec::with_capacity(WRITE_BUFFER_BYTES),
            digest: Sha256::new(),
            bytes: 0,
            records: 0,
            handed_off: false,
        };
        writer.append_bytes(SPOOL_MAGIC)?;
        Ok(writer)
    }

    pub(super) fn package_bytes(&mut self, canonical_package: &[u8]) -> Result<()> {
        self.frame(PACKAGE_RECORD, canonical_package)
    }

    pub(super) fn provide_merge(&mut self, merge: &SpoolProvideMergeV1) -> Result<()> {
        self.serialized_frame(PROVIDE_MERGE_RECORD, merge)
    }

    pub(super) fn finish_join(&mut self, join: SnapshotPackageJoin) -> Result<()> {
        self.serialized_frame(FINISH_JOIN_RECORD, &join)
    }

    pub(super) fn arch_fragment_bytes(&mut self, canonical_fragment: &[u8]) -> Result<()> {
        self.frame(ARCH_FRAGMENT_RECORD, canonical_fragment)
    }

    fn serialized_frame<T: Serialize>(&mut self, kind: u8, value: &T) -> Result<()> {
        let payload = crate::json::canonical_json(value).map_err(|error| {
            Error::ParseError(format!(
                "serialize normalized projection spool record: {error}"
            ))
        })?;
        self.frame(kind, &payload)
    }

    fn frame(&mut self, kind: u8, payload: &[u8]) -> Result<()> {
        let payload_bytes = u64::try_from(payload.len())
            .map_err(|_| Error::IoError("projection spool frame exceeds u64".to_string()))?;
        let frame_bytes = payload_bytes.checked_add(9).ok_or_else(|| {
            Error::IoError("projection spool frame byte count overflow".to_string())
        })?;
        let frame_bytes_usize = usize::try_from(frame_bytes).map_err(|_| {
            Error::IoError("projection spool frame exceeds addressable memory".to_string())
        })?;
        if frame_bytes_usize > WRITE_BUFFER_BYTES {
            self.flush_buffer()?;
            let mut frame = Vec::with_capacity(frame_bytes_usize);
            frame.push(kind);
            frame.extend_from_slice(&payload_bytes.to_le_bytes());
            frame.extend_from_slice(payload);
            self.append_bytes(&frame)?;
            self.flush_buffer()?;
        } else {
            if self.buffer.len() + frame_bytes_usize > WRITE_BUFFER_BYTES {
                self.flush_buffer()?;
            }
            self.append_bytes(&[kind])?;
            self.append_bytes(&payload_bytes.to_le_bytes())?;
            self.append_bytes(payload)?;
        }
        self.records = self
            .records
            .checked_add(1)
            .ok_or_else(|| Error::IoError("projection spool record count overflow".to_string()))?;
        Ok(())
    }

    fn append_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.digest.update(bytes);
        self.bytes = self
            .bytes
            .checked_add(u64::try_from(bytes.len()).map_err(|_| {
                Error::IoError("projection spool byte count exceeds u64".to_string())
            })?)
            .ok_or_else(|| Error::IoError("projection spool byte count overflow".to_string()))?;
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn flush_buffer(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let bytes = u64::try_from(self.buffer.len())
            .map_err(|_| Error::IoError("projection spool write exceeds u64".to_string()))?;
        let permit = self.admission.reserve_next(bytes)?;
        self.file.write_all(&self.buffer)?;
        self.buffer.clear();
        drop(permit);
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<ProjectionSpoolReaderV1> {
        self.flush_buffer()?;
        self.file.flush()?;
        let actual = self.file.metadata()?.len();
        if actual != self.bytes {
            return Err(Error::IoError(format!(
                "normalized projection spool expected {} bytes but staged {actual}",
                self.bytes
            )));
        }
        let expected_digest: [u8; 32] = self.digest.clone().finalize().into();
        let file = File::open(&self.path)?;
        let reader = ProjectionSpoolReaderV1 {
            path: self.path.clone(),
            file,
            expected_digest,
            expected_bytes: self.bytes,
            expected_records: self.records,
            digest: Sha256::new(),
            bytes: 0,
            records: 0,
            complete: false,
        };
        self.handed_off = true;
        Ok(reader)
    }
}

impl Drop for ProjectionSpoolWriterV1 {
    fn drop(&mut self) {
        if !self.handed_off {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) struct ProjectionSpoolReaderV1 {
    path: PathBuf,
    file: File,
    expected_digest: [u8; 32],
    expected_bytes: u64,
    expected_records: u64,
    digest: Sha256,
    bytes: u64,
    records: u64,
    complete: bool,
}

impl ProjectionSpoolReaderV1 {
    pub(super) fn open(mut self) -> Result<Self> {
        let mut magic = vec![0; SPOOL_MAGIC.len()];
        self.read_exact(&mut magic)?;
        if magic != SPOOL_MAGIC {
            return Err(Error::ParseError(
                "normalized projection spool has an invalid schema marker".to_string(),
            ));
        }
        Ok(self)
    }

    pub(super) fn next(&mut self) -> Result<Option<ProjectionSpoolRecordV1>> {
        let mut kind = [0_u8; 1];
        let read = self.file.read(&mut kind)?;
        if read == 0 {
            self.finish_validation()?;
            return Ok(None);
        }
        self.observe(&kind)?;
        let mut length = [0_u8; 8];
        self.read_exact(&mut length)?;
        let payload_bytes = u64::from_le_bytes(length);
        let remaining = self.expected_bytes.checked_sub(self.bytes).ok_or_else(|| {
            Error::ParseError("normalized projection spool byte count overflow".to_string())
        })?;
        if payload_bytes > remaining {
            return Err(Error::ParseError(format!(
                "normalized projection spool frame declares {payload_bytes} bytes with only {remaining} remaining"
            )));
        }
        let payload_len = usize::try_from(payload_bytes).map_err(|_| {
            Error::ParseError(
                "normalized projection spool frame exceeds addressable memory".to_string(),
            )
        })?;
        let mut payload = vec![0; payload_len];
        self.read_exact(&mut payload)?;
        self.records = self.records.checked_add(1).ok_or_else(|| {
            Error::ParseError("normalized projection spool record count overflow".to_string())
        })?;
        let record = match kind[0] {
            PACKAGE_RECORD => {
                ProjectionSpoolRecordV1::Package(Box::new(decode(&payload, "package")?))
            }
            PROVIDE_MERGE_RECORD => {
                ProjectionSpoolRecordV1::ProvideMerge(decode(&payload, "provide merge")?)
            }
            FINISH_JOIN_RECORD => {
                ProjectionSpoolRecordV1::FinishJoin(decode(&payload, "finished join")?)
            }
            ARCH_FRAGMENT_RECORD => {
                let (directory, kind, content): (String, String, String) =
                    decode(&payload, "Arch fragment")?;
                let kind = match kind.as_str() {
                    "desc" => ArchPackageFragmentKind::Desc,
                    "depends" => ArchPackageFragmentKind::Depends,
                    other => {
                        return Err(Error::ParseError(format!(
                            "normalized projection spool has unknown Arch fragment kind '{other}'"
                        )));
                    }
                };
                ProjectionSpoolRecordV1::ArchFragment {
                    directory,
                    kind,
                    content,
                }
            }
            other => {
                return Err(Error::ParseError(format!(
                    "normalized projection spool has unknown record kind {other}"
                )));
            }
        };
        Ok(Some(record))
    }

    pub(super) fn stats(&self) -> ProjectionSpoolStatsV1 {
        ProjectionSpoolStatsV1 {
            bytes: self.expected_bytes,
            records: self.expected_records,
        }
    }

    fn read_exact(&mut self, bytes: &mut [u8]) -> Result<()> {
        self.file.read_exact(bytes).map_err(|error| {
            Error::ParseError(format!("read normalized projection spool: {error}"))
        })?;
        self.observe(bytes)
    }

    fn observe(&mut self, bytes: &[u8]) -> Result<()> {
        self.digest.update(bytes);
        self.bytes =
            self.bytes
                .checked_add(u64::try_from(bytes.len()).map_err(|_| {
                    Error::ParseError("projection spool read exceeds u64".to_string())
                })?)
                .ok_or_else(|| {
                    Error::ParseError("projection spool read byte count overflow".to_string())
                })?;
        if self.bytes > self.expected_bytes {
            return Err(Error::ParseError(
                "normalized projection spool exceeds its staged byte count".to_string(),
            ));
        }
        Ok(())
    }

    fn finish_validation(&mut self) -> Result<()> {
        if self.complete {
            return Err(Error::ConflictError(
                "normalized projection spool was consumed more than once".to_string(),
            ));
        }
        let actual_digest: [u8; 32] = self.digest.clone().finalize().into();
        if self.bytes != self.expected_bytes
            || self.records != self.expected_records
            || actual_digest != self.expected_digest
        {
            return Err(Error::ConflictError(format!(
                "normalized projection spool changed during replay: expected {} bytes / {} records / {}, got {} bytes / {} records / {}",
                self.expected_bytes,
                self.expected_records,
                hex::encode(self.expected_digest),
                self.bytes,
                self.records,
                hex::encode(actual_digest)
            )));
        }
        self.complete = true;
        Ok(())
    }
}

impl Drop for ProjectionSpoolReaderV1 {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn decode<T: DeserializeOwned>(payload: &[u8], label: &str) -> Result<T> {
    serde_json::from_slice(payload).map_err(|error| {
        Error::ParseError(format!(
            "decode normalized projection spool {label} record: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::repository::catalog::CatalogMetadataStreamAdmission;

    struct RecordingAdmission {
        chunks: Arc<Mutex<Vec<u64>>>,
        refuse: bool,
    }

    impl CatalogMetadataStreamAdmission for RecordingAdmission {
        fn reserve_next(&self, additional_bytes: u64) -> Result<Box<dyn Send>> {
            if self.refuse {
                return Err(Error::IoError(
                    "projection spool capacity refused".to_string(),
                ));
            }
            self.chunks.lock().unwrap().push(additional_bytes);
            Ok(Box::new(()))
        }
    }

    fn admission(refuse: bool) -> (RecordingAdmission, Arc<Mutex<Vec<u64>>>) {
        let chunks = Arc::new(Mutex::new(Vec::new()));
        (
            RecordingAdmission {
                chunks: Arc::clone(&chunks),
                refuse,
            },
            chunks,
        )
    }

    #[test]
    fn admitted_spool_round_trips_and_verifies_complete_digest() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(PROJECTION_SPOOL_FILE_NAME);
        let (admission, chunks) = admission(false);
        let mut writer = ProjectionSpoolWriterV1::create(&path, Box::new(admission)).unwrap();
        let fragment = crate::json::canonical_json(&("alpha", "desc", "payload")).unwrap();
        writer.arch_fragment_bytes(&fragment).unwrap();
        let mut reader = writer.finish().unwrap().open().unwrap();

        match reader.next().unwrap().unwrap() {
            ProjectionSpoolRecordV1::ArchFragment {
                directory,
                kind,
                content,
            } => {
                assert_eq!(directory, "alpha");
                assert_eq!(kind, ArchPackageFragmentKind::Desc);
                assert_eq!(content, "payload");
            }
            _ => panic!("unexpected projection spool record"),
        }
        assert!(reader.next().unwrap().is_none());
        assert_eq!(chunks.lock().unwrap().iter().sum::<u64>(), reader.bytes);
        drop(reader);
        assert!(!path.exists());
    }

    #[test]
    fn changed_spool_bytes_fail_digest_verification() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(PROJECTION_SPOOL_FILE_NAME);
        let (admission, _) = admission(false);
        let mut writer = ProjectionSpoolWriterV1::create(&path, Box::new(admission)).unwrap();
        let fragment = crate::json::canonical_json(&("alpha", "desc", "payload")).unwrap();
        writer.arch_fragment_bytes(&fragment).unwrap();
        let reader = writer.finish().unwrap();
        let mut bytes = fs::read(&path).unwrap();
        let offset = bytes
            .windows(b"payload".len())
            .position(|window| window == b"payload")
            .unwrap();
        bytes[offset] = b'P';
        fs::write(&path, bytes).unwrap();
        let mut reader = reader.open().unwrap();

        assert!(reader.next().unwrap().is_some());
        assert!(matches!(reader.next(), Err(Error::ConflictError(_))));
    }

    #[test]
    fn refused_spool_chunk_leaves_no_staged_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(PROJECTION_SPOOL_FILE_NAME);
        let (admission, _) = admission(true);
        let mut writer = ProjectionSpoolWriterV1::create(&path, Box::new(admission)).unwrap();
        writer
            .finish_join(SnapshotPackageJoin::RpmFilelists)
            .unwrap();

        assert!(writer.finish().is_err());
        assert!(!path.exists());
    }
}
