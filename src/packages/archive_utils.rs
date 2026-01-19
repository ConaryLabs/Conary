use crate::hash;
use crate::filesystem::path::sanitize_path;
use tracing::warn;

/// Maximum size for a single file during package extraction (512 MB).
pub const MAX_EXTRACTION_FILE_SIZE: u64 = 512 * 1024 * 1024;

pub const S_IFMT: u32 = 0o170000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFDIR: u32 = 0o040000;

/// Check if mode corresponds to a regular file
pub fn is_regular_file_mode(mode: u32) -> bool {
    (mode & S_IFMT) == S_IFREG
}

/// Normalize archive entry path to absolute form with security sanitization
pub fn normalize_path(path: &str) -> String {
    match sanitize_path(path) {
        Ok(sanitized) => {
            let s = sanitized.to_string_lossy();
            if s.starts_with('/') {
                s.to_string()
            } else {
                format!("/{}", s)
            }
        }
        Err(_) => {
            // Fallback for cases where sanitization fails (e.g. empty)
            // though sanitize_path is strict.
            let trimmed = path.trim_start_matches("./").trim_start_matches('.');
            if trimmed.starts_with('/') {
                trimmed.to_string()
            } else {
                format!("/{}", trimmed)
            }
        }
    }
}

/// Compute SHA256 hash of content
pub fn compute_sha256(content: &[u8]) -> String {
    hash::sha256(content)
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
