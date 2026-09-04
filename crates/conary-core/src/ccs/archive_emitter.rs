// conary-core/src/ccs/archive_emitter.rs

//! Atomic emission of one exact CCS v3 control and object set.

use anyhow::{Context, Result, bail};
use flate2::Compression;
use gzp::ZWriter;
use gzp::deflate::Mgzip;
use gzp::par::compress::{ParCompress, ParCompressBuilder};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tar::{Builder, EntryType, Header};

/// Exact physical work and output identity from one archive emission.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ArchiveEmissionMetrics {
    pub(crate) members: u64,
    pub(crate) input_bytes: u64,
    pub(crate) compression_input_bytes: u64,
    pub(crate) compression_workers: u64,
    pub(crate) compression_block_bytes: u64,
    pub(crate) compression_blocks: u64,
    pub(crate) compression_buffer_ceiling_bytes: u64,
    pub(crate) output_sha256: String,
    pub(crate) output_bytes: u64,
}

/// Emit controls and authenticated objects through the sole CCS archive order.
///
/// The final path is published only after every exact-size source, tar, MGZIP,
/// and output-identity operation succeeds. Callers retain authority over the
/// supplied control bytes and object inventory; this function owns only their
/// deterministic physical archive representation.
pub(crate) fn write_exact_archive(
    output_path: &Path,
    mtime: u64,
    controls: &BTreeMap<&str, &[u8]>,
    objects: &BTreeMap<String, u64>,
    compression: crate::ccs::builder::CcsArchiveCompression,
    mut open_object: impl FnMut(&str) -> Result<Box<dyn Read + Send>>,
) -> Result<ArchiveEmissionMetrics> {
    let first_control = controls
        .first_key_value()
        .map(|(path, _)| *path)
        .context("CCS archive requires a MANIFEST control document")?;
    if first_control != "MANIFEST" || !controls.contains_key("MANIFEST.sig") {
        bail!("CCS archive controls must begin with MANIFEST and include MANIFEST.sig");
    }
    for (sha256, size) in objects {
        if sha256.len() != 64 || !crate::ccs::archive_layout::is_lower_hex(sha256) {
            bail!("CCS object digest {sha256:?} is not canonical lowercase SHA-256");
        }
        crate::ccs::budget::CCS_BUDGET
            .archive_decode_bounds()?
            .admit_payload_object("CCS archive object", *size)?;
    }

    let parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::Builder::new()
        .prefix(".conary-ccs-archive-")
        .tempfile_in(parent)?;
    let mut metrics = ArchiveEmissionMetrics::default();
    let identity;
    {
        let identity_writer = ArchiveIdentityWriter::new(staged.reopen()?);
        let encoder = ParallelMgzipWriter::new(identity_writer, compression)?;
        let mut archive = Builder::new(encoder);

        let mut directory = Header::new_gnu();
        directory.set_entry_type(EntryType::Directory);
        directory.set_mode(0o755);
        directory.set_size(0);
        directory.set_mtime(mtime);
        directory.set_cksum();
        archive.append_data(&mut directory, "objects", io::empty())?;
        metrics.members = checked_add(metrics.members, 1, "CCS archive member count")?;

        let object_prefixes = objects
            .keys()
            .map(|sha256| &sha256[..2])
            .collect::<BTreeSet<_>>();
        for prefix in object_prefixes {
            let mut directory = Header::new_gnu();
            directory.set_entry_type(EntryType::Directory);
            directory.set_mode(0o755);
            directory.set_size(0);
            directory.set_mtime(mtime);
            directory.set_cksum();
            archive.append_data(&mut directory, format!("objects/{prefix}"), io::empty())?;
            metrics.members = checked_add(metrics.members, 1, "CCS archive member count")?;
        }

        for (path, bytes) in controls {
            append_bytes(&mut archive, path, bytes, mtime)?;
            metrics.members = checked_add(metrics.members, 1, "CCS archive member count")?;
            metrics.input_bytes = checked_add(
                metrics.input_bytes,
                bytes.len() as u64,
                "CCS archive input bytes",
            )?;
        }

        for (sha256, size) in objects {
            let path = format!("objects/{}/{}", &sha256[..2], &sha256[2..]);
            let reader = open_object(sha256)
                .with_context(|| format!("open prepared CCS object {sha256}"))?;
            let mut reader = ExactSizeReader::new(reader, *size);
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::Regular);
            header.set_mode(0o644);
            header.set_size(*size);
            header.set_mtime(mtime);
            header.set_cksum();
            archive
                .append_data(&mut header, path, &mut reader)
                .with_context(|| format!("append prepared CCS object {sha256}"))?;
            reader
                .finish()
                .with_context(|| format!("validate prepared CCS object {sha256}"))?;
            metrics.members = checked_add(metrics.members, 1, "CCS archive member count")?;
            metrics.input_bytes =
                checked_add(metrics.input_bytes, *size, "CCS archive input bytes")?;
        }

        archive.finish()?;
        let (identity_writer, compression_metrics) = archive.into_inner()?.finish()?;
        metrics.compression_input_bytes = compression_metrics.input_bytes;
        metrics.compression_workers = compression_metrics.workers;
        metrics.compression_block_bytes = compression_metrics.block_bytes;
        metrics.compression_blocks = compression_metrics.blocks;
        metrics.compression_buffer_ceiling_bytes = compression_metrics.buffer_ceiling_bytes;
        identity = identity_writer.finish();
    }
    staged.as_file_mut().flush()?;
    staged
        .as_file_mut()
        .set_permissions(fs::Permissions::from_mode(0o644))?;
    staged.persist(output_path).map_err(|error| error.error)?;
    metrics.output_sha256 = identity.sha256;
    metrics.output_bytes = identity.bytes;
    Ok(metrics)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParallelCompressionMetrics {
    input_bytes: u64,
    workers: u64,
    block_bytes: u64,
    blocks: u64,
    buffer_ceiling_bytes: u64,
}

struct ParallelMgzipWriter<W: Write + Send + 'static> {
    inner: ParCompress<'static, Mgzip, W>,
    input_bytes: u64,
    workers: usize,
}

impl<W: Write + Send + 'static> ParallelMgzipWriter<W> {
    fn new(writer: W, compression: crate::ccs::builder::CcsArchiveCompression) -> Result<Self> {
        let workers = compression.workers();
        let inner = ParCompressBuilder::<Mgzip>::new()
            .buffer_size(crate::ccs::CCS_BUDGET.archive_compression_block_bytes)?
            .num_threads(workers)?
            .compression_level(Compression::default())
            .from_writer(writer);
        Ok(Self {
            inner,
            input_bytes: 0,
            workers,
        })
    }

    fn finish(mut self) -> Result<(W, ParallelCompressionMetrics)> {
        let writer = self.inner.finish()?;
        let block_bytes = u64::try_from(crate::ccs::CCS_BUDGET.archive_compression_block_bytes)
            .context("archive compression block bytes exceed u64")?;
        let blocks = self.input_bytes.div_ceil(block_bytes);
        Ok((
            writer,
            ParallelCompressionMetrics {
                input_bytes: self.input_bytes,
                workers: self.workers as u64,
                block_bytes,
                blocks,
                buffer_ceiling_bytes: crate::ccs::builder::CcsArchiveCompression::with_workers(
                    self.workers,
                )?
                .buffer_ceiling_bytes()?,
            },
        ))
    }
}

impl<W: Write + Send + 'static> Write for ParallelMgzipWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.input_bytes = self
            .input_bytes
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::other("CCS compression input byte count overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn append_bytes<W: Write>(
    archive: &mut Builder<W>,
    path: &str,
    bytes: &[u8],
    mtime: u64,
) -> Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o644);
    header.set_size(bytes.len() as u64);
    header.set_mtime(mtime);
    header.set_cksum();
    archive.append_data(&mut header, path, bytes)?;
    Ok(())
}

fn checked_add(left: u64, right: u64, label: &str) -> Result<u64> {
    left.checked_add(right)
        .with_context(|| format!("{label} overflow"))
}

struct ExactSizeReader {
    inner: Box<dyn Read + Send>,
    expected: u64,
    remaining: u64,
}

impl ExactSizeReader {
    fn new(inner: Box<dyn Read + Send>, expected: u64) -> Self {
        Self {
            inner,
            expected,
            remaining: expected,
        }
    }

    fn finish(mut self) -> io::Result<()> {
        if self.remaining != 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "CCS object ended after {} bytes, expected {}",
                    self.expected - self.remaining,
                    self.expected
                ),
            ));
        }
        let mut extra = [0_u8; 1];
        if self.inner.read(&mut extra)? != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("CCS object exceeds declared size {}", self.expected),
            ));
        }
        Ok(())
    }
}

impl Read for ExactSizeReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.remaining == 0 {
            return Ok(0);
        }
        let limit = usize::try_from(self.remaining.min(buffer.len() as u64))
            .expect("bounded read size fits usize");
        let read = self.inner.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "CCS object ended after {} bytes, expected {}",
                    self.expected - self.remaining,
                    self.expected
                ),
            ));
        }
        self.remaining -= read as u64;
        Ok(read)
    }
}

struct ArchiveOutputIdentity {
    sha256: String,
    bytes: u64,
}

struct ArchiveIdentityWriter<W> {
    inner: W,
    sha256: crate::hash::Hasher,
    bytes_written: u64,
}

impl<W> ArchiveIdentityWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            sha256: crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256),
            bytes_written: 0,
        }
    }

    fn finish(self) -> ArchiveOutputIdentity {
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

#[cfg(test)]
mod tests;
