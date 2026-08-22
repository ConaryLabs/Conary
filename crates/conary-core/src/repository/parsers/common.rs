// conary-core/src/repository/parsers/common.rs

//! Shared parser helpers for repository metadata parsers.
//!
//! Eliminates duplication across the Arch, Debian, and Fedora parsers for
//! common operations like download URL construction and path validation.

use std::io::{Read, Seek};
use std::path::Path;

use crate::error::{Error, Result as ConaryResult};

/// Maximum allowed package size (5 GB).
///
/// Shared across all parsers to reject unreasonably large packages.
pub const MAX_PACKAGE_SIZE: u64 = 5 * 1024 * 1024 * 1024;

/// A decoder guard whose ceiling comes from authenticated metadata.
pub(super) struct AuthenticatedLengthReader<R> {
    inner: R,
    read: u64,
    ceiling: u64,
    label: String,
}

impl<R: Read> AuthenticatedLengthReader<R> {
    pub(super) fn new(inner: R, ceiling: u64, label: impl Into<String>) -> Self {
        Self {
            inner,
            read: 0,
            ceiling,
            label: label.into(),
        }
    }

    pub(super) const fn read_bytes(&self) -> u64 {
        self.read
    }
}

impl<R: Read> Read for AuthenticatedLengthReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.read = self.read.saturating_add(read as u64);
        if self.read > self.ceiling {
            return Err(std::io::Error::other(format!(
                "{} decodes to more than the {} bytes its authenticated metadata declares",
                self.label, self.ceiling
            )));
        }
        Ok(read)
    }
}

/// Open a metadata file through a magic-byte-selected streaming decoder.
pub(super) fn open_metadata_decoder(path: &Path, label: &str) -> ConaryResult<Box<dyn Read>> {
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0_u8; 6];
    let read = file.read(&mut magic)?;
    file.rewind()?;
    let format = crate::compression::CompressionFormat::from_magic_bytes(&magic[..read]);
    crate::compression::create_decoder(file, format)
        .map_err(|error| Error::ParseError(format!("failed to decode {label}: {error}")))
}

/// Whether a file's magic names a compression format Conary decodes.
pub(super) fn metadata_file_is_compressed(path: &Path) -> ConaryResult<bool> {
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0_u8; 6];
    let read = file.read(&mut magic)?;
    Ok(
        crate::compression::CompressionFormat::from_magic_bytes(&magic[..read])
            != crate::compression::CompressionFormat::None,
    )
}

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
