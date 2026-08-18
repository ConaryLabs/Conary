// conary-core/src/packages/arch/payload/sparse.rs

//! GNU PAX sparse v1 authority and payload decoding.

use super::exact_utf8;
use crate::ccs::ArchiveDecodeBounds;
use crate::error::{Error, Result};
use crate::hash::{HashAlgorithm, Hasher};
use crate::packages::payload::PAYLOAD_IO_BUFFER_SIZE;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GnuSparseV1 {
    pub(super) name: String,
    pub(super) logical_size: u64,
}

#[derive(Default)]
pub(super) struct GnuSparsePaxRecords {
    major: Option<u64>,
    minor: Option<u64>,
    name: Option<String>,
    logical_size: Option<u64>,
    saw_record: bool,
}

impl GnuSparsePaxRecords {
    pub(super) fn record(&mut self, key: &str, value: &[u8], path: &str) -> Result<bool> {
        match key {
            "GNU.sparse.major" => {
                self.saw_record = true;
                Self::insert_decimal(&mut self.major, value, key, path)?;
            }
            "GNU.sparse.minor" => {
                self.saw_record = true;
                Self::insert_decimal(&mut self.minor, value, key, path)?;
            }
            "GNU.sparse.name" => {
                self.saw_record = true;
                self.insert_name(value, key, path)?;
            }
            "GNU.sparse.realsize" => {
                self.saw_record = true;
                Self::insert_decimal(&mut self.logical_size, value, key, path)?;
            }
            _ if key.starts_with("GNU.sparse.") => {
                return Err(Error::ParseError(format!(
                    "unsupported GNU sparse PAX record {key} for ALPM payload node {path}"
                )));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn insert_decimal(slot: &mut Option<u64>, value: &[u8], key: &str, path: &str) -> Result<()> {
        if slot.is_some() {
            return Err(Error::ParseError(format!(
                "duplicate {key} record for ALPM payload node {path}"
            )));
        }
        *slot = Some(parse_decimal(value, key, path)?);
        Ok(())
    }

    fn insert_name(&mut self, value: &[u8], key: &str, path: &str) -> Result<()> {
        if self.name.is_some() {
            return Err(Error::ParseError(format!(
                "duplicate {key} record for ALPM payload node {path}"
            )));
        }
        self.name = Some(exact_utf8(value, key, path)?);
        Ok(())
    }

    pub(super) fn finish(self, path: &str) -> Result<Option<GnuSparseV1>> {
        if !self.saw_record {
            return Ok(None);
        }
        let (Some(major), Some(minor), Some(name), Some(logical_size)) =
            (self.major, self.minor, self.name, self.logical_size)
        else {
            return Err(Error::ParseError(format!(
                "incomplete GNU sparse PAX v1 authority for ALPM payload node {path}"
            )));
        };
        if (major, minor) != (1, 0) {
            return Err(Error::ParseError(format!(
                "unsupported GNU sparse PAX version {major}.{minor} for ALPM payload node {path}"
            )));
        }
        Ok(Some(GnuSparseV1 { name, logical_size }))
    }
}

pub(super) fn copy_payload(
    reader: &mut dyn Read,
    writer: &mut File,
    declared_encoded_size: u64,
    sparse: &GnuSparseV1,
    path: &str,
    bounds: &ArchiveDecodeBounds,
) -> Result<String> {
    let mut map_bytes = 0_u64;
    let extent_count = read_map_decimal(reader, &mut map_bytes, "extent count", path)?;
    bounds.admit_archive_entry("ALPM GNU sparse extent records", extent_count)?;
    let capacity = usize::try_from(extent_count).map_err(|_| {
        Error::ParseError(format!(
            "GNU sparse extent count for {path} exceeds addressable memory"
        ))
    })?;
    let mut extents = Vec::with_capacity(capacity);
    let mut previous_end = 0_u64;
    let mut stored_payload_bytes = 0_u64;
    for index in 0..extent_count {
        let offset = read_map_decimal(
            reader,
            &mut map_bytes,
            &format!("extent {index} offset"),
            path,
        )?;
        let length = read_map_decimal(
            reader,
            &mut map_bytes,
            &format!("extent {index} length"),
            path,
        )?;
        let end = offset.checked_add(length).ok_or_else(|| {
            Error::ParseError(format!(
                "GNU sparse extent {index} for {path} overflows u64"
            ))
        })?;
        if offset < previous_end || end > sparse.logical_size {
            return Err(Error::ParseError(format!(
                "GNU sparse extent {index} for {path} is overlapping, out of order, or beyond logical size {}",
                sparse.logical_size
            )));
        }
        previous_end = end;
        stored_payload_bytes = stored_payload_bytes.checked_add(length).ok_or_else(|| {
            Error::ParseError(format!(
                "GNU sparse stored payload size for {path} overflows u64"
            ))
        })?;
        extents.push((offset, length));
    }

    let padding = (512 - map_bytes % 512) % 512;
    read_zero_padding(reader, padding, path)?;
    let encoded_size = map_bytes
        .checked_add(padding)
        .and_then(|value| value.checked_add(stored_payload_bytes))
        .ok_or_else(|| {
            Error::ParseError(format!("GNU sparse encoded size for {path} overflows u64"))
        })?;
    if encoded_size != declared_encoded_size {
        return Err(Error::ParseError(format!(
            "GNU sparse payload node {path} declares {declared_encoded_size} encoded bytes but its map requires {encoded_size}"
        )));
    }

    let mut hasher = Hasher::new(HashAlgorithm::Sha256);
    let mut logical_cursor = 0_u64;
    for (index, (offset, length)) in extents.into_iter().enumerate() {
        hash_zeroes(&mut hasher, offset - logical_cursor);
        writer.seek(SeekFrom::Start(offset))?;
        copy_extent(reader, writer, &mut hasher, length, index, path)?;
        logical_cursor = offset + length;
    }
    hash_zeroes(&mut hasher, sparse.logical_size - logical_cursor);
    writer.set_len(sparse.logical_size)?;

    let mut extra = [0_u8; 1];
    if reader.read(&mut extra).map_err(|error| {
        Error::ParseError(format!("read GNU sparse payload node {path}: {error}"))
    })? != 0
    {
        return Err(Error::ParseError(format!(
            "GNU sparse payload node {path} yields bytes beyond its declared extent map"
        )));
    }
    Ok(hasher.finalize().value)
}

fn parse_decimal(value: &[u8], field: &str, path: &str) -> Result<u64> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return Err(Error::ParseError(format!(
            "invalid {field} value for {path}: expected unsigned decimal"
        )));
    }
    let text = std::str::from_utf8(value).expect("ASCII decimal bytes are UTF-8");
    text.parse::<u64>()
        .map_err(|error| Error::ParseError(format!("invalid {field} value for {path}: {error}")))
}

fn read_map_decimal(
    reader: &mut dyn Read,
    consumed: &mut u64,
    field: &str,
    path: &str,
) -> Result<u64> {
    let mut digits = [0_u8; 20];
    let mut length = 0_usize;
    loop {
        let mut byte = [0_u8; 1];
        reader.read_exact(&mut byte).map_err(|error| {
            Error::ParseError(format!("read GNU sparse {field} for {path}: {error}"))
        })?;
        *consumed = consumed.checked_add(1).ok_or_else(|| {
            Error::ParseError(format!("GNU sparse map size for {path} overflows u64"))
        })?;
        if byte[0] == b'\n' {
            if length == 0 {
                return Err(Error::ParseError(format!(
                    "GNU sparse {field} for {path} is empty"
                )));
            }
            return parse_decimal(&digits[..length], field, path);
        }
        if !byte[0].is_ascii_digit() || length == digits.len() {
            return Err(Error::ParseError(format!(
                "GNU sparse {field} for {path} is outside unsigned u64 decimal grammar"
            )));
        }
        digits[length] = byte[0];
        length += 1;
    }
}

fn read_zero_padding(reader: &mut dyn Read, mut remaining: u64, path: &str) -> Result<()> {
    let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
    while remaining != 0 {
        let chunk =
            usize::try_from(remaining.min(buffer.len() as u64)).expect("buffer length fits usize");
        reader.read_exact(&mut buffer[..chunk]).map_err(|error| {
            Error::ParseError(format!("read GNU sparse map padding for {path}: {error}"))
        })?;
        if buffer[..chunk].iter().any(|byte| *byte != 0) {
            return Err(Error::ParseError(format!(
                "GNU sparse map padding for {path} contains non-zero bytes"
            )));
        }
        remaining -= chunk as u64;
    }
    Ok(())
}

fn copy_extent(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    hasher: &mut Hasher,
    mut remaining: u64,
    index: usize,
    path: &str,
) -> Result<()> {
    let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
    while remaining != 0 {
        let chunk =
            usize::try_from(remaining.min(buffer.len() as u64)).expect("buffer length fits usize");
        reader.read_exact(&mut buffer[..chunk]).map_err(|error| {
            Error::ParseError(format!(
                "read GNU sparse extent {index} for {path}: {error}"
            ))
        })?;
        hasher.update(&buffer[..chunk]);
        writer.write_all(&buffer[..chunk])?;
        remaining -= chunk as u64;
    }
    Ok(())
}

fn hash_zeroes(hasher: &mut Hasher, mut remaining: u64) {
    let zeroes = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
    while remaining != 0 {
        let chunk =
            usize::try_from(remaining.min(zeroes.len() as u64)).expect("buffer length fits usize");
        hasher.update(&zeroes[..chunk]);
        remaining -= chunk as u64;
    }
}
