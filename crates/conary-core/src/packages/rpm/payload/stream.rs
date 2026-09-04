// conary-core/src/packages/rpm/payload/stream.rs

//! Streaming RPM payload decompression and exact CPIO grammar.

use super::digest::{ComputedFileDigest, ComputedRegularContent, DeclaredDigestHasher};
use super::header::{HeaderRecord, RpmFileDigestAlgorithm};
use super::{mode_type, parse_error};
use crate::ccs::{ArchiveDecodeBounds, CCS_BUDGET};
use crate::compression::{self, CompressionFormat};
use crate::error::Result;
use crate::packages::archive_utils::normalize_path;
use crate::packages::payload::{PAYLOAD_IO_BUFFER_SIZE, PayloadSpool, ReopenablePayload};
use rpm::{IndexTag, Package};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{Read, Write};

const CPIO_HEADER_SIZE: usize = 110;
const CPIO_NEWC_MAGIC: &[u8; 6] = b"070701";
const CPIO_CRC_MAGIC: &[u8; 6] = b"070702";
const CPIO_STRIPPED_MAGIC: &[u8; 6] = b"07070X";
const CPIO_TRAILER: &str = "TRAILER!!!";

#[derive(Debug)]
pub(super) struct PayloadMember {
    pub(super) header_index: usize,
    pub(super) archive_position: usize,
    pub(super) content_size: u64,
    pub(super) regular: Option<RegularPayloadEvidence>,
}

/// One indivisible regular-member source and its same-pass digest evidence.
#[derive(Debug, Clone)]
pub(super) struct RegularPayloadEvidence {
    pub(super) source: ReopenablePayload,
    pub(super) computed: ComputedRegularContent,
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
    let bounds = CCS_BUDGET.archive_decode_bounds()?;
    records
        .iter()
        .filter(|record| !record.ghost && mode_type(record.mode) == libc::S_IFREG)
        .try_fold(0_u64, |total, record| {
            bounds
                .add_payload_bytes(
                    "RPM header declared regular payload bytes",
                    total,
                    record.size,
                )
                .map_err(Into::into)
        })
}

pub(super) fn parse_members<'a>(
    package: &Package,
    payload: Box<dyn Read + 'a>,
    records: &'a [HeaderRecord],
    spool: &'a PayloadSpool,
    decompressed_bytes: &crate::packages::parse_metrics::ReadCounter,
) -> Result<Vec<PayloadMember>> {
    let bounds = CCS_BUDGET.archive_decode_bounds()?;
    bounds.admit_archive_entry(
        "RPM payload header entries",
        records.iter().filter(|record| !record.ghost).count() as u64,
    )?;
    let compressor = payload_compressor(package)?;
    let reader = decompressed_bytes.wrap(payload_decoder(payload, compressor)?);
    RpmPayloadReader::new(Box::new(reader), records, spool, bounds).read_all()
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

/// Associate one standard CPIO name with RPM's absolute header path.
///
/// RPM removes one optional `./` prefix and one optional leading `/` before
/// comparing a CPIO name with its DIRNAMES/BASENAMES tables. The independent
/// prefix steps make `""`, `"./"`, `"/"`, and `".//"` equivalent archive
/// spellings of the header's exact root entry. See `rpmfnFindFN()` at the
/// pinned upstream revision:
/// <https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmfi.cc#L388-L435>.
fn rpm_cpio_header_path(name: &str) -> Result<String> {
    let without_dot = name.strip_prefix("./").unwrap_or(name);
    let relative = without_dot.strip_prefix('/').unwrap_or(without_dot);
    if relative.is_empty() {
        return Ok("/".to_string());
    }

    let raw_path = format!("/{relative}");
    let path = normalize_path(&raw_path)
        .map_err(|error| parse_error(format!("invalid RPM CPIO path: {error}")))?;
    if raw_path != path {
        return Err(parse_error(format!(
            "RPM CPIO path {name:?} does not identify canonical header path {path:?}"
        )));
    }
    Ok(path)
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
    bounds: ArchiveDecodeBounds,
}

impl<'a> RpmPayloadReader<'a> {
    fn new(
        reader: Box<dyn Read + 'a>,
        records: &'a [HeaderRecord],
        spool: &'a PayloadSpool,
        bounds: ArchiveDecodeBounds,
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
            bounds,
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
        let max_name_size = CCS_BUDGET.max_path_bytes.saturating_add(1);
        if name_size == 0 || name_size > max_name_size {
            return Err(parse_error(format!(
                "RPM CPIO name size {name_size} is outside 1..={max_name_size}"
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
        let path = rpm_cpio_header_path(&name)?;
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
        self.bounds
            .admit_archive_entry("RPM CPIO entries", self.members.len() as u64 + 1)?;
        if !self.seen.insert(index) {
            return Err(parse_error(format!(
                "RPM payload repeats header path {}",
                self.records[index].path
            )));
        }
        self.total_content = self.bounds.add_payload_bytes(
            "RPM CPIO declared payload bytes",
            self.total_content,
            size,
        )?;
        let record = &self.records[index];
        let (regular, actual_crc) = match mode_type(record.mode) {
            libc::S_IFREG => {
                if size != 0 && size != record.size {
                    return Err(parse_error(format!(
                        "RPM regular node {} declares {} bytes but CPIO yields {size}",
                        record.path, record.size
                    )));
                }
                let declared_algorithm = record
                    .digest
                    .as_ref()
                    .ok_or_else(|| {
                        parse_error(format!(
                            "RPM regular node {} has no declared file digest",
                            record.path
                        ))
                    })?
                    .algorithm;
                let path = self.spool.indexed_path(index);
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                let (computed, crc) = copy_exact_payload(
                    &mut self.reader,
                    &mut output,
                    size,
                    declared_algorithm,
                    expected_crc.is_some(),
                )?;
                drop(output);
                (
                    Some(RegularPayloadEvidence {
                        source: self.spool.source(path),
                        computed,
                    }),
                    crc,
                )
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
                let crc = compare_exact_payload(
                    &mut self.reader,
                    target.as_bytes(),
                    &record.path,
                    expected_crc.is_some(),
                )?;
                (None, crc)
            }
            _ => {
                if size != 0 {
                    return Err(parse_error(format!(
                        "non-regular RPM node {} carries {size} payload bytes",
                        record.path
                    )));
                }
                (None, expected_crc.map(|_| 0))
            }
        };
        if let Some(expected_crc) = expected_crc {
            let actual_crc = actual_crc.expect("CRC-flavored entry computes a CRC");
            if actual_crc != expected_crc {
                return Err(parse_error(format!(
                    "RPM CPIO CRC mismatch for {}: expected {expected_crc:#x}, got {actual_crc:#x}",
                    record.path
                )));
            }
        }
        self.read_alignment_padding(size, "CPIO content")?;
        let archive_position = self.members.len();
        self.members.push(PayloadMember {
            header_index: index,
            archive_position,
            content_size: size,
            regular,
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

pub(super) fn copy_exact_payload(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    size: u64,
    declared_algorithm: RpmFileDigestAlgorithm,
    compute_crc: bool,
) -> Result<(ComputedRegularContent, Option<u32>)> {
    let bytes_hashed = size
        .checked_mul(if declared_algorithm == RpmFileDigestAlgorithm::Sha2_256 {
            1
        } else {
            2
        })
        .ok_or_else(|| parse_error("RPM payload hash byte count overflow"))?;
    let mut remaining = size;
    let mut sha256 = crate::hash::Hasher::new(crate::hash::HashAlgorithm::Sha256);
    let mut declared = DeclaredDigestHasher::new(declared_algorithm);
    let mut crc = compute_crc.then_some(0_u32);
    let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded by the fixed buffer");
        reader
            .read_exact(&mut buffer[..wanted])
            .map_err(|error| parse_error(format!("read RPM payload bytes: {error}")))?;
        writer.write_all(&buffer[..wanted])?;
        sha256.update(&buffer[..wanted]);
        if let Some(declared) = declared.as_mut() {
            declared.update(&buffer[..wanted]);
        }
        if let Some(crc) = crc.as_mut() {
            *crc = buffer[..wanted]
                .iter()
                .fold(*crc, |sum, byte| sum.wrapping_add(u32::from(*byte)));
        }
        remaining -= wanted as u64;
    }
    let sha256 = sha256.finalize().value;
    let declared = ComputedFileDigest {
        algorithm: declared_algorithm,
        hex: declared
            .map(DeclaredDigestHasher::finalize)
            .unwrap_or_else(|| sha256.clone()),
    };
    Ok((
        ComputedRegularContent {
            sha256,
            declared,
            bytes_hashed,
        },
        crc,
    ))
}

fn compare_exact_payload(
    reader: &mut dyn Read,
    expected: &[u8],
    path: &str,
    compute_crc: bool,
) -> Result<Option<u32>> {
    let mut offset = 0_usize;
    let mut crc = compute_crc.then_some(0_u32);
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
        if let Some(crc) = crc.as_mut() {
            *crc = buffer[..wanted]
                .iter()
                .fold(*crc, |sum, byte| sum.wrapping_add(u32::from(*byte)));
        }
        offset += wanted;
    }
    Ok(crc)
}

#[cfg(test)]
mod tests;
