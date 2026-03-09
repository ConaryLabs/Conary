// conary-erofs/src/compress.rs
//! Metadata compression for EROFS images.
//!
//! Supports LZ4 and LZMA compression algorithms for metadata blocks.
//! Compression functions return `None` when the compressed output is not
//! smaller than the input, allowing the caller to store data uncompressed.
//!
//! NOTE: Not yet integrated into the builder. The current composefs builder
//! produces uncompressed images. This module is retained for future use
//! when compressed metadata blocks are needed.

use std::io::{self, Cursor};

/// EROFS compression algorithm ID for LZ4.
pub const EROFS_COMPR_ALG_LZ4: u16 = 1;

/// EROFS compression algorithm ID for LZMA.
pub const EROFS_COMPR_ALG_LZMA: u16 = 2;

/// Compression algorithm selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Lz4,
    Lzma,
}

/// Compress data with LZ4. Returns `Some(compressed)` if smaller, `None` otherwise.
pub fn compress_lz4(data: &[u8]) -> Option<Vec<u8>> {
    let compressed = lz4_flex::compress_prepend_size(data);
    if compressed.len() < data.len() {
        Some(compressed)
    } else {
        None
    }
}

/// Decompress LZ4 data (for verification/testing).
pub fn decompress_lz4(data: &[u8], _original_size: usize) -> io::Result<Vec<u8>> {
    lz4_flex::decompress_size_prepended(data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Compress data with LZMA. Returns `Some(compressed)` if smaller, `None` otherwise.
pub fn compress_lzma(data: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Cursor::new(data);
    let mut writer = Vec::new();
    lzma_rs::lzma_compress(&mut reader, &mut writer).ok()?;
    if writer.len() < data.len() {
        Some(writer)
    } else {
        None
    }
}

/// Decompress LZMA data.
pub fn decompress_lzma(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut reader = Cursor::new(data);
    let mut writer = Vec::new();
    lzma_rs::lzma_decompress(&mut reader, &mut writer)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(writer)
}

/// Compress data with the specified algorithm.
///
/// Returns `None` if compression doesn't reduce size (stores uncompressed)
/// or if the algorithm is `Compression::None`.
pub fn compress(data: &[u8], algo: Compression) -> Option<Vec<u8>> {
    match algo {
        Compression::None => None,
        Compression::Lz4 => compress_lz4(data),
        Compression::Lzma => compress_lzma(data),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Highly compressible data: repeated pattern.
    fn compressible_data() -> Vec<u8> {
        "abcdefgh".repeat(1024).into_bytes()
    }

    /// Incompressible data: pseudo-random bytes via simple LCG.
    fn random_data() -> Vec<u8> {
        let mut v = vec![0u8; 4096];
        let mut state: u64 = 0xDEAD_BEEF;
        for b in &mut v {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            *b = (state >> 33) as u8;
        }
        v
    }

    #[test]
    fn lz4_compressible_round_trip() {
        let original = compressible_data();
        let compressed = compress_lz4(&original).expect("compressible data should compress");
        assert!(compressed.len() < original.len());
        let decompressed = decompress_lz4(&compressed, original.len()).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn lz4_incompressible_returns_none() {
        let data = random_data();
        assert!(
            compress_lz4(&data).is_none(),
            "random data should not compress"
        );
    }

    #[test]
    fn lzma_compressible_round_trip() {
        let original = compressible_data();
        let compressed = compress_lzma(&original).expect("compressible data should compress");
        assert!(compressed.len() < original.len());
        let decompressed = decompress_lzma(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn lzma_incompressible_returns_none() {
        let data = random_data();
        assert!(
            compress_lzma(&data).is_none(),
            "random data should not compress"
        );
    }

    #[test]
    fn compress_dispatches_lz4() {
        let original = compressible_data();
        let via_dispatch = compress(&original, Compression::Lz4);
        let via_direct = compress_lz4(&original);
        assert_eq!(via_dispatch, via_direct);
    }

    #[test]
    fn compress_dispatches_lzma() {
        let original = compressible_data();
        let via_dispatch = compress(&original, Compression::Lzma);
        let via_direct = compress_lzma(&original);
        assert_eq!(via_dispatch, via_direct);
    }

    #[test]
    fn compress_none_returns_none() {
        let data = compressible_data();
        assert!(compress(&data, Compression::None).is_none());
    }
}
