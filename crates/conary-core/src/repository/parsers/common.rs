// conary-core/src/repository/parsers/common.rs

//! Shared parser helpers for repository metadata parsers.
//!
//! Eliminates duplication across the Arch, Debian, and Fedora parsers for
//! common operations like download URL construction and path validation.

/// Maximum allowed package size (5 GB).
///
/// Shared across all parsers to reject unreasonably large packages.
pub const MAX_PACKAGE_SIZE: u64 = 5 * 1024 * 1024 * 1024;

/// Construct a metadata URL by joining a base URL with a relative path.
///
/// Normalizes trailing slashes on the base URL.
pub fn join_repo_url(base_url: &str, path: &str) -> String {
    format!("{}/{}", base_url.trim_end_matches('/'), path)
}

/// Validate that a filename is safe (no path traversal, not absolute, no
/// URL schemes).
///
/// Returns `Ok(())` if safe, or an error description if suspicious.
pub fn validate_filename(filename: &str) -> Result<(), String> {
    if filename.contains("..") {
        return Err(format!(
            "Suspicious filename (path traversal): {}",
            filename
        ));
    }
    if filename.starts_with('/') || filename.contains("://") {
        return Err(format!(
            "Suspicious filename (not relative path): {}",
            filename
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_repo_url() {
        assert_eq!(
            join_repo_url("https://repo.example.com/", "repodata/repomd.xml"),
            "https://repo.example.com/repodata/repomd.xml"
        );
        assert_eq!(
            join_repo_url("https://repo.example.com", "Packages/a/app.rpm"),
            "https://repo.example.com/Packages/a/app.rpm"
        );
    }

    #[test]
    fn test_validate_filename_safe() {
        assert!(validate_filename("Packages/a/app.rpm").is_ok());
        assert!(validate_filename("pool/main/g/glibc.deb").is_ok());
    }

    #[test]
    fn test_validate_filename_traversal() {
        assert!(validate_filename("../../../etc/passwd").is_err());
    }

    #[test]
    fn test_validate_filename_absolute() {
        assert!(validate_filename("/etc/passwd").is_err());
    }

    #[test]
    fn test_validate_filename_url_scheme() {
        assert!(validate_filename("https://evil.com/malware.rpm").is_err());
    }
}
