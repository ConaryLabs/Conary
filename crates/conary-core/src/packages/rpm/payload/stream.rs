// conary-core/src/packages/rpm/payload/stream.rs

//! Streaming RPM payload decompression and exact CPIO grammar.

use super::header::HeaderRecord;
use super::{mode_type, parse_error};
use crate::compression::{self, CompressionFormat};
use crate::error::Result;
use crate::packages::archive_utils::normalize_path;
use crate::packages::payload::{PAYLOAD_IO_BUFFER_SIZE, PayloadSpool, ReopenablePayload};
use rpm::{IndexTag, Package};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};

const CPIO_HEADER_SIZE: usize = 110;
const CPIO_NEWC_MAGIC: &[u8; 6] = b"070701";
const CPIO_CRC_MAGIC: &[u8; 6] = b"070702";
const CPIO_STRIPPED_MAGIC: &[u8; 6] = b"07070X";
const CPIO_TRAILER: &str = "TRAILER!!!";
const MAX_CPIO_NAME_SIZE: u64 = 4096;

#[derive(Debug)]
pub(super) struct PayloadMember {
    pub(super) header_index: usize,
    pub(super) content_size: u64,
    pub(super) sha256: Option<String>,
    pub(super) source: Option<ReopenablePayload>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFlavor {
    Newc,
    Stripped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RpmPayloadCompressor {
    None,
    Gzip,
    Bzip2,
    Xz,
    Lzma,
    Zstd,
}

pub(super) fn required_spool_bytes(records: &[HeaderRecord]) -> Result<u64> {
    records
        .iter()
        .filter(|record| !record.ghost && mode_type(record.mode) == libc::S_IFREG)
        .try_fold(0_u64, |total, record| {
            total
                .checked_add(record.size)
                .ok_or_else(|| parse_error("RPM payload spool-size arithmetic overflow"))
        })
}

pub(super) fn parse_members<'a>(
    package: &Package,
    payload: Box<dyn Read + 'a>,
    records: &'a [HeaderRecord],
    spool: &'a PayloadSpool,
) -> Result<Vec<PayloadMember>> {
    let compressor = payload_compressor(package)?;
    let reader = payload_decoder(payload, compressor)?;
    RpmPayloadReader::new(reader, records, spool).read_all()
}

fn payload_compressor(package: &Package) -> Result<RpmPayloadCompressor> {
    match package
        .metadata
        .header
        .get_entry_data_as_string(IndexTag::RPMTAG_PAYLOADCOMPRESSOR)
    {
        Ok("gzip") => Ok(RpmPayloadCompressor::Gzip),
        Ok("bzip2") => Ok(RpmPayloadCompressor::Bzip2),
        Ok("xz") => Ok(RpmPayloadCompressor::Xz),
        Ok("lzma") => Ok(RpmPayloadCompressor::Lzma),
        Ok("zstd") => Ok(RpmPayloadCompressor::Zstd),
        Ok(value) => Err(parse_error(format!(
            "RPM payload compressor {value:?} is not part of the supported RPM grammar"
        ))),
        Err(rpm::Error::TagNotFound(_)) => Ok(RpmPayloadCompressor::None),
        Err(error) => Err(parse_error(format!("read RPM payload compressor: {error}"))),
    }
}

fn payload_decoder<'a>(
    payload: Box<dyn Read + 'a>,
    compressor: RpmPayloadCompressor,
) -> Result<Box<dyn Read + 'a>> {
    match compressor {
        RpmPayloadCompressor::None => Ok(payload),
        RpmPayloadCompressor::Gzip => decoder(payload, CompressionFormat::Gzip, "gzip"),
        RpmPayloadCompressor::Xz => decoder(payload, CompressionFormat::Xz, "xz"),
        RpmPayloadCompressor::Zstd => decoder(payload, CompressionFormat::Zstd, "zstd"),
        RpmPayloadCompressor::Bzip2 => Ok(Box::new(bzip2::read::BzDecoder::new(payload))),
        RpmPayloadCompressor::Lzma => {
            // The liblzma limit bounds decoder working memory, not output size.
            // Payload output is bounded by exact header sizes and disk preflight.
            let stream =
                liblzma::stream::Stream::new_lzma_decoder(1024 * 1024 * 1024).map_err(|error| {
                    parse_error(format!("create RPM lzma payload decoder: {error}"))
                })?;
            Ok(Box::new(liblzma::read::XzDecoder::new_stream(
                payload, stream,
            )))
        }
    }
}

fn decoder<'a>(
    payload: Box<dyn Read + 'a>,
    format: CompressionFormat,
    label: &str,
) -> Result<Box<dyn Read + 'a>> {
    compression::create_decoder(payload, format)
        .map_err(|error| parse_error(format!("create RPM {label} payload decoder: {error}")))
}

struct RpmPayloadReader<'a> {
    reader: Box<dyn Read + 'a>,
    records: &'a [HeaderRecord],
    spool: &'a PayloadSpool,
    members: Vec<PayloadMember>,
    seen: HashSet<usize>,
    flavor: Option<ArchiveFlavor>,
    standard_path_indexes: HashMap<&'a str, usize>,
    total_content: u64,
}

impl<'a> RpmPayloadReader<'a> {
    fn new(
        reader: Box<dyn Read + 'a>,
        records: &'a [HeaderRecord],
        spool: &'a PayloadSpool,
    ) -> Self {
        Self {
            reader,
            records,
            spool,
            members: Vec::new(),
            seen: HashSet::new(),
            flavor: None,
            standard_path_indexes: records
                .iter()
                .enumerate()
                .filter(|(_, record)| !record.ghost)
                .map(|(index, record)| (record.path.as_str(), index))
                .collect(),
            total_content: 0,
        }
    }

    fn read_all(mut self) -> Result<Vec<PayloadMember>> {
        loop {
            let mut magic = [0_u8; 6];
            self.reader
                .read_exact(&mut magic)
                .map_err(|error| parse_error(format!("read RPM CPIO magic: {error}")))?;
            let finished = if &magic == CPIO_STRIPPED_MAGIC {
                self.require_flavor(ArchiveFlavor::Stripped)?;
                self.read_stripped_member()?
            } else if &magic == CPIO_NEWC_MAGIC || &magic == CPIO_CRC_MAGIC {
                self.require_flavor(ArchiveFlavor::Newc)?;
                self.read_newc_member(magic)?
            } else {
                return Err(parse_error(format!(
                    "RPM payload has invalid CPIO magic {:?}",
                    String::from_utf8_lossy(&magic)
                )));
            };
            if finished {
                break;
            }
        }
        let mut trailing = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
        loop {
            let read = self.reader.read(&mut trailing).map_err(|error| {
                parse_error(format!("read RPM payload trailer padding: {error}"))
            })?;
            if read == 0 {
                break;
            }
            if trailing[..read].iter().any(|byte| *byte != 0) {
                return Err(parse_error(
                    "RPM payload carries nonzero bytes after the CPIO trailer",
                ));
            }
        }
        Ok(self.members)
    }

    fn require_flavor(&mut self, flavor: ArchiveFlavor) -> Result<()> {
        if let Some(existing) = self.flavor
            && existing != flavor
        {
            return Err(parse_error(
                "RPM payload mixes standard and stripped CPIO entries",
            ));
        }
        self.flavor = Some(flavor);
        Ok(())
    }

    /// Read one stripped member. `ffffffff` is RPM's exact end marker.
    fn read_stripped_member(&mut self) -> Result<bool> {
        let raw_index = self.read_hex_u32("stripped CPIO file index")?;
        self.read_zero_padding(2, "stripped CPIO header")?;
        if raw_index == u32::MAX {
            return Ok(true);
        }
        let index = raw_index as usize;
        let record = self.records.get(index).ok_or_else(|| {
            parse_error(format!(
                "stripped RPM CPIO index {index} is outside {} header entries",
                self.records.len()
            ))
        })?;
        if record.ghost {
            return Err(parse_error(format!(
                "stripped RPM CPIO references ghost path {}",
                record.path
            )));
        }
        self.read_member_content(index, record.size, None)?;
        Ok(false)
    }

    /// Read a standard newc member. Returns `true` for the trailer.
    fn read_newc_member(&mut self, magic: [u8; 6]) -> Result<bool> {
        let _inode = self.read_hex_u32("inode")?;
        let _mode = self.read_hex_u32("mode")?;
        let _uid = self.read_hex_u32("uid")?;
        let _gid = self.read_hex_u32("gid")?;
        let _nlink = self.read_hex_u32("link count")?;
        let _mtime = self.read_hex_u32("mtime")?;
        let size = u64::from(self.read_hex_u32("content size")?);
        let _device_major = self.read_hex_u32("device major")?;
        let _device_minor = self.read_hex_u32("device minor")?;
        let _rdev_major = self.read_hex_u32("rdev major")?;
        let _rdev_minor = self.read_hex_u32("rdev minor")?;
        let name_size = u64::from(self.read_hex_u32("name size")?);
        let checksum = self.read_hex_u32("checksum")?;
        if name_size == 0 || name_size > MAX_CPIO_NAME_SIZE {
            return Err(parse_error(format!(
                "RPM CPIO name size {name_size} is outside 1..={MAX_CPIO_NAME_SIZE}"
            )));
        }
        let mut name = vec![0_u8; name_size as usize];
        self.reader
            .read_exact(&mut name)
            .map_err(|error| parse_error(format!("read RPM CPIO name: {error}")))?;
        if name.last() != Some(&0) || name[..name.len() - 1].contains(&0) {
            return Err(parse_error(
                "RPM CPIO name must contain exactly one trailing NUL",
            ));
        }
        name.pop();
        let name = String::from_utf8(name)
            .map_err(|error| parse_error(format!("RPM CPIO name is not UTF-8: {error}")))?;
        self.read_alignment_padding(CPIO_HEADER_SIZE as u64 + name_size, "CPIO name")?;
        if name == CPIO_TRAILER {
            if size != 0 || checksum != 0 {
                return Err(parse_error(
                    "RPM CPIO trailer carries content or a checksum",
                ));
            }
            return Ok(true);
        }
        let path = normalize_path(&name)
            .map_err(|error| parse_error(format!("invalid RPM CPIO path: {error}")))?;
        let index = *self
            .standard_path_indexes
            .get(path.as_str())
            .ok_or_else(|| {
                parse_error(format!(
                    "RPM CPIO path {path} does not identify a non-ghost header entry"
                ))
            })?;
        let expected_crc = (&magic == CPIO_CRC_MAGIC).then_some(checksum);
        if &magic == CPIO_NEWC_MAGIC && checksum != 0 {
            return Err(parse_error(format!(
                "RPM newc entry {path} has a nonzero checksum field"
            )));
        }
        self.read_member_content(index, size, expected_crc)?;
        Ok(false)
    }

    fn read_member_content(
        &mut self,
        index: usize,
        size: u64,
        expected_crc: Option<u32>,
    ) -> Result<()> {
        if !self.seen.insert(index) {
            return Err(parse_error(format!(
                "RPM payload repeats header path {}",
                self.records[index].path
            )));
        }
        self.total_content = self
            .total_content
            .checked_add(size)
            .ok_or_else(|| parse_error("RPM payload cumulative content size overflow"))?;
        let record = &self.records[index];
        let (source, sha256, actual_crc) = match mode_type(record.mode) {
            libc::S_IFREG => {
                if size != 0 && size != record.size {
                    return Err(parse_error(format!(
                        "RPM regular node {} declares {} bytes but CPIO yields {size}",
                        record.path, record.size
                    )));
                }
                let path = self.spool.indexed_path(index);
                let mut output = File::create(&path)?;
                let (sha256, crc) = copy_exact_payload(&mut self.reader, &mut output, size)?;
                output.sync_all()?;
                (Some(self.spool.source(path)), Some(sha256), crc)
            }
            libc::S_IFLNK => {
                let target = record.link_target.as_deref().ok_or_else(|| {
                    parse_error(format!(
                        "RPM symlink {} has no FILELINKTOS target",
                        record.path
                    ))
                })?;
                if size != target.len() as u64 || record.size != size {
                    return Err(parse_error(format!(
                        "RPM symlink {} size disagrees with FILELINKTOS",
                        record.path
                    )));
                }
                let crc = compare_exact_payload(&mut self.reader, target.as_bytes(), &record.path)?;
                (None, None, crc)
            }
            _ => {
                if size != 0 {
                    return Err(parse_error(format!(
                        "non-regular RPM node {} carries {size} payload bytes",
                        record.path
                    )));
                }
                (None, None, 0)
            }
        };
        if let Some(expected_crc) = expected_crc
            && actual_crc != expected_crc
        {
            return Err(parse_error(format!(
                "RPM CPIO CRC mismatch for {}: expected {expected_crc:#x}, got {actual_crc:#x}",
                record.path
            )));
        }
        self.read_alignment_padding(size, "CPIO content")?;
        self.members.push(PayloadMember {
            header_index: index,
            content_size: size,
            sha256,
            source,
        });
        Ok(())
    }

    fn read_hex_u32(&mut self, field: &str) -> Result<u32> {
        let mut bytes = [0_u8; 8];
        self.reader
            .read_exact(&mut bytes)
            .map_err(|error| parse_error(format!("read RPM CPIO {field}: {error}")))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| parse_error(format!("RPM CPIO {field} is not ASCII: {error}")))?;
        u32::from_str_radix(text, 16)
            .map_err(|error| parse_error(format!("RPM CPIO {field} is not hexadecimal: {error}")))
    }

    fn read_alignment_padding(&mut self, length: u64, field: &str) -> Result<()> {
        self.read_zero_padding(((4 - (length % 4)) % 4) as usize, field)
    }

    fn read_zero_padding(&mut self, count: usize, field: &str) -> Result<()> {
        let mut padding = [0_u8; 3];
        self.reader
            .read_exact(&mut padding[..count])
            .map_err(|error| parse_error(format!("read RPM {field} padding: {error}")))?;
        if padding[..count].iter().any(|byte| *byte != 0) {
            return Err(parse_error(format!(
                "RPM {field} padding contains nonzero bytes"
            )));
        }
        Ok(())
    }
}

fn copy_exact_payload(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    size: u64,
) -> Result<(String, u32)> {
    let mut remaining = size;
    let mut sha256 = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
    let mut crc = 0_u32;
    let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded by the fixed buffer");
        reader
            .read_exact(&mut buffer[..wanted])
            .map_err(|error| parse_error(format!("read RPM payload bytes: {error}")))?;
        writer.write_all(&buffer[..wanted])?;
        sha256.update(&buffer[..wanted]);
        crc = buffer[..wanted]
            .iter()
            .fold(crc, |sum, byte| sum.wrapping_add(u32::from(*byte)));
        remaining -= wanted as u64;
    }
    Ok((sha256.finalize().value, crc))
}

fn compare_exact_payload(reader: &mut dyn Read, expected: &[u8], path: &str) -> Result<u32> {
    let mut offset = 0_usize;
    let mut crc = 0_u32;
    let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
    while offset < expected.len() {
        let wanted = (expected.len() - offset).min(buffer.len());
        reader
            .read_exact(&mut buffer[..wanted])
            .map_err(|error| parse_error(format!("read RPM symlink {path}: {error}")))?;
        if buffer[..wanted] != expected[offset..offset + wanted] {
            return Err(parse_error(format!(
                "RPM symlink {path} payload target differs from FILELINKTOS"
            )));
        }
        crc = buffer[..wanted]
            .iter()
            .fold(crc, |sum, byte| sum.wrapping_add(u32::from(*byte)));
        offset += wanted;
    }
    Ok(crc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn header_record(mode: u32, size: u64, link_target: Option<&str>) -> HeaderRecord {
        HeaderRecord {
            path: "/usr/lib/.build-id/fixture".to_string(),
            mode,
            user: "root".to_string(),
            group: "root".to_string(),
            mtime: 0,
            size,
            ghost: false,
            digest: None,
            link_target: link_target.map(str::to_string),
            caps: None,
            ima_signature: None,
            device: 0,
            inode: 0,
            rdev: 0,
            nlink: None,
        }
    }

    struct ZeroReader {
        max_requested: usize,
    }

    impl Read for ZeroReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.max_requested = self.max_requested.max(buffer.len());
            buffer.fill(0);
            Ok(buffer.len())
        }
    }

    struct StopWriter;

    impl Write for StopWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("injected bounded sink stop"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn stripped_large_member_uses_u64_and_fixed_buffers() {
        let mut reader = ZeroReader { max_requested: 0 };
        let error =
            copy_exact_payload(&mut reader, &mut StopWriter, u64::from(u32::MAX) + 1).unwrap_err();
        assert!(error.to_string().contains("injected bounded sink stop"));
        assert!(reader.max_requested <= PAYLOAD_IO_BUFFER_SIZE);
    }

    #[test]
    fn lzma_alone_payload_decoder_round_trips_bytes() {
        let input = b"RPM lzma-alone payload";
        let options = liblzma::stream::LzmaOptions::new_preset(6).unwrap();
        let stream = liblzma::stream::Stream::new_lzma_encoder(&options).unwrap();
        let mut encoder = liblzma::read::XzEncoder::new_stream(input.as_slice(), stream);
        let mut encoded = Vec::new();
        encoder.read_to_end(&mut encoded).unwrap();

        let mut decoded = Vec::new();
        payload_decoder(
            Box::new(io::Cursor::new(encoded)),
            RpmPayloadCompressor::Lzma,
        )
        .unwrap()
        .read_to_end(&mut decoded)
        .unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn symlink_payload_comparison_rejects_bytes_that_differ_from_filelinktos() {
        let error = compare_exact_payload(
            &mut io::Cursor::new(b"../../expecteD-target"),
            b"../../expected-target",
            "/usr/lib/.build-id/fixture",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("payload target differs from FILELINKTOS")
        );
    }

    #[test]
    fn symlink_size_and_other_nonregular_payload_content_remain_rejected() {
        let target = b"../../expected-target";
        let symlink = [header_record(
            libc::S_IFLNK | 0o777,
            target.len() as u64,
            Some(std::str::from_utf8(target).unwrap()),
        )];
        let spool = PayloadSpool::new(0).unwrap();
        let mut reader = RpmPayloadReader::new(Box::new(io::Cursor::new(target)), &symlink, &spool);
        let error = reader
            .read_member_content(0, target.len() as u64 - 1, None)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("size disagrees with FILELINKTOS"),
            "{error}"
        );

        let directory = [header_record(libc::S_IFDIR | 0o755, 0, None)];
        let mut reader = RpmPayloadReader::new(Box::new(io::Cursor::new(b"x")), &directory, &spool);
        let error = reader.read_member_content(0, 1, None).unwrap_err();
        assert!(
            error.to_string().contains(
                "non-regular RPM node /usr/lib/.build-id/fixture carries 1 payload bytes"
            ),
            "{error}"
        );
    }
}
