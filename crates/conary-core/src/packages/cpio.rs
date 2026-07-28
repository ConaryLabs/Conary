// conary-core/src/packages/cpio.rs

use crate::ccs::{ArchiveDecodeBounds, CCS_BUDGET};
use crate::error::{Error, Result};
use std::io::{self, Read};

/// CPIO New ASCII Format (newc) header size
const HEADER_SIZE: usize = 110;
/// Magic string for newc format
const MAGIC_NEWC: &[u8] = b"070701";
/// Magic string for CRC format
const MAGIC_CRC: &[u8] = b"070702";
/// Extracted CPIO entry metadata
#[derive(Debug, Clone)]
pub struct CpioEntry {
    pub name: String,
    pub size: u64,
    pub mode: u32,
    pub mtime: u32,
    pub uid: u32,
    pub gid: u32,
    pub ino: u32,
    pub nlink: u32,
    pub dev_major: u32,
    pub dev_minor: u32,
    pub rdev_major: u32,
    pub rdev_minor: u32,
}

/// A reader for CPIO (New ASCII) archives
pub struct CpioReader<R: Read> {
    reader: R,
    entries_seen: u64,
    total_content_size: u64,
    max_name_size: u64,
    bounds: ArchiveDecodeBounds,
}

impl<R: Read> CpioReader<R> {
    pub fn new(reader: R) -> Result<Self> {
        let bounds = CCS_BUDGET.archive_decode_bounds()?;
        Ok(Self::with_bounds(
            reader,
            bounds,
            CCS_BUDGET.max_path_bytes.saturating_add(1),
        ))
    }

    fn with_bounds(reader: R, bounds: ArchiveDecodeBounds, max_name_size: u64) -> Self {
        Self {
            reader,
            entries_seen: 0,
            total_content_size: 0,
            max_name_size,
            bounds,
        }
    }

    /// Read the next entry from the CPIO archive
    /// Returns Ok(None) if end of archive (TRAILER!!!)
    pub fn next_entry(&mut self) -> Result<Option<(CpioEntry, Vec<u8>)>> {
        // Read fixed header
        let mut header_buf = [0u8; HEADER_SIZE];
        if let Err(e) = self.reader.read_exact(&mut header_buf) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e.into());
        }

        // Verify magic
        let magic = &header_buf[0..6];
        if magic != MAGIC_NEWC && magic != MAGIC_CRC {
            return Err(cpio_parse(format!(
                "invalid magic {:?}",
                String::from_utf8_lossy(magic)
            )));
        }

        // Parse hex fields into u64 to avoid silent truncation on malformed headers
        let parse_hex = |start: usize, len: usize| -> Result<u64> {
            let s = std::str::from_utf8(&header_buf[start..start + len])
                .map_err(|error| cpio_parse(format!("header field is not ASCII: {error}")))?;
            u64::from_str_radix(s, 16)
                .map_err(|error| cpio_parse(format!("header field is not hexadecimal: {error}")))
        };

        let parse_hex_u32 = |start: usize, len: usize| -> Result<u32> {
            let val = parse_hex(start, len)?;
            u32::try_from(val)
                .map_err(|_| cpio_parse(format!("header field value {val:#x} overflows u32")))
        };

        let ino = parse_hex_u32(6, 8)?;
        let mode = parse_hex_u32(14, 8)?;
        let uid = parse_hex_u32(22, 8)?;
        let gid = parse_hex_u32(30, 8)?;
        let nlink = parse_hex_u32(38, 8)?;
        let mtime = parse_hex_u32(46, 8)?;
        let filesize = parse_hex(54, 8)?;
        let dev_major = parse_hex_u32(62, 8)?;
        let dev_minor = parse_hex_u32(70, 8)?;
        let rdev_major = parse_hex_u32(78, 8)?;
        let rdev_minor = parse_hex_u32(86, 8)?;
        let namesize = parse_hex(94, 8)?;
        let check = parse_hex_u32(102, 8)?;

        // Guard against unreasonable filename sizes
        if namesize == 0 || namesize > self.max_name_size {
            return Err(cpio_parse(format!(
                "entry name size {namesize} is outside 1..={}",
                self.max_name_size
            )));
        }

        // Read filename (including trailing NUL)
        let mut name_buf = vec![0u8; namesize as usize];
        self.reader.read_exact(&mut name_buf)?;

        if name_buf.last() != Some(&0) || name_buf[..name_buf.len() - 1].contains(&0) {
            return Err(cpio_parse(
                "entry name must contain exactly one trailing NUL",
            ));
        }
        let name = std::str::from_utf8(&name_buf[..name_buf.len() - 1])
            .map_err(|error| cpio_parse(format!("entry name is not UTF-8: {error}")))?
            .to_string();

        // Check for trailer
        if name == "TRAILER!!!" {
            if filesize != 0 {
                return Err(cpio_parse("TRAILER!!! entry must have zero size"));
            }
            return Ok(None);
        }

        self.entries_seen = self
            .entries_seen
            .checked_add(1)
            .ok_or_else(|| cpio_parse("entry count overflows u64"))?;
        self.bounds
            .admit_archive_entry("CPIO archive entries", self.entries_seen)?;

        // Skip padding after filename (align to 4 bytes)
        let header_plus_name = HEADER_SIZE
            .checked_add(namesize as usize)
            .ok_or_else(|| cpio_parse("header + name size arithmetic overflow"))?;
        let pad = (4 - (header_plus_name % 4)) % 4;
        if pad > 0 {
            let mut skip = [0u8; 3];
            self.reader.read_exact(&mut skip[..pad])?;
            if skip[..pad].iter().any(|byte| *byte != 0) {
                return Err(cpio_parse("filename padding must be zero"));
            }
        }

        self.total_content_size = self.bounds.add_payload_bytes(
            "CPIO declared payload bytes",
            self.total_content_size,
            filesize,
        )?;

        // Read file content
        let mut content = vec![0u8; filesize as usize];
        self.reader.read_exact(&mut content)?;
        if magic == MAGIC_CRC {
            let actual = content
                .iter()
                .fold(0_u32, |sum, byte| sum.wrapping_add(u32::from(*byte)));
            if actual != check {
                return Err(cpio_parse(format!(
                    "CRC mismatch: expected {check:#x}, got {actual:#x}"
                )));
            }
        } else if check != 0 {
            return Err(cpio_parse("newc checksum field must be zero"));
        }

        // Skip padding after content (align to 4 bytes)
        let pad = (4 - (filesize as usize % 4)) % 4;
        if pad > 0 {
            let mut skip = [0u8; 3];
            self.reader.read_exact(&mut skip[..pad])?;
            if skip[..pad].iter().any(|byte| *byte != 0) {
                return Err(cpio_parse("content padding must be zero"));
            }
        }

        Ok(Some((
            CpioEntry {
                name,
                size: filesize,
                mode,
                mtime,
                uid,
                gid,
                ino,
                nlink,
                dev_major,
                dev_minor,
                rdev_major,
                rdev_minor,
            },
            content,
        )))
    }
}

fn cpio_parse(message: impl Into<String>) -> Error {
    Error::ParseError(format!("invalid CPIO archive: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal CPIO newc header with the given namesize and filesize (as hex strings).
    fn make_header(namesize_hex: &str, filesize_hex: &str) -> Vec<u8> {
        // Field layout (all 8-char hex, total 110 bytes):
        // magic(6) ino(8) mode(8) uid(8) gid(8) nlink(8) mtime(8) filesize(8)
        // devmajor(8) devminor(8) rdevmajor(8) rdevminor(8) namesize(8) check(8)
        let mut h = Vec::new();
        h.extend_from_slice(b"070701"); // magic
        h.extend_from_slice(b"00000000"); // ino
        h.extend_from_slice(b"00000000"); // mode
        h.extend_from_slice(b"00000000"); // uid
        h.extend_from_slice(b"00000000"); // gid
        h.extend_from_slice(b"00000001"); // nlink
        h.extend_from_slice(b"00000000"); // mtime
        h.extend_from_slice(filesize_hex.as_bytes()); // filesize
        h.extend_from_slice(b"00000000"); // devmajor
        h.extend_from_slice(b"00000000"); // devminor
        h.extend_from_slice(b"00000000"); // rdevmajor
        h.extend_from_slice(b"00000000"); // rdevminor
        h.extend_from_slice(namesize_hex.as_bytes()); // namesize
        h.extend_from_slice(b"00000000"); // check
        assert_eq!(h.len(), HEADER_SIZE);
        h
    }

    #[test]
    fn reject_oversized_name() {
        // namesize = 0x2000 = 8192 > MAX_NAME_SIZE (4096)
        let data = make_header("00002000", "00000000");
        let mut reader = CpioReader::new(data.as_slice()).unwrap();
        let err = reader.next_entry().unwrap_err();
        assert!(matches!(&err, Error::ParseError(_)));
        assert!(
            err.to_string().contains("name size"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reject_oversized_file_content() {
        // Exercise a one-over file bound without allocating its declared body.
        let mut data = make_header("00000002", "FFFFFFFF");
        // Append filename "a\0" (2 bytes) + 2 bytes padding to align to 4
        data.extend_from_slice(b"a\0\0\0");
        let mut reader = CpioReader::with_bounds(
            data.as_slice(),
            ArchiveDecodeBounds {
                max_payload_object_bytes: u64::from(u32::MAX) - 1,
                ..CCS_BUDGET.archive_decode_bounds().unwrap()
            },
            CCS_BUDGET.max_path_bytes + 1,
        );
        let err = reader.next_entry().unwrap_err();
        let Error::Budget(error) = err else {
            panic!("expected typed payload-object refusal");
        };
        assert_eq!(
            error.dimension,
            crate::ccs::BudgetDimension::PayloadObjectBytes
        );
        assert_eq!(error.observed, u64::from(u32::MAX));
        assert_eq!(error.limit, u64::from(u32::MAX) - 1);
    }

    #[test]
    fn accept_valid_small_entry() {
        // namesize = 6 ("hello" + NUL), filesize = 3 ("abc")
        let mut data = make_header("00000006", "00000003");
        // filename "hello\0" = 6 bytes; header(110) + name(6) = 116 = 4*29 -> no padding
        data.extend_from_slice(b"hello\0");
        // file content "abc" = 3 bytes + 1 byte padding
        data.extend_from_slice(b"abc\0");
        let mut reader = CpioReader::new(data.as_slice()).unwrap();
        let entry = reader.next_entry().unwrap().unwrap();
        assert_eq!(entry.0.name, "hello");
        assert_eq!(entry.0.size, 3);
        assert_eq!(&entry.1, b"abc");
    }

    fn append_entry(data: &mut Vec<u8>, name: &str, content: &[u8]) {
        let namesize = name.len() + 1;
        let header = make_header(
            &format!("{namesize:08X}"),
            &format!("{:08X}", content.len()),
        );
        data.extend_from_slice(&header);
        data.extend_from_slice(name.as_bytes());
        data.push(0);
        let name_pad = (4 - ((HEADER_SIZE + namesize) % 4)) % 4;
        data.extend(std::iter::repeat_n(0, name_pad));
        data.extend_from_slice(content);
        let content_pad = (4 - (content.len() % 4)) % 4;
        data.extend(std::iter::repeat_n(0, content_pad));
    }

    #[test]
    fn reject_too_many_entries() {
        let mut data = Vec::new();
        append_entry(&mut data, "one", b"a");
        append_entry(&mut data, "two", b"b");
        append_entry(&mut data, "TRAILER!!!", b"");

        let mut reader = CpioReader::with_bounds(
            data.as_slice(),
            ArchiveDecodeBounds {
                max_archive_entries: 1,
                ..CCS_BUDGET.archive_decode_bounds().unwrap()
            },
            u64::MAX,
        );
        assert!(reader.next_entry().unwrap().is_some());
        let err = reader.next_entry().unwrap_err();
        let Error::Budget(error) = err else {
            panic!("expected typed archive-entry refusal");
        };
        assert_eq!(
            error.dimension,
            crate::ccs::BudgetDimension::ArchiveEntryCount
        );
        assert_eq!(error.observed, 2);
        assert_eq!(error.limit, 1);
    }

    #[test]
    fn reject_cumulative_content_over_limit() {
        let mut data = Vec::new();
        append_entry(&mut data, "one", b"abc");
        append_entry(&mut data, "two", b"def");
        append_entry(&mut data, "TRAILER!!!", b"");

        let mut reader = CpioReader::with_bounds(
            data.as_slice(),
            ArchiveDecodeBounds {
                max_payload_object_bytes: 3,
                max_total_payload_bytes: 5,
                ..CCS_BUDGET.archive_decode_bounds().unwrap()
            },
            u64::MAX,
        );
        assert!(reader.next_entry().unwrap().is_some());
        let err = reader.next_entry().unwrap_err();
        let Error::Budget(error) = err else {
            panic!("expected typed cumulative-payload refusal");
        };
        assert_eq!(
            error.dimension,
            crate::ccs::BudgetDimension::TotalPayloadBytes
        );
        assert_eq!(error.observed, 6);
        assert_eq!(error.limit, 5);
    }
}
