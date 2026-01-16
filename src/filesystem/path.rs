// src/filesystem/path.rs

//! Path sanitization utilities for security
//!
//! This module provides functions to safely handle file paths from untrusted
//! sources (packages, repositories, etc.) to prevent path traversal attacks.

use crate::error::{Error, Result};
use std::path::{Component, Path, PathBuf};

/// Sanitize a path from an untrusted source
///
/// This function:
/// 1. Rejects paths containing `..` (parent directory) components
/// 2. Skips `.` (current directory) components
/// 3. Strips leading slashes to make the path relative
/// 4. Returns an error for empty paths
///
/// # Security
///
/// This is a critical security function. Malicious packages could attempt
/// to write files outside their intended directory using paths like:
/// - `../../../etc/passwd`
/// - `/etc/passwd` (absolute paths)
/// - `foo/../../bar`
///
/// # Examples
///
/// ```
/// use conary::filesystem::path::sanitize_path;
/// use std::path::PathBuf;
///
/// // Normal paths are preserved
/// assert_eq!(sanitize_path("usr/bin/foo").unwrap(), PathBuf::from("usr/bin/foo"));
///
/// // Leading slashes are stripped
/// assert_eq!(sanitize_path("/usr/bin/foo").unwrap(), PathBuf::from("usr/bin/foo"));
///
/// // Path traversal is rejected
/// assert!(sanitize_path("../etc/passwd").is_err());
/// assert!(sanitize_path("usr/../../../etc/passwd").is_err());
/// ```
pub fn sanitize_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    let path_str = path.to_string_lossy();

    // Strip leading slashes to make relative
    let relative = path_str.trim_start_matches('/');

    let mut normalized = PathBuf::new();

    for component in Path::new(relative).components() {
        match component {
            Component::Normal(c) => {
                // Normal path component - keep it
                normalized.push(c);
            }
            Component::CurDir => {
                // "." - skip it
            }
            Component::ParentDir => {
                // ".." - this is a path traversal attempt
                return Err(Error::PathTraversal(path_str.to_string()));
            }
            Component::Prefix(_) | Component::RootDir => {
                // Skip Windows prefixes and root markers
                // (we already stripped leading slashes)
            }
        }
    }

    // Reject empty paths
    if normalized.as_os_str().is_empty() {
        return Err(Error::InvalidPath("Empty path after sanitization".to_string()));
    }

    Ok(normalized)
}

/// Safely join a root path with an untrusted path
///
/// This function sanitizes the path and joins it with the root, ensuring
/// the result cannot escape the root directory.
///
/// # Security
///
/// This provides defense-in-depth by:
/// 1. Sanitizing the path to remove traversal attempts
/// 2. Verifying the final path is under the root (catches edge cases)
///
/// # Examples
///
/// ```
/// use conary::filesystem::path::safe_join;
/// use std::path::{Path, PathBuf};
///
/// let root = Path::new("/var/conary");
///
/// // Normal paths work
/// assert_eq!(
///     safe_join(root, "/usr/bin/foo").unwrap(),
///     PathBuf::from("/var/conary/usr/bin/foo")
/// );
///
/// // Traversal attempts are rejected
/// assert!(safe_join(root, "../etc/passwd").is_err());
/// ```
pub fn safe_join(root: impl AsRef<Path>, path: impl AsRef<Path>) -> Result<PathBuf> {
    let root = root.as_ref();
    let sanitized = sanitize_path(path.as_ref())?;
    let joined = root.join(&sanitized);

    // Defense in depth: verify the result is under root
    // This catches any edge cases we might have missed
    if let (Ok(canonical_root), Ok(canonical_joined)) =
        (root.canonicalize(), joined.canonicalize())
        && !canonical_joined.starts_with(&canonical_root)
    {
        return Err(Error::PathTraversal(format!(
            "Path {} escapes root {}",
            joined.display(),
            root.display()
        )));
    }
    // Note: If canonicalize fails (e.g., path doesn't exist yet),
    // we rely on the sanitize_path check above

    Ok(joined)
}

/// Sanitize a filename (single path component) from an untrusted source
///
/// This is stricter than `sanitize_path` - it rejects any path separators.
/// Use this for filenames that should not contain directory components.
///
/// # Examples
///
/// ```
/// use conary::filesystem::path::sanitize_filename;
///
/// // Normal filenames work
/// assert_eq!(sanitize_filename("package-1.0.rpm").unwrap(), "package-1.0.rpm");
///
/// // Paths are rejected
/// assert!(sanitize_filename("../package.rpm").is_err());
/// assert!(sanitize_filename("subdir/package.rpm").is_err());
/// ```
pub fn sanitize_filename(name: &str) -> Result<String> {
    // Check for path separators
    if name.contains('/') || name.contains('\\') {
        return Err(Error::PathTraversal(format!(
            "Filename contains path separator: {}",
            name
        )));
    }

    // Check for path traversal
    if name == ".." || name == "." {
        return Err(Error::PathTraversal(format!(
            "Invalid filename: {}",
            name
        )));
    }

    // Check for empty
    if name.is_empty() {
        return Err(Error::InvalidPath("Empty filename".to_string()));
    }

    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_path_normal() {
        assert_eq!(
            sanitize_path("usr/bin/foo").unwrap(),
            PathBuf::from("usr/bin/foo")
        );
        assert_eq!(
            sanitize_path("usr/lib/libfoo.so").unwrap(),
            PathBuf::from("usr/lib/libfoo.so")
        );
    }

    #[test]
    fn test_sanitize_path_leading_slash() {
        assert_eq!(
            sanitize_path("/usr/bin/foo").unwrap(),
            PathBuf::from("usr/bin/foo")
        );
        assert_eq!(
            sanitize_path("///usr/bin/foo").unwrap(),
            PathBuf::from("usr/bin/foo")
        );
    }

    #[test]
    fn test_sanitize_path_dot() {
        assert_eq!(
            sanitize_path("./usr/bin/foo").unwrap(),
            PathBuf::from("usr/bin/foo")
        );
        assert_eq!(
            sanitize_path("usr/./bin/./foo").unwrap(),
            PathBuf::from("usr/bin/foo")
        );
    }

    #[test]
    fn test_sanitize_path_traversal_rejected() {
        assert!(sanitize_path("..").is_err());
        assert!(sanitize_path("../etc/passwd").is_err());
        assert!(sanitize_path("usr/../../../etc/passwd").is_err());
        assert!(sanitize_path("usr/bin/../../..").is_err());
        assert!(sanitize_path("/usr/../etc/passwd").is_err());
    }

    #[test]
    fn test_sanitize_path_empty_rejected() {
        assert!(sanitize_path("").is_err());
        assert!(sanitize_path("/").is_err());
        assert!(sanitize_path("///").is_err());
        assert!(sanitize_path("./").is_err());
    }

    #[test]
    fn test_safe_join_normal() {
        let root = PathBuf::from("/tmp/test");
        assert_eq!(
            safe_join(&root, "usr/bin/foo").unwrap(),
            PathBuf::from("/tmp/test/usr/bin/foo")
        );
    }

    #[test]
    fn test_safe_join_traversal_rejected() {
        let root = PathBuf::from("/tmp/test");
        assert!(safe_join(&root, "../etc/passwd").is_err());
        assert!(safe_join(&root, "usr/../../etc/passwd").is_err());
    }

    #[test]
    fn test_sanitize_filename_normal() {
        assert_eq!(sanitize_filename("package.rpm").unwrap(), "package.rpm");
        assert_eq!(sanitize_filename("file-1.0.tar.gz").unwrap(), "file-1.0.tar.gz");
    }

    #[test]
    fn test_sanitize_filename_path_rejected() {
        assert!(sanitize_filename("../package.rpm").is_err());
        assert!(sanitize_filename("subdir/package.rpm").is_err());
        assert!(sanitize_filename("..").is_err());
        assert!(sanitize_filename(".").is_err());
        assert!(sanitize_filename("").is_err());
    }
}
