// src/packages/cpio.rs

use std::io::{self, Read};

/// CPIO New ASCII Format (newc) header size
const HEADER_SIZE: usize = 110;
/// Magic string for newc format
const MAGIC_NEWC: &[u8] = b"070701";
/// Magic string for CRC format
const MAGIC_CRC: &[u8] = b"070702";

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
}

impl<R: Read> CpioReader<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
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

        // Parse hex fields
        let parse_hex = |start: usize, len: usize| -> io::Result<u32> {
            let s = std::str::from_utf8(&header_buf[start..start + len])
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            u32::from_str_radix(s, 16)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        };

        let mode = parse_hex(14, 8)?;
        let uid = parse_hex(22, 8)?;
        let gid = parse_hex(30, 8)?;
        let nlink = parse_hex(38, 8)?;
        let mtime = parse_hex(46, 8)? as u64;
        let filesize = parse_hex(54, 8)? as u64;
        let namesize = parse_hex(94, 8)? as u64;

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

        // Skip padding after filename (align to 4 bytes)
        let header_plus_name = HEADER_SIZE + namesize as usize;
        let pad = (4 - (header_plus_name % 4)) % 4;
        if pad > 0 {
            let mut skip = [0u8; 3];
            self.reader.read_exact(&mut skip[..pad])?;
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
