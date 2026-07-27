// conary-core/src/filesystem/path.rs

//! Path sanitization utilities for security
//!
//! This module provides functions to safely handle file paths from untrusted
//! sources (packages, repositories, etc.) to prevent path traversal attacks.

use crate::error::{Error, Result};
use crate::filesystem::source_path::SourcePathBytes;
use std::path::{Path, PathBuf};

/// Sanitize a path from an untrusted source
///
/// Delegates to [`SourcePathBytes`], which decides traversal by exact
/// component comparison on bytes. This function:
/// 1. Rejects paths containing an exact `..` component
/// 2. Skips `.` components
/// 3. Drops leading, trailing, and repeated separators, making the path relative
/// 4. Rejects NUL bytes and paths with no deployable components
///
/// Character class is deliberately not consulted. Non-ASCII paths are valid in
/// every source format Conary parses; a name is traversal only if it *is* `..`,
/// not if it merely looks unusual.
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
    Ok(source_path_for(path.as_ref())
        .to_deployment_path()?
        .to_path_buf())
}

/// Recover the declared bytes of a path for validation.
///
/// On Unix this is exact. Validation is defined on bytes rather than on
/// decoded text so that traversal is decided by component identity, not by how
/// a path renders.
#[cfg(unix)]
fn source_path_for(path: &Path) -> SourcePathBytes {
    use std::os::unix::ffi::OsStrExt;
    SourcePathBytes::new(path.as_os_str().as_bytes().to_vec())
}

#[cfg(not(unix))]
fn source_path_for(path: &Path) -> SourcePathBytes {
    SourcePathBytes::new(path.to_string_lossy().as_bytes().to_vec())
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
///
/// let root = std::env::temp_dir().join(format!(
///     "conary-safe-join-doc-{}",
///     std::process::id()
/// ));
/// let _ = std::fs::remove_dir_all(&root);
/// std::fs::create_dir_all(&root).unwrap();
///
/// // Normal paths work
/// assert_eq!(
///     safe_join(&root, "/usr/bin/foo").unwrap(),
///     root.join("usr/bin/foo")
/// );
///
/// // Traversal attempts are rejected
/// assert!(safe_join(&root, "../etc/passwd").is_err());
///
/// std::fs::remove_dir_all(&root).unwrap();
/// ```
pub fn safe_join(root: impl AsRef<Path>, path: impl AsRef<Path>) -> Result<PathBuf> {
    let root = root.as_ref();
    let sanitized = sanitize_path(path.as_ref())?;
    let joined = root.join(&sanitized);
    let canonical_root = root.canonicalize().map_err(|e| {
        Error::PathTraversal(format!(
            "Failed to canonicalize root {}: {}",
            root.display(),
            e
        ))
    })?;

    // Defense in depth: verify the result is under root.
    // First try canonicalize (works when the full path exists).
    if let Ok(canonical_joined) = joined.canonicalize() {
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

    /// Behavior change: non-ASCII paths are accepted.
    ///
    /// The previous rule rejected every non-ASCII path as traversal, which is
    /// not a rule of ALPM, RPM, Debian, tar, or Linux, and rejected valid
    /// repository packages across all three source formats.
    #[test]
    fn sanitize_path_accepts_valid_non_ascii_paths() {
        assert_eq!(
            sanitize_path("usr/bin/cafe\u{301}").unwrap(),
            PathBuf::from("usr/bin/cafe\u{301}")
        );
        assert_eq!(
            sanitize_path("usr/share/doc/n\u{f8}rsk.txt").unwrap(),
            PathBuf::from("usr/share/doc/n\u{f8}rsk.txt")
        );
    }

    /// A separator homoglyph is not a separator.
    ///
    /// U+FF0F FULLWIDTH SOLIDUS renders like `/` but is not the separator byte,
    /// so `usr／bin／tool` is one ordinary component, not three. It cannot
    /// escape a directory precisely because it does not split the path. The old
    /// rule rejected it out of homoglyph anxiety; the correct answer is that
    /// only the separator byte separates.
    #[test]
    fn separator_homoglyphs_are_ordinary_name_bytes_not_separators() {
        let sanitized =
            sanitize_path("usr\u{ff0f}bin\u{ff0f}tool").expect("valid single component");

        assert_eq!(sanitized, PathBuf::from("usr\u{ff0f}bin\u{ff0f}tool"));
        assert_eq!(
            sanitized.components().count(),
            1,
            "a fullwidth solidus must not split the path"
        );
    }

    /// The security property is unchanged: traversal is still rejected, and
    /// dressing it in non-ASCII does not get it through.
    #[test]
    fn traversal_is_still_rejected_regardless_of_character_class() {
        assert!(sanitize_path("usr/\u{e9}t\u{e9}/../../etc/passwd").is_err());
        assert!(sanitize_path("../etc/passwd").is_err());
        assert!(sanitize_path("usr/\u{e9}t\u{e9}/file").is_ok());
    }

    #[test]
    fn test_safe_join_normal() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            safe_join(root.path(), "usr/bin/foo").unwrap(),
            root.path().join("usr/bin/foo")
        );
    }

    #[test]
    fn test_safe_join_traversal_rejected() {
        let root = PathBuf::from("/tmp/test");
        assert!(safe_join(&root, "../etc/passwd").is_err());
        assert!(safe_join(&root, "usr/../../etc/passwd").is_err());
    }

    #[test]
    fn test_safe_join_rejects_uncanonicalizable_root() {
        let temp_dir = tempfile::tempdir().unwrap();
        let missing_root = temp_dir.path().join("missing-root");

        let err = safe_join(&missing_root, "usr/bin/foo")
            .expect_err("missing roots should fail closed instead of silently passing");
        assert!(err.to_string().contains("canonical"));
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
