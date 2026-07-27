// conary-core/src/packages/archive_utils.rs

use crate::error::Result;
use crate::filesystem::source_path::SourcePathBytes;
pub use crate::packages::common::MAX_EXTRACTION_FILE_SIZE;
use tracing::warn;

pub const S_IFMT: u32 = 0o170000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFDIR: u32 = 0o040000;

/// Check if mode corresponds to a regular file
pub fn is_regular_file_mode(mode: u32) -> bool {
    (mode & S_IFMT) == S_IFREG
}

/// Normalize an archive entry path to absolute form with traversal validation.
///
/// Shared by the RPM, Debian, and ALPM payload parsers, so a change here
/// affects every source format.
///
/// Validation happens on the declared bytes: traversal is decided by exact
/// component comparison, not by character class. The UTF-8 requirement comes
/// last and belongs to persistence rather than to safety — Conary stores paths
/// as text, so a path that cannot be represented there fails explicitly instead
/// of being lossily converted into one that does not match the artifact.
pub fn normalize_path(path: &str) -> Result<String> {
    let deployment = SourcePathBytes::from(path).to_deployment_path()?;
    Ok(format!("/{}", deployment.to_utf8()?))
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

    // Use symlink_metadata to avoid following symlinks. The callers
    // (dpkg_query, pacman_query) check the mode bits to detect symlinks
    // and read link targets; following symlinks here would misclassify
    // them as regular files and drop broken links entirely.
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| crate::error::Error::InitError(format!("Failed to stat {}: {}", path, e)))?;
    Ok((
        i64::try_from(meta.len()).unwrap_or(i64::MAX),
        meta.mode() as i32,
    ))
}
