// conary-core/src/packages/archive_utils.rs

use crate::error::Result;
use crate::filesystem::path::sanitize_path;
pub use crate::packages::common::MAX_EXTRACTION_FILE_SIZE;
use tracing::warn;

pub const S_IFMT: u32 = 0o170000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFDIR: u32 = 0o040000;

/// Check if mode corresponds to a regular file
pub fn is_regular_file_mode(mode: u32) -> bool {
    (mode & S_IFMT) == S_IFREG
}

/// Normalize archive entry path to absolute form with security sanitization
pub fn normalize_path(path: &str) -> Result<String> {
    let sanitized = sanitize_path(path)?;
    let s = sanitized.to_string_lossy();
    if s.starts_with('/') {
        Ok(s.to_string())
    } else {
        Ok(format!("/{}", s))
    }
}

/// Check if file size exceeds limit, warn if so
pub fn check_file_size(path: &str, size: u64) -> bool {
    if size > MAX_EXTRACTION_FILE_SIZE {
        warn!("Skipping oversized file: {} ({} bytes)", path, size);
        false
    } else {
        true
    }
}

/// Get file metadata (size and mode) from the filesystem.
///
/// Returns `(size_in_bytes, raw_mode)` on success, or an error if the file
/// cannot be stat'd (e.g., permission denied, missing file).
pub fn get_file_metadata(path: &str) -> Result<(i64, i32)> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::metadata(path).map_err(|e| {
        crate::error::Error::InitError(format!("Failed to stat {}: {}", path, e))
    })?;
    Ok((meta.len() as i64, meta.mode() as i32))
}
