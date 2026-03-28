// conary-core/src/filesystem/path.rs

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
/// use conary_core::filesystem::path::sanitize_path;
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

    // Reject null bytes -- these truncate paths at C API boundaries (open(), stat(), etc.)
    if path_str.contains('\0') {
        return Err(Error::PathTraversal("path contains null byte".to_string()));
    }

    // Reject non-ASCII paths from untrusted sources to avoid Unicode
    // normalization edge cases on filesystems that treat homoglyphs as
    // separators or normalize canonically equivalent forms.
    if !path_str.is_ascii() {
        return Err(Error::PathTraversal(
            "path contains non-ASCII characters".to_string(),
        ));
    }

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
        return Err(Error::InvalidPath(
            "Empty path after sanitization".to_string(),
        ));
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
/// use conary_core::filesystem::path::safe_join;
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

    // Defense in depth: verify the result is under root.
    // First try canonicalize (works when the full path exists).
    if let (Ok(canonical_root), Ok(canonical_joined)) = (root.canonicalize(), joined.canonicalize())
    {
        if !canonical_joined.starts_with(&canonical_root) {
            return Err(Error::PathTraversal(format!(
                "Path {} escapes root {}",
                joined.display(),
                root.display()
            )));
        }
        return Ok(joined);
    }

    // When the final path doesn't exist yet, walk existing ancestors to
    // ensure none of them are symlinks that escape the root. An attacker
    // can plant a symlink inside root so that the joined path resolves
    // outside root once the caller creates the file.
    //
    // If the root itself doesn't exist, we can't do symlink checks but
    // sanitize_path already rejected traversal components.
    let canonical_root = match root.canonicalize() {
        Ok(cr) => cr,
        Err(_) => return Ok(joined),
    };

    let mut check = root.to_path_buf();
    for component in sanitized.components() {
        check.push(component);
        // Only check components that exist on disk.
        match check.symlink_metadata() {
            Ok(meta) if meta.is_symlink() => {
                // Resolve the symlink and verify it stays under root.
                match check.canonicalize() {
                    Ok(resolved) if !resolved.starts_with(&canonical_root) => {
                        return Err(Error::PathTraversal(format!(
                            "Symlink at {} escapes root {}",
                            check.display(),
                            root.display()
                        )));
                    }
                    Ok(_) => {} // Symlink stays under root, continue
                    Err(_) => {
                        // Dangling symlink -- reject to be safe
                        return Err(Error::PathTraversal(format!(
                            "Dangling symlink at {} under root {}",
                            check.display(),
                            root.display()
                        )));
                    }
                }
            }
            Ok(_) => {}      // Regular file or directory, fine
            Err(_) => break, // Path doesn't exist yet, stop checking
        }
    }

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
/// use conary_core::filesystem::path::sanitize_filename;
///
/// // Normal filenames work
/// assert_eq!(sanitize_filename("package-1.0.rpm").unwrap(), "package-1.0.rpm");
///
/// // Paths are rejected
/// assert!(sanitize_filename("../package.rpm").is_err());
/// assert!(sanitize_filename("subdir/package.rpm").is_err());
/// ```
pub fn sanitize_filename(name: &str) -> Result<String> {
    // Check for null bytes -- these truncate filenames at C API boundaries
    if name.contains('\0') {
        return Err(Error::PathTraversal(format!(
            "Filename contains null byte: {}",
            name.replace('\0', "\\0")
        )));
    }

    // Check for path separators
    if name.contains('/') || name.contains('\\') {
        return Err(Error::PathTraversal(format!(
            "Filename contains path separator: {}",
            name
        )));
    }

    // Check for path traversal
    if name == ".." || name == "." {
        return Err(Error::PathTraversal(format!("Invalid filename: {}", name)));
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
    fn test_sanitize_path_null_byte_rejected() {
        assert!(sanitize_path("usr/bin/foo\0bar").is_err());
        assert!(sanitize_path("\0").is_err());
        assert!(sanitize_path("etc/passwd\0.bak").is_err());
    }

    #[test]
    fn test_sanitize_path_empty_rejected() {
        assert!(sanitize_path("").is_err());
        assert!(sanitize_path("/").is_err());
        assert!(sanitize_path("///").is_err());
        assert!(sanitize_path("./").is_err());
    }

    #[test]
    fn test_sanitize_path_non_ascii_rejected() {
        assert!(sanitize_path("usr/bin/cafe\u{301}").is_err());
        assert!(sanitize_path("usr\u{ff0f}bin\u{ff0f}tool").is_err());
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
        assert_eq!(
            sanitize_filename("file-1.0.tar.gz").unwrap(),
            "file-1.0.tar.gz"
        );
    }

    #[test]
    fn test_sanitize_filename_path_rejected() {
        assert!(sanitize_filename("../package.rpm").is_err());
        assert!(sanitize_filename("subdir/package.rpm").is_err());
        assert!(sanitize_filename("..").is_err());
        assert!(sanitize_filename(".").is_err());
        assert!(sanitize_filename("").is_err());
    }

    #[test]
    fn test_sanitize_filename_null_byte_rejected() {
        assert!(sanitize_filename("package\0.rpm").is_err());
        assert!(sanitize_filename("\0").is_err());
        assert!(sanitize_filename("etc/passwd\0.rpm").is_err());
    }
}
