// conary-core/src/generation/delta.rs

//! Binary delta support between EROFS generation images.
//!
//! Since EROFS images are deterministic and contain only metadata
//! (no file content -- that lives in CAS), deltas between generations
//! are very compact. A typical update touching 50 packages produces
//! a delta of a few KB.
//!
//! # Algorithm
//!
//! Deltas are computed using zstd dictionary compression, treating the
//! old image as a dictionary for compressing the new image:
//!
//! ```text
//! delta = zstd_compress(new_image, dictionary=old_image)
//! new_image = zstd_decompress(delta, dictionary=old_image)
//! ```
//!
//! This leverages the structural similarity of consecutive EROFS images
//! (same inode table layout, same path strings, mostly identical blocks)
//! for very high compression ratios.

use std::io::Read;
use std::path::Path;

use crate::error::{Error, Result};

/// Default zstd compression level for generation deltas.
///
/// Level 3 provides a good balance between speed and ratio.
const COMPRESSION_LEVEL: i32 = 3;

/// Maximum decompressed delta output (512 MiB).
///
/// EROFS generation images should never approach this size; the limit
/// guards against malformed or malicious delta inputs.
const MAX_OUTPUT_SIZE: u64 = 512 * 1024 * 1024;

/// Compute a binary delta between two EROFS images.
///
/// Compresses `new_image` using `old_image` as a zstd dictionary.
/// The resulting delta is typically orders of magnitude smaller than
/// either image for consecutive generation updates.
///
/// # Errors
///
/// Returns [`Error::DeltaError`] if compression fails.
pub fn compute_delta(old_image: &[u8], new_image: &[u8]) -> Result<Vec<u8>> {
    let encoder_dict = zstd::dict::EncoderDictionary::copy(old_image, COMPRESSION_LEVEL);

    let mut encoder =
        zstd::Encoder::with_prepared_dictionary(Vec::new(), &encoder_dict).map_err(|e| {
            Error::DeltaError(format!(
                "Failed to create zstd encoder with dictionary: {e}"
            ))
        })?;

    std::io::Write::write_all(&mut encoder, new_image)
        .map_err(|e| Error::DeltaError(format!("Failed to write image data to encoder: {e}")))?;

    encoder
        .finish()
        .map_err(|e| Error::DeltaError(format!("Failed to finish zstd compression: {e}")))
}

/// Apply a binary delta to produce a new EROFS image.
///
/// Decompresses `delta` using `old_image` as a zstd dictionary,
/// reconstructing the new image that was originally compressed.
///
/// # Errors
///
/// Returns [`Error::DeltaError`] if decompression fails or the output
/// exceeds [`MAX_OUTPUT_SIZE`].
pub fn apply_delta(old_image: &[u8], delta: &[u8]) -> Result<Vec<u8>> {
    let decoder_dict = zstd::dict::DecoderDictionary::copy(old_image);

    let mut decoder =
        zstd::Decoder::with_prepared_dictionary(delta, &decoder_dict).map_err(|e| {
            Error::DeltaError(format!(
                "Failed to create zstd decoder with dictionary: {e}"
            ))
        })?;

    let mut output = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = decoder
            .read(&mut buf)
            .map_err(|e| Error::DeltaError(format!("Failed to read decompressed data: {e}")))?;
        if n == 0 {
            break;
        }
        output.extend_from_slice(&buf[..n]);
        if output.len() as u64 > MAX_OUTPUT_SIZE {
            return Err(Error::DeltaError(format!(
                "Delta output exceeds maximum allowed size ({MAX_OUTPUT_SIZE} bytes)"
            )));
        }
    }

    Ok(output)
}

/// Compute delta between two generation image files on disk.
///
/// Reads both files into memory and delegates to [`compute_delta`].
///
/// # Errors
///
/// Returns an error if either file cannot be read or compression fails.
pub fn compute_generation_delta(old_path: &Path, new_path: &Path) -> Result<Vec<u8>> {
    let old = std::fs::read(old_path).map_err(|e| {
        Error::IoError(format!(
            "Failed to read old generation image {}: {e}",
            old_path.display()
        ))
    })?;
    let new = std::fs::read(new_path).map_err(|e| {
        Error::IoError(format!(
            "Failed to read new generation image {}: {e}",
            new_path.display()
        ))
    })?;
    compute_delta(&old, &new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ---------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------

    /// Build a minimal synthetic EROFS-like buffer.
    ///
    /// Real EROFS images begin with 1024 bytes of padding followed by the
    /// superblock magic at offset 1024. For delta tests we only need
    /// repeatable, realistic-looking byte sequences; we embed the magic
    /// so tests that inspect it still pass.
    fn make_erofs_image(paths: &[(&str, &str)]) -> Vec<u8> {
        // 1024-byte leading pad
        let mut buf = vec![0u8; 1024];
        // EROFS superblock magic (LE) at offset 1024
        buf.extend_from_slice(&0xE0F5_E1E2_u32.to_le_bytes());
        // Pad superblock to 128 bytes
        buf.extend_from_slice(&[0u8; 124]);
        // Encode the path table so consecutive images differ predictably
        for (path, hash) in paths {
            let entry = format!("{path}\0{hash}\n");
            buf.extend_from_slice(entry.as_bytes());
        }
        // Align to 4096
        let rem = buf.len() % 4096;
        if rem != 0 {
            buf.extend_from_slice(&vec![0u8; 4096 - rem]);
        }
        buf
    }

    // ---------------------------------------------------------------
    // test_roundtrip
    // ---------------------------------------------------------------

    /// Compute a delta between two similar images and verify that applying it
    /// to the old image exactly reproduces the new image.
    #[cfg(feature = "composefs-rs")]
    #[test]
    fn test_roundtrip_composefs() {
        use crate::generation::builder::{FileEntryRef, build_erofs_image};

        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();

        let entries_v1 = vec![
            FileEntryRef {
                path: "/usr/bin/hello".to_string(),
                sha256_hash: "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd"
                    .to_string(),
                size: 1024,
                permissions: 0o755,
                owner: None,
                group_name: None,
            },
            FileEntryRef {
                path: "/usr/lib/libfoo.so".to_string(),
                sha256_hash: "1122334411223344112233441122334411223344112233441122334411223344"
                    .to_string(),
                size: 4096,
                permissions: 0o644,
                owner: None,
                group_name: None,
            },
        ];
        let entries_v2 = vec![
            FileEntryRef {
                path: "/usr/bin/hello".to_string(),
                sha256_hash: "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd"
                    .to_string(),
                size: 1024,
                permissions: 0o755,
                owner: None,
                group_name: None,
            },
            FileEntryRef {
                path: "/usr/lib/libfoo.so".to_string(),
                sha256_hash: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
                    .to_string(),
                size: 8192,
                permissions: 0o644,
                owner: None,
                group_name: None,
            },
            FileEntryRef {
                path: "/usr/bin/world".to_string(),
                sha256_hash: "cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe"
                    .to_string(),
                size: 512,
                permissions: 0o755,
                owner: None,
                group_name: None,
            },
        ];

        let r1 = build_erofs_image(&entries_v1, &[], tmp1.path()).unwrap();
        let r2 = build_erofs_image(&entries_v2, &[], tmp2.path()).unwrap();

        let old_bytes = std::fs::read(&r1.image_path).unwrap();
        let new_bytes = std::fs::read(&r2.image_path).unwrap();

        let delta = compute_delta(&old_bytes, &new_bytes).unwrap();
        let reconstructed = apply_delta(&old_bytes, &delta).unwrap();

        assert_eq!(
            reconstructed, new_bytes,
            "Reconstructed image must be byte-for-byte identical to new image"
        );
    }

    /// Synthetic roundtrip that does not require the composefs-rs feature.
    #[test]
    fn test_roundtrip_synthetic() {
        let old = make_erofs_image(&[
            (
                "/usr/bin/hello",
                "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd",
            ),
            (
                "/usr/lib/libfoo.so",
                "1122334411223344112233441122334411223344112233441122334411223344",
            ),
        ]);
        let new = make_erofs_image(&[
            (
                "/usr/bin/hello",
                "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd",
            ),
            (
                "/usr/lib/libfoo.so",
                "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            ),
            (
                "/usr/bin/world",
                "cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe",
            ),
        ]);

        let delta = compute_delta(&old, &new).unwrap();
        let reconstructed = apply_delta(&old, &delta).unwrap();

        assert_eq!(
            reconstructed, new,
            "Roundtrip must reconstruct the new image exactly"
        );
    }

    // ---------------------------------------------------------------
    // test_delta_is_compact
    // ---------------------------------------------------------------

    /// Verify that a delta between two similar images is significantly
    /// smaller than the new image itself.
    #[test]
    fn test_delta_is_compact() {
        // Build two large, mostly-identical images (1,000 paths)
        let base_paths: Vec<(String, String)> = (0..1000)
            .map(|i| {
                (
                    format!("/usr/lib/libpkg{i:04}.so"),
                    format!("{:064x}", i as u64),
                )
            })
            .collect();

        let old_paths: Vec<(&str, &str)> = base_paths
            .iter()
            .map(|(p, h)| (p.as_str(), h.as_str()))
            .collect();

        // New image: only the last 10 entries differ
        let mut new_path_data = base_paths.clone();
        for entry in new_path_data.iter_mut().skip(990) {
            entry.1 = format!("{:064x}", 0xdead_beef_u64);
        }
        let new_paths: Vec<(&str, &str)> = new_path_data
            .iter()
            .map(|(p, h)| (p.as_str(), h.as_str()))
            .collect();

        let old = make_erofs_image(&old_paths);
        let new = make_erofs_image(&new_paths);

        let delta = compute_delta(&old, &new).unwrap();

        // Delta should be far smaller than the new image
        assert!(
            delta.len() < new.len() / 2,
            "Delta ({} bytes) should be less than half the new image ({} bytes)",
            delta.len(),
            new.len()
        );
    }

    // ---------------------------------------------------------------
    // test_identical_images
    // ---------------------------------------------------------------

    /// Delta of identical images should be very small (essentially just
    /// the zstd framing overhead with near-zero content).
    #[test]
    fn test_identical_images() {
        let image = make_erofs_image(&[(
            "/usr/bin/hello",
            "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd",
        )]);

        let delta = compute_delta(&image, &image).unwrap();
        let reconstructed = apply_delta(&image, &delta).unwrap();

        assert_eq!(
            reconstructed, image,
            "Identity delta must reconstruct image"
        );
        // Allow up to 256 bytes for zstd framing; identical data compresses to nearly nothing
        assert!(
            delta.len() < 256,
            "Identical-image delta should be tiny, got {} bytes",
            delta.len()
        );
    }

    // ---------------------------------------------------------------
    // test_empty_old_image
    // ---------------------------------------------------------------

    /// When the old image is empty, the delta is effectively a compressed
    /// copy of the new image (no shared dictionary content).
    #[test]
    fn test_empty_old_image() {
        let new = make_erofs_image(&[(
            "/usr/bin/hello",
            "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd",
        )]);

        let delta = compute_delta(&[], &new).unwrap();
        let reconstructed = apply_delta(&[], &delta).unwrap();

        assert_eq!(
            reconstructed, new,
            "Empty-old delta must still reconstruct the new image"
        );
        // The delta should be non-empty (it holds the compressed new image)
        assert!(
            !delta.is_empty(),
            "Delta from empty old image must be non-empty"
        );
    }

    // ---------------------------------------------------------------
    // test_compute_generation_delta (file-based API)
    // ---------------------------------------------------------------

    #[test]
    fn test_compute_generation_delta() {
        let tmp = TempDir::new().unwrap();
        let old_path = tmp.path().join("gen1.erofs");
        let new_path = tmp.path().join("gen2.erofs");

        let old = make_erofs_image(&[(
            "/usr/bin/hello",
            "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd",
        )]);
        let new = make_erofs_image(&[
            (
                "/usr/bin/hello",
                "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd",
            ),
            (
                "/usr/bin/world",
                "cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe",
            ),
        ]);

        std::fs::write(&old_path, &old).unwrap();
        std::fs::write(&new_path, &new).unwrap();

        let delta = compute_generation_delta(&old_path, &new_path).unwrap();
        let reconstructed = apply_delta(&old, &delta).unwrap();

        assert_eq!(
            reconstructed, new,
            "File-based delta API must produce correct delta"
        );
    }

    // ---------------------------------------------------------------
    // test_compute_generation_delta_missing_file
    // ---------------------------------------------------------------

    #[test]
    fn test_compute_generation_delta_missing_file() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does_not_exist.erofs");
        let other = tmp.path().join("other.erofs");
        std::fs::write(&other, b"data").unwrap();

        let result = compute_generation_delta(&missing, &other);
        assert!(
            result.is_err(),
            "Reading a missing file must return an error"
        );

        let result2 = compute_generation_delta(&other, &missing);
        assert!(
            result2.is_err(),
            "Reading a missing file must return an error"
        );
    }
}
