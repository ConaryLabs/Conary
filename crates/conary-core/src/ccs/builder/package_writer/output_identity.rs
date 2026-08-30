// conary-core/src/ccs/builder/package_writer/output_identity.rs

//! Fused physical identity for compressed CCS archive output.

use std::io::{self, Write};

pub(super) struct ArchiveOutputIdentity {
    pub(super) sha256: String,
    pub(super) bytes: u64,
}

pub(super) struct ArchiveIdentityWriter<W> {
    inner: W,
    sha256: crate::hash::Hasher,
    bytes_written: u64,
}

impl<W> ArchiveIdentityWriter<W> {
    pub(super) fn new(inner: W) -> Self {
        Self {
            inner,
            sha256: crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256),
            bytes_written: 0,
        }
    }

    pub(super) fn finish(self) -> ArchiveOutputIdentity {
        ArchiveOutputIdentity {
            sha256: self.sha256.finalize().value,
            bytes: self.bytes_written,
        }
    }
}

impl<W: Write> Write for ArchiveIdentityWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.sha256.update(&buffer[..written]);
        self.bytes_written = self
            .bytes_written
            .checked_add(written as u64)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "CCS output byte count overflow")
            })?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
