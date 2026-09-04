// crates/conary-core/src/ccs/verify/archive_identity.rs

//! Exact compressed CCS archive identity and canonical stream completion.

use super::VerifiedArchiveIdentity;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use tar::Archive;

pub(super) type ArchiveDecoder =
    crate::ccs::archive_framing::ParallelMgzipDecoder<ArchiveIdentityReader<File>>;

pub(super) fn open(path: &Path, workers: usize) -> Result<Archive<ArchiveDecoder>> {
    let file = File::open(path).with_context(|| format!("open CCS package {}", path.display()))?;
    let reader = ArchiveIdentityReader::new(file);
    Ok(Archive::new(
        crate::ccs::archive_framing::ParallelMgzipDecoder::new(reader, workers)?,
    ))
}

/// Finish the canonical tar/MGZIP stream before exposing its exact identity.
pub(super) fn finish(
    archive: Archive<ArchiveDecoder>,
) -> Result<(
    VerifiedArchiveIdentity,
    crate::ccs::archive_framing::ArchiveDecodeMetrics,
)> {
    let decoder = crate::ccs::archive_framing::finish_canonical_tar(archive)
        .context("finish canonical CCS MGZIP blocks and verify CRC/footer")?;
    let (compressed, decode_metrics) = decoder.finish()?;
    let (sha256, bytes) = compressed.finish();
    Ok((VerifiedArchiveIdentity::new(sha256, bytes), decode_metrics))
}

pub(super) struct ArchiveIdentityReader<R> {
    inner: R,
    sha256: crate::hash::Hasher,
    bytes_read: u64,
}

impl<R> ArchiveIdentityReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            sha256: crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256),
            bytes_read: 0,
        }
    }

    fn finish(self) -> (String, u64) {
        (self.sha256.finalize().value, self.bytes_read)
    }
}

impl<R: Read> Read for ArchiveIdentityReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.sha256.update(&buffer[..read]);
        self.bytes_read = self.bytes_read.checked_add(read as u64).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "CCS archive byte count overflow",
            )
        })?;
        Ok(read)
    }
}
