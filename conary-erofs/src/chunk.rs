// conary-erofs/src/chunk.rs
//! Chunk-based external data layout for composefs file references.
//!
//! In composefs mode, regular files use `CHUNK_BASED` layout instead of
//! inline data. Each chunk index entry tells the kernel where to find
//! a chunk of the file. For external data (composefs), all entries use
//! `EROFS_NULL_ADDR` so the kernel fetches content from the CAS directory
//! at mount time.

use std::io::{self, Write};

/// Sentinel block address indicating external (out-of-image) data.
pub const EROFS_NULL_ADDR: u32 = 0xFFFF_FFFF;

/// Mask for the chunk-bits field within the chunk format value.
pub const EROFS_CHUNK_FORMAT_BLKBITS_MASK: u16 = 0x001F;

/// Flag indicating that chunk index entries are present.
pub const EROFS_CHUNK_FORMAT_INDEXES: u16 = 0x0020;

/// Size of a single chunk index entry in bytes.
const CHUNK_INDEX_SIZE: usize = 8;

/// A chunk index entry for composefs external file references.
///
/// Each entry is 8 bytes, little-endian:
/// - `advise` (u16): advisory flags, always 0 for composefs
/// - `device_id` (u16): device table index, 0 for primary device
/// - `blkaddr` (u32): block address, or `EROFS_NULL_ADDR` for external data
pub struct ChunkIndex {
    pub advise: u16,
    pub device_id: u16,
    pub blkaddr: u32,
}

impl ChunkIndex {
    /// Create a chunk index entry pointing to external data (composefs).
    ///
    /// Sets `blkaddr` to `EROFS_NULL_ADDR` so the kernel knows to fetch
    /// the file content from the external CAS store.
    #[must_use]
    pub fn external() -> Self {
        Self {
            advise: 0,
            device_id: 0,
            blkaddr: EROFS_NULL_ADDR,
        }
    }

    /// Serialize this chunk index entry to 8 bytes (little-endian).
    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&self.advise.to_le_bytes())?;
        w.write_all(&self.device_id.to_le_bytes())?;
        w.write_all(&self.blkaddr.to_le_bytes())?;
        Ok(())
    }
}

/// Build chunk index entries for a file of the given size.
///
/// For composefs, every chunk points to external data (`EROFS_NULL_ADDR`).
/// The number of chunks is `ceil(file_size / chunk_size)`, where
/// `chunk_size = 1 << chunk_bits`.
///
/// Returns a tuple of:
/// - The serialized chunk index bytes (8 bytes per chunk)
/// - The chunk format value to store in the inode's `i_u` union field
///
/// A zero-byte file produces no chunk entries.
///
/// NOTE: Not yet integrated into the builder. The current composefs builder
/// relies on `trusted.overlay.redirect` xattrs for CAS references rather
/// than chunk index entries. This function is retained for future use when
/// chunk-index-based composefs images are needed.
#[allow(dead_code)]
pub fn build_chunk_indexes(file_size: u64, chunk_bits: u8) -> (Vec<u8>, u16) {
    let chunk_format = EROFS_CHUNK_FORMAT_INDEXES | (u16::from(chunk_bits) & EROFS_CHUNK_FORMAT_BLKBITS_MASK);

    if file_size == 0 {
        return (Vec::new(), chunk_format);
    }

    let chunk_size = 1u64 << chunk_bits;
    let num_chunks = file_size.div_ceil(chunk_size);

    let mut buf = Vec::with_capacity(num_chunks as usize * CHUNK_INDEX_SIZE);
    for _ in 0..num_chunks {
        let entry = ChunkIndex::external();
        // Writing to a Vec<u8> cannot fail.
        entry.write_to(&mut buf).expect("write to vec failed");
    }

    (buf, chunk_format)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_index_serializes_to_8_bytes() {
        let entry = ChunkIndex::external();
        let mut buf = Vec::new();
        entry.write_to(&mut buf).unwrap();
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn external_chunk_has_null_blkaddr() {
        let entry = ChunkIndex::external();
        assert_eq!(entry.blkaddr, 0xFFFF_FFFF);
        assert_eq!(entry.advise, 0);
        assert_eq!(entry.device_id, 0);
    }

    #[test]
    fn external_chunk_serializes_correctly() {
        let entry = ChunkIndex::external();
        let mut buf = Vec::new();
        entry.write_to(&mut buf).unwrap();
        // advise=0 (2 bytes LE) + device_id=0 (2 bytes LE) + blkaddr=0xFFFFFFFF (4 bytes LE)
        assert_eq!(buf, [0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn chunk_format_encodes_correctly() {
        let chunk_bits: u8 = 12;
        let expected = EROFS_CHUNK_FORMAT_INDEXES | u16::from(chunk_bits);
        let (_, format) = build_chunk_indexes(1, chunk_bits);
        assert_eq!(format, expected);
        assert_eq!(format, 0x0020 | 12);
    }

    #[test]
    fn one_byte_file_yields_one_chunk() {
        let (data, _) = build_chunk_indexes(1, 12);
        assert_eq!(data.len(), 8); // 1 chunk * 8 bytes
    }

    #[test]
    fn file_spanning_two_chunks() {
        // 4097 bytes with chunk_bits=12 (chunk_size=4096) needs 2 chunks.
        let (data, _) = build_chunk_indexes(4097, 12);
        assert_eq!(data.len(), 16); // 2 chunks * 8 bytes
    }

    #[test]
    fn zero_byte_file_yields_no_chunks() {
        let (data, format) = build_chunk_indexes(0, 12);
        assert!(data.is_empty());
        // Format is still valid even for empty files.
        assert_eq!(format, EROFS_CHUNK_FORMAT_INDEXES | 12);
    }

    #[test]
    fn exact_chunk_boundary() {
        // Exactly 4096 bytes with chunk_bits=12 should be 1 chunk, not 2.
        let (data, _) = build_chunk_indexes(4096, 12);
        assert_eq!(data.len(), 8);
    }
}
