// conary-core/src/compression/mod.rs
//! Unified compression/decompression utilities
//!
//! Provides a consistent interface for handling multiple compression formats
//! (gzip, xz, zstd) used across package formats (DEB, Arch, CCS).

use std::io::{self, Read};
use thiserror::Error;

/// Compression-related errors
#[derive(Error, Debug)]
pub enum CompressionError {
    #[error("Failed to create {format} decoder: {source}")]
    DecoderCreation {
        format: &'static str,
        source: io::Error,
    },

    #[error("Failed to decompress {format} data: {source}")]
    Decompression {
        format: &'static str,
        source: io::Error,
    },

    #[error(
        "Decompressed {format} output exceeds {limit} byte limit (possible decompression bomb)"
    )]
    DecompressionBomb { format: &'static str, limit: u64 },

    #[error("Unsupported compression format: {0}")]
    UnsupportedFormat(String),
}

/// Supported compression formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionFormat {
    /// No compression (raw data)
    None,
    /// Gzip compression (.gz)
    Gzip,
    /// XZ/LZMA compression (.xz)
    Xz,
    /// Zstandard compression (.zst)
    Zstd,
}

impl CompressionFormat {
    /// Detect compression format from file extension
    ///
    /// Checks the end of the path for common compression extensions.
    ///
    /// # Examples
    /// ```
    /// use conary_core::compression::CompressionFormat;
    ///
    /// assert_eq!(CompressionFormat::from_extension("data.tar.gz"), CompressionFormat::Gzip);
    /// assert_eq!(CompressionFormat::from_extension("data.tar.xz"), CompressionFormat::Xz);
    /// assert_eq!(CompressionFormat::from_extension("data.tar.zst"), CompressionFormat::Zstd);
    /// assert_eq!(CompressionFormat::from_extension("data.tar"), CompressionFormat::None);
    /// ```
    pub fn from_extension(path: &str) -> Self {
        if path.ends_with(".gz") || path.ends_with(".tgz") {
            Self::Gzip
        } else if path.ends_with(".xz") {
            Self::Xz
        } else if path.ends_with(".zst") || path.ends_with(".zstd") {
            Self::Zstd
        } else {
            Self::None
        }
    }

    /// Detect compression format from magic bytes
    ///
    /// Inspects the first few bytes of data to identify the compression format.
    ///
    /// Magic bytes:
    /// - Gzip: `1f 8b`
    /// - XZ: `fd 37 7a 58 5a 00` (FD + "7zXZ" + NUL)
    /// - Zstd: `28 b5 2f fd`
    pub fn from_magic_bytes(data: &[u8]) -> Self {
        if data.starts_with(&[0x1f, 0x8b]) {
            Self::Gzip
        } else if data.starts_with(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]) {
            Self::Xz
        } else if data.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            Self::Zstd
        } else {
            Self::None
        }
    }

    /// Get the file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            Self::None => "",
            Self::Gzip => ".gz",
            Self::Xz => ".xz",
            Self::Zstd => ".zst",
        }
    }

    /// Get a human-readable name for this format
    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "gzip",
            Self::Xz => "xz",
            Self::Zstd => "zstd",
        }
    }
}

impl std::fmt::Display for CompressionFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Create a decompressing reader for the given format
///
/// Returns a boxed `Read` implementation that decompresses data on the fly.
/// For `CompressionFormat::None`, returns the reader unchanged.
///
/// # Example
/// ```no_run
/// use conary_core::compression::{CompressionFormat, create_decoder};
/// use std::io::Read;
///
/// let compressed_data: &[u8] = &[/* gzip data */];
/// let mut decoder = create_decoder(compressed_data, CompressionFormat::Gzip)?;
/// let mut output = Vec::new();
/// decoder.read_to_end(&mut output)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn create_decoder<'a, R: Read + 'a>(
    reader: R,
    format: CompressionFormat,
) -> Result<Box<dyn Read + 'a>, CompressionError> {
    match format {
        CompressionFormat::None => Ok(Box::new(reader)),
        CompressionFormat::Gzip => Ok(Box::new(flate2::read::GzDecoder::new(reader))),
        CompressionFormat::Xz => Ok(Box::new(liblzma::read::XzDecoder::new(reader))),
        CompressionFormat::Zstd => {
            let decoder =
                zstd::Decoder::new(reader).map_err(|e| CompressionError::DecoderCreation {
                    format: "zstd",
                    source: e,
                })?;
            Ok(Box::new(decoder))
        }
    }
}

/// Create a decompressing reader with a cumulative output limit.
///
/// The returned reader stops after `limit + 1` decompressed bytes so callers can
/// detect and reject oversized streams instead of silently truncating them.
pub fn create_decoder_limited<'a, R: Read + 'a>(
    reader: R,
    format: CompressionFormat,
    limit: u64,
) -> Result<Box<dyn Read + 'a>, CompressionError> {
    let decoder = create_decoder(reader, format)?;
    Ok(Box::new(decoder.take(limit.saturating_add(1))))
}

/// Decompress a byte slice with an explicit output limit.
pub fn decompress_with_limit(
    data: &[u8],
    format: CompressionFormat,
    limit: u64,
) -> Result<Vec<u8>, CompressionError> {
    let mut limited = create_decoder_limited(data, format, limit)?;
    let mut output = Vec::new();
    limited
        .read_to_end(&mut output)
        .map_err(|e| CompressionError::Decompression {
            format: format.name(),
            source: e,
        })?;
    if output.len() as u64 > limit {
        return Err(CompressionError::DecompressionBomb {
            format: format.name(),
            limit,
        });
    }
    Ok(output)
}

/// Decompress a byte slice with automatic format detection and an explicit limit.
pub fn decompress_auto_with_limit(data: &[u8], limit: u64) -> Result<Vec<u8>, CompressionError> {
    let format = CompressionFormat::from_magic_bytes(data);
    decompress_with_limit(data, format, limit)
}

/// Decode bounded in-memory metadata through the shared structural owner.
pub fn decompress_metadata_auto(data: &[u8], field: &str) -> crate::error::Result<Vec<u8>> {
    let format = CompressionFormat::from_magic_bytes(data);
    let bounds = crate::ccs::CCS_BUDGET.archive_decode_bounds()?;
    decompress_metadata_with_bounds(data, format, field, &bounds)
}

/// Decode bounded in-memory metadata with an explicit compression format.
pub fn decompress_metadata(
    data: &[u8],
    format: CompressionFormat,
    field: &str,
) -> crate::error::Result<Vec<u8>> {
    let bounds = crate::ccs::CCS_BUDGET.archive_decode_bounds()?;
    decompress_metadata_with_bounds(data, format, field, &bounds)
}

fn decompress_metadata_with_bounds(
    data: &[u8],
    format: CompressionFormat,
    field: &str,
    bounds: &crate::ccs::ArchiveDecodeBounds,
) -> crate::error::Result<Vec<u8>> {
    match decompress_with_limit(data, format, bounds.max_metadata_bytes) {
        Ok(output) => {
            bounds.admit_metadata_bytes(field, output.len() as u64)?;
            Ok(output)
        }
        Err(CompressionError::DecompressionBomb { .. }) => {
            bounds.admit_metadata_bytes(field, bounds.max_metadata_bytes.saturating_add(1))?;
            unreachable!("one-over metadata admission always refuses")
        }
        Err(error) => Err(crate::error::Error::ParseError(format!(
            "failed to decompress {field}: {error}"
        ))),
    }
}

/// Create a decompressing reader, auto-detecting format from data
///
/// Reads the first few bytes to detect the compression format, then returns
/// a decoder. Note: the data must be available as a slice since we need to
/// peek at the magic bytes.
pub fn create_decoder_auto(data: &[u8]) -> Result<Box<dyn Read + '_>, CompressionError> {
    let format = CompressionFormat::from_magic_bytes(data);
    create_decoder(data, format)
}

/// Reject archives with pathologically large entry counts.
#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    #[test]
    fn test_format_from_extension() {
        assert_eq!(
            CompressionFormat::from_extension("data.tar.gz"),
            CompressionFormat::Gzip
        );
        assert_eq!(
            CompressionFormat::from_extension("data.tgz"),
            CompressionFormat::Gzip
        );
        assert_eq!(
            CompressionFormat::from_extension("data.tar.xz"),
            CompressionFormat::Xz
        );
        assert_eq!(
            CompressionFormat::from_extension("data.tar.zst"),
            CompressionFormat::Zstd
        );
        assert_eq!(
            CompressionFormat::from_extension("data.tar.zstd"),
            CompressionFormat::Zstd
        );
        assert_eq!(
            CompressionFormat::from_extension("data.tar"),
            CompressionFormat::None
        );
        assert_eq!(
            CompressionFormat::from_extension("plain.txt"),
            CompressionFormat::None
        );
    }

    #[test]
    fn test_format_from_magic_bytes() {
        // Gzip magic: 1f 8b
        assert_eq!(
            CompressionFormat::from_magic_bytes(&[0x1f, 0x8b, 0x08, 0x00]),
            CompressionFormat::Gzip
        );

        // XZ magic: fd 37 7a 58 5a 00
        assert_eq!(
            CompressionFormat::from_magic_bytes(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]),
            CompressionFormat::Xz
        );

        // Zstd magic: 28 b5 2f fd
        assert_eq!(
            CompressionFormat::from_magic_bytes(&[0x28, 0xb5, 0x2f, 0xfd]),
            CompressionFormat::Zstd
        );

        // Unknown/no compression
        assert_eq!(
            CompressionFormat::from_magic_bytes(&[0x00, 0x00, 0x00, 0x00]),
            CompressionFormat::None
        );

        // Too short for any magic
        assert_eq!(
            CompressionFormat::from_magic_bytes(&[0x1f]),
            CompressionFormat::None
        );
    }

    #[test]
    fn test_format_display() {
        assert_eq!(format!("{}", CompressionFormat::Gzip), "gzip");
        assert_eq!(format!("{}", CompressionFormat::Xz), "xz");
        assert_eq!(format!("{}", CompressionFormat::Zstd), "zstd");
        assert_eq!(format!("{}", CompressionFormat::None), "none");
    }

    #[test]
    fn test_decompress_none() {
        let data = b"hello world";
        let result =
            decompress_with_limit(data, CompressionFormat::None, data.len() as u64).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_decompress_gzip() {
        // Minimal gzip of "hello"
        let gzip_data: &[u8] = &[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xcb, 0x48, 0xcd, 0xc9,
            0xc9, 0x07, 0x00, 0x86, 0xa6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00,
        ];
        let result = decompress_with_limit(gzip_data, CompressionFormat::Gzip, 1024).unwrap();
        assert_eq!(result, b"hello");
    }

    #[test]
    fn metadata_decode_uses_the_typed_metadata_dimension() {
        // Minimal gzip of "hello"
        let gzip_data: &[u8] = &[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xcb, 0x48, 0xcd, 0xc9,
            0xc9, 0x07, 0x00, 0x86, 0xa6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00,
        ];
        let bounds = crate::ccs::ArchiveDecodeBounds {
            max_archive_entries: 1,
            max_payload_object_bytes: 4,
            max_total_payload_bytes: 4,
            max_metadata_bytes: 4,
            max_decompressed_stream_bytes: 1024,
        };
        let error = decompress_metadata_with_bounds(
            gzip_data,
            CompressionFormat::Gzip,
            "test repository metadata",
            &bounds,
        )
        .unwrap_err();
        let crate::error::Error::Budget(error) = error else {
            panic!("expected typed budget refusal");
        };
        assert_eq!(error.dimension, crate::ccs::BudgetDimension::MetadataBytes);
        assert_eq!(error.field, "test repository metadata");
        assert_eq!(error.observed, 5);
        assert_eq!(error.limit, 4);
    }

    #[test]
    fn test_decompress_auto_with_limit_rejects_oversized_output() {
        let payload = vec![b'a'; 1024];
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&payload).unwrap();
        let compressed = encoder.finish().unwrap();

        let err = decompress_auto_with_limit(&compressed, 512).unwrap_err();
        assert!(matches!(
            err,
            CompressionError::DecompressionBomb {
                format: "gzip",
                limit: 512
            }
        ));
    }
}
