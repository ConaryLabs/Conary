// conary-core/src/packages/cpio.rs

use crate::compression::{MAX_ARCHIVE_ENTRIES, MAX_DECOMPRESS_SIZE};
use std::io::{self, Read};

/// CPIO New ASCII Format (newc) header size
const HEADER_SIZE: usize = 110;
/// Magic string for newc format
const MAGIC_NEWC: &[u8] = b"070701";
/// Magic string for CRC format
const MAGIC_CRC: &[u8] = b"070702";
/// Maximum allowed filename length in bytes (4 KiB)
const MAX_NAME_SIZE: u64 = 4096;
/// Maximum allowed file content size in bytes (512 MB)
const MAX_FILE_SIZE: u64 = 512 * 1024 * 1024;

/// Extracted CPIO entry metadata
#[derive(Debug)]
pub struct CpioEntry {
    pub name: String,
    pub size: u64,
    pub mode: u32,
    pub mtime: u64,
    pub uid: u32,
    pub gid: u32,
    pub ino: u32,
    pub nlink: u32,
}

/// A reader for CPIO (New ASCII) archives
pub struct CpioReader<R: Read> {
    reader: R,
    entries_seen: usize,
    total_content_size: u64,
    max_entries: usize,
    max_total_content_size: u64,
}

impl<R: Read> CpioReader<R> {
    pub fn new(reader: R) -> Self {
        Self::with_limits(reader, MAX_ARCHIVE_ENTRIES, MAX_DECOMPRESS_SIZE)
    }

    fn with_limits(reader: R, max_entries: usize, max_total_content_size: u64) -> Self {
        Self {
            reader,
            entries_seen: 0,
            total_content_size: 0,
            max_entries,
            max_total_content_size,
        }
    }

    /// Read the next entry from the CPIO archive
    /// Returns Ok(None) if end of archive (TRAILER!!!)
    pub fn next_entry(&mut self) -> io::Result<Option<(CpioEntry, Vec<u8>)>> {
        // Read fixed header
        let mut header_buf = [0u8; HEADER_SIZE];
        if let Err(e) = self.reader.read_exact(&mut header_buf) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e);
        }

        // Verify magic
        let magic = &header_buf[0..6];
        if magic != MAGIC_NEWC && magic != MAGIC_CRC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid CPIO magic: {:?}", String::from_utf8_lossy(magic)),
            ));
        }

        // Parse hex fields into u64 to avoid silent truncation on malformed headers
        let parse_hex = |start: usize, len: usize| -> io::Result<u64> {
            let s = std::str::from_utf8(&header_buf[start..start + len])
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            u64::from_str_radix(s, 16).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        };

        let parse_hex_u32 = |start: usize, len: usize| -> io::Result<u32> {
            let val = parse_hex(start, len)?;
            u32::try_from(val).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("CPIO header field value {val:#x} overflows u32"),
                )
            })
        };

        let mode = parse_hex_u32(14, 8)?;
        let uid = parse_hex_u32(22, 8)?;
        let gid = parse_hex_u32(30, 8)?;
        let nlink = parse_hex_u32(38, 8)?;
        let mtime = parse_hex(46, 8)?;
        let filesize = parse_hex(54, 8)?;
        let namesize = parse_hex(94, 8)?;

        // Guard against unreasonable filename sizes
        if namesize > MAX_NAME_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("CPIO entry name size {namesize} exceeds maximum {MAX_NAME_SIZE}"),
            ));
        }

        // Read filename (including trailing NUL)
        let mut name_buf = vec![0u8; namesize as usize];
        self.reader.read_exact(&mut name_buf)?;

        // Remove trailing NUL
        let name = if let Some(last) = name_buf.last() {
            if *last == 0 {
                String::from_utf8_lossy(&name_buf[..name_buf.len() - 1]).to_string()
            } else {
                String::from_utf8_lossy(&name_buf).to_string()
            }
        } else {
            String::new()
        };

        // Check for trailer
        if name == "TRAILER!!!" {
            return Ok(None);
        }

        self.entries_seen += 1;
        if self.entries_seen > self.max_entries {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("CPIO entry count exceeds maximum {}", self.max_entries),
            ));
        }

        // Skip padding after filename (align to 4 bytes)
        let header_plus_name = HEADER_SIZE.checked_add(namesize as usize).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "CPIO header + name size arithmetic overflow",
            )
        })?;
        let pad = (4 - (header_plus_name % 4)) % 4;
        if pad > 0 {
            let mut skip = [0u8; 3];
            self.reader.read_exact(&mut skip[..pad])?;
        }

        // Guard against unreasonable file content sizes
        if filesize > MAX_FILE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("CPIO entry file size {filesize} exceeds maximum {MAX_FILE_SIZE}"),
            ));
        }

        self.total_content_size =
            self.total_content_size
                .checked_add(filesize)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "CPIO cumulative content size overflow",
                    )
                })?;
        if self.total_content_size > self.max_total_content_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "CPIO cumulative content size {} exceeds maximum {}",
                    self.total_content_size, self.max_total_content_size
                ),
            ));
        }

        // Read file content
        let mut content = vec![0u8; filesize as usize];
        self.reader.read_exact(&mut content)?;

        // Skip padding after content (align to 4 bytes)
        let pad = (4 - (filesize as usize % 4)) % 4;
        if pad > 0 {
            let mut skip = [0u8; 3];
            self.reader.read_exact(&mut skip[..pad])?;
        }

        Ok(Some((
            CpioEntry {
                name,
                size: filesize,
                mode,
                mtime,
                uid,
                gid,
                ino: 0, // Ignored
                nlink,
            },
            content,
        )))
    }
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
        let mut reader = CpioReader::new(data.as_slice());
        let err = reader.next_entry().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("name size"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reject_oversized_file_content() {
        // namesize = 2 (one char + NUL), filesize = 0xFFFFFFFF (~4 GiB, > MAX_FILE_SIZE)
        let mut data = make_header("00000002", "FFFFFFFF");
        // Append filename "a\0" (2 bytes) + 2 bytes padding to align to 4
        data.extend_from_slice(b"a\0\0\0");
        let mut reader = CpioReader::new(data.as_slice());
        let err = reader.next_entry().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("file size"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn accept_valid_small_entry() {
        // namesize = 6 ("hello" + NUL), filesize = 3 ("abc")
        let mut data = make_header("00000006", "00000003");
        // filename "hello\0" = 6 bytes; header(110) + name(6) = 116 = 4*29 -> no padding
        data.extend_from_slice(b"hello\0");
        // file content "abc" = 3 bytes + 1 byte padding
        data.extend_from_slice(b"abc\0");
        let mut reader = CpioReader::new(data.as_slice());
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

        let mut reader = CpioReader::with_limits(data.as_slice(), 1, u64::MAX);
        assert!(reader.next_entry().unwrap().is_some());
        let err = reader.next_entry().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("entry count"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reject_cumulative_content_over_limit() {
        let mut data = Vec::new();
        append_entry(&mut data, "one", b"abc");
        append_entry(&mut data, "two", b"def");
        append_entry(&mut data, "TRAILER!!!", b"");

        let mut reader = CpioReader::with_limits(data.as_slice(), usize::MAX, 5);
        assert!(reader.next_entry().unwrap().is_some());
        let err = reader.next_entry().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("cumulative"),
            "unexpected error: {err}"
        );
    }
}
