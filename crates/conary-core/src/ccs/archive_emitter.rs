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
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;

    fn controls() -> BTreeMap<&'static str, &'static [u8]> {
        BTreeMap::from([
            ("MANIFEST", b"manifest".as_slice()),
            ("MANIFEST.sig", b"signature".as_slice()),
        ])
    }

    fn staged_archive_paths(directory: &Path) -> Vec<PathBuf> {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".conary-ccs-archive-")
            })
            .collect()
    }

    #[test]
    fn exact_archive_is_atomic_and_binds_inline_output_identity() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("package.ccs");
        let bytes = b"authenticated object".to_vec();
        let sha256 = crate::hash::sha256(&bytes);
        let objects = BTreeMap::from([(sha256.clone(), bytes.len() as u64)]);

        let metrics = write_exact_archive(
            &output,
            7,
            &controls(),
            &objects,
            crate::ccs::builder::CcsArchiveCompression::default(),
            |_| Ok(Box::new(Cursor::new(bytes.clone()))),
        )
        .unwrap();
        let archive = fs::read(&output).unwrap();

        assert_eq!(metrics.members, 5);
        assert_eq!(metrics.input_bytes, (8 + 9 + bytes.len()) as u64);
        assert_eq!(metrics.compression_workers, 1);
        assert_eq!(
            metrics.compression_block_bytes,
            crate::ccs::CCS_BUDGET.archive_compression_block_bytes as u64
        );
        assert_eq!(
            metrics.compression_blocks,
            metrics
                .compression_input_bytes
                .div_ceil(metrics.compression_block_bytes)
        );
        assert_eq!(
            metrics.compression_buffer_ceiling_bytes,
            crate::ccs::builder::CcsArchiveCompression::default()
                .buffer_ceiling_bytes()
                .unwrap()
        );
        assert_eq!(metrics.output_bytes, archive.len() as u64);
        assert_eq!(metrics.output_sha256, crate::hash::sha256(&archive));
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn parallel_compression_is_worker_independent_and_canonical_mgzip() {
        let temp = tempfile::tempdir().unwrap();
        let one_worker = temp.path().join("one.ccs");
        let four_workers = temp.path().join("four.ccs");
        let bytes = (0..(crate::ccs::CCS_BUDGET.archive_compression_block_bytes * 3 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let sha256 = crate::hash::sha256(&bytes);
        let objects = BTreeMap::from([(sha256, bytes.len() as u64)]);

        let one_metrics = write_exact_archive(
            &one_worker,
            7,
            &controls(),
            &objects,
            crate::ccs::builder::CcsArchiveCompression::with_workers(1).unwrap(),
            |_| Ok(Box::new(Cursor::new(bytes.clone()))),
        )
        .unwrap();
        let four_metrics = write_exact_archive(
            &four_workers,
            7,
            &controls(),
            &objects,
            crate::ccs::builder::CcsArchiveCompression::with_workers(4).unwrap(),
            |_| Ok(Box::new(Cursor::new(bytes.clone()))),
        )
        .unwrap();

        let one = fs::read(one_worker).unwrap();
        let four = fs::read(four_workers).unwrap();
        assert_eq!(one, four);
        assert_eq!(one_metrics.output_sha256, four_metrics.output_sha256);
        assert_eq!(
            one_metrics.compression_input_bytes,
            four_metrics.compression_input_bytes
        );
        assert_eq!(
            one_metrics.compression_blocks,
            four_metrics.compression_blocks
        );
        assert_eq!(one_metrics.compression_workers, 1);
        assert_eq!(four_metrics.compression_workers, 4);
        assert!(
            four_metrics.compression_buffer_ceiling_bytes
                > one_metrics.compression_buffer_ceiling_bytes
        );

        let mut decoder = crate::ccs::archive_framing::MgzipDecoder::new(one.as_slice());
        io::copy(&mut decoder, &mut io::sink()).unwrap();
    }

    #[test]
    fn compression_budget_rejects_zero_and_unbounded_workers() {
        use crate::ccs::builder::CcsArchiveCompression;

        assert!(CcsArchiveCompression::with_workers(0).is_err());
        assert!(
            CcsArchiveCompression::with_workers(crate::ccs::CCS_BUDGET.max_archive_cpu_workers + 1)
                .is_err()
        );
    }

    #[test]
    fn exact_archive_reconstructs_the_complete_deterministic_layout() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("package.ccs");
        let mtime = 314_159;
        let controls = BTreeMap::from([
            ("MANIFEST", b"manifest authority".as_slice()),
            ("MANIFEST.sig", b"manifest signature".as_slice()),
            ("provenance.json", b"source provenance".as_slice()),
        ]);
        let object_bytes = [
            b"third physical object".to_vec(),
            b"first physical object".to_vec(),
            b"second physical object".to_vec(),
        ]
        .into_iter()
        .map(|bytes| (crate::hash::sha256(&bytes), bytes))
        .collect::<BTreeMap<_, _>>();
        let objects = object_bytes
            .iter()
            .map(|(sha256, bytes)| (sha256.clone(), bytes.len() as u64))
            .collect::<BTreeMap<_, _>>();
        let expected_object_order = objects.keys().cloned().collect::<Vec<_>>();
        let mut opened_objects = Vec::new();

        write_exact_archive(
            &output,
            mtime,
            &controls,
            &objects,
            crate::ccs::builder::CcsArchiveCompression::default(),
            |sha256| {
                opened_objects.push(sha256.to_owned());
                Ok(Box::new(Cursor::new(
                    object_bytes.get(sha256).unwrap().clone(),
                )))
            },
        )
        .unwrap();

        assert_eq!(opened_objects, expected_object_order);

        let decoder =
            crate::ccs::archive_framing::MgzipDecoder::new(fs::File::open(&output).unwrap());
        let mut archive = tar::Archive::new(decoder);
        let mut reconstructed = Vec::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            let entry_type = entry.header().entry_type();
            let mode = entry.header().mode().unwrap();
            let entry_mtime = entry.header().mtime().unwrap();
            let declared_size = entry.header().size().unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert_eq!(declared_size, bytes.len() as u64);
            reconstructed.push((path, entry_type, mode, entry_mtime, bytes));
        }

        let mut expected_paths = vec![PathBuf::from("objects")];
        expected_paths.extend(
            objects
                .keys()
                .map(|sha256| &sha256[..2])
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|prefix| PathBuf::from(format!("objects/{prefix}"))),
        );
        expected_paths.extend(controls.keys().map(PathBuf::from));
        expected_paths.extend(
            objects
                .keys()
                .map(|sha256| PathBuf::from(format!("objects/{}/{}", &sha256[..2], &sha256[2..]))),
        );
        assert_eq!(
            reconstructed
                .iter()
                .map(|(path, ..)| path.clone())
                .collect::<Vec<_>>(),
            expected_paths
        );
        let directory_count = 1 + objects
            .keys()
            .map(|sha256| &sha256[..2])
            .collect::<BTreeSet<_>>()
            .len();
        for (path, entry_type, mode, _, bytes) in &reconstructed[..directory_count] {
            assert!(entry_type.is_dir(), "{} is not a directory", path.display());
            assert_eq!(*mode, 0o755, "unexpected mode for {}", path.display());
            assert!(bytes.is_empty());
        }
        assert_eq!(reconstructed[directory_count].0, Path::new("MANIFEST"));
        for (path, entry_type, mode, entry_mtime, bytes) in &reconstructed[directory_count..] {
            assert!(
                entry_type.is_file(),
                "{} is not a regular file",
                path.display()
            );
            assert_eq!(*mode, 0o644, "unexpected mode for {}", path.display());
            assert_eq!(
                *entry_mtime,
                mtime,
                "unexpected mtime for {}",
                path.display()
            );
            if let Some(expected) = controls.get(path.to_str().unwrap()) {
                assert_eq!(bytes, expected);
                continue;
            }
            let components = path
                .iter()
                .map(|part| part.to_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(components.len(), 3);
            assert_eq!(components[0], "objects");
            let sha256 = format!("{}{}", components[1], components[2]);
            assert_eq!(crate::hash::sha256(bytes), sha256);
            assert_eq!(bytes, object_bytes.get(&sha256).unwrap());
        }
        assert!(
            reconstructed
                .iter()
                .all(|(_, _, _, entry_mtime, _)| *entry_mtime == mtime)
        );

        // Exhausting the decoder validates every MGZIP trailer as well as the
        // tar reader's member stream. Only tar end padding may remain.
        let mut decoder = archive.into_inner();
        let mut tar_tail = Vec::new();
        decoder.read_to_end(&mut tar_tail).unwrap();
        assert!(tar_tail.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn short_or_extra_object_never_replaces_the_final_path() {
        let expected = b"exact object".to_vec();
        let sha256 = crate::hash::sha256(&expected);
        let objects = BTreeMap::from([(sha256, expected.len() as u64)]);

        for bytes in [
            expected[..expected.len() - 1].to_vec(),
            [expected.as_slice(), b"!"].concat(),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let output = temp.path().join("package.ccs");
            fs::write(&output, b"previous complete archive").unwrap();

            assert!(
                write_exact_archive(
                    &output,
                    7,
                    &controls(),
                    &objects,
                    crate::ccs::builder::CcsArchiveCompression::default(),
                    |_| Ok(Box::new(Cursor::new(bytes.clone()))),
                )
                .is_err()
            );
            assert_eq!(fs::read(&output).unwrap(), b"previous complete archive");
            assert!(staged_archive_paths(temp.path()).is_empty());
        }
    }

    #[test]
    fn object_open_or_read_failure_removes_staging_and_preserves_final_path() {
        struct FailsAfterPrefix {
            prefix: Option<Vec<u8>>,
        }

        impl Read for FailsAfterPrefix {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if let Some(prefix) = self.prefix.take() {
                    let count = prefix.len().min(buffer.len());
                    buffer[..count].copy_from_slice(&prefix[..count]);
                    return Ok(count);
                }
                Err(io::Error::other("injected prepared-object read failure"))
            }
        }

        let expected = b"exact object".to_vec();
        let sha256 = crate::hash::sha256(&expected);
        let objects = BTreeMap::from([(sha256, expected.len() as u64)]);

        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("package.ccs");
        fs::write(&output, b"previous complete archive").unwrap();
        assert!(
            write_exact_archive(
                &output,
                7,
                &controls(),
                &objects,
                crate::ccs::builder::CcsArchiveCompression::default(),
                |_| bail!("injected prepared-object open failure"),
            )
            .is_err()
        );
        assert_eq!(fs::read(&output).unwrap(), b"previous complete archive");
        assert!(staged_archive_paths(temp.path()).is_empty());

        assert!(
            write_exact_archive(
                &output,
                7,
                &controls(),
                &objects,
                crate::ccs::builder::CcsArchiveCompression::default(),
                |_| {
                    Ok(Box::new(FailsAfterPrefix {
                        prefix: Some(expected[..3].to_vec()),
                    }))
                },
            )
            .is_err()
        );
        assert_eq!(fs::read(&output).unwrap(), b"previous complete archive");
        assert!(staged_archive_paths(temp.path()).is_empty());
    }
}
