use sha2::{Digest, Sha256};
use tracing::warn;

pub const MAX_EXTRACTION_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB

pub const S_IFMT: u32 = 0o170000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFDIR: u32 = 0o040000;

/// Check if mode corresponds to a regular file
pub fn is_regular_file_mode(mode: u32) -> bool {
    (mode & S_IFMT) == S_IFREG
}

/// Normalize archive entry path to absolute form
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_start_matches("./").trim_start_matches('.');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{}", trimmed)
    }
}

/// Compute SHA256 hash of content
pub fn compute_sha256(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("{:x}", hasher.finalize())
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
