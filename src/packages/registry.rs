// src/packages/registry.rs

//! Package format registry and detection
//!
//! Provides centralized detection and parsing of various package formats
//! using both magic bytes and file extensions.

use crate::error::{Error, Result};
use crate::packages::traits::PackageFormat;
use crate::packages::{rpm, deb, arch};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Supported package formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormatType {
    Rpm,
    Deb,
    Arch,
}

impl PackageFormatType {
    /// Get a human-readable name for the format
    pub fn name(&self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Arch => "arch",
        }
    }
}

/// Detect the format of a package file
///
/// Uses magic bytes for reliable detection, falling back to file extensions.
pub fn detect_format(path: impl AsRef<Path>) -> Result<PackageFormatType> {
    let path = path.as_ref();
    
    // Try magic bytes first
    if let Ok(mut file) = File::open(path) {
        let mut magic = [0u8; 8];
        if let Ok(n) = file.read(&mut magic) {
            if n >= 4 && magic[0..4] == [0xED, 0xAB, 0xEE, 0xDB] {
                return Ok(PackageFormatType::Rpm);
            }
            if n >= 7 && magic[0..7] == *b"!<arch>" {
                return Ok(PackageFormatType::Deb);
            }

            // Arch packages are tarballs, check for common compression magic
            // Zstd: 28 b5 2f fd
            if n >= 4 && magic[0..4] == [0x28, 0xB5, 0x2F, 0xFD] && is_arch_extension(path) {
                return Ok(PackageFormatType::Arch);
            }
            // XZ: fd 37 7a 58 5a 00
            if n >= 6 && magic[0..6] == [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00] && is_arch_extension(path) {
                return Ok(PackageFormatType::Arch);
            }
            // Gzip: 1f 8b
            if n >= 2 && magic[0..2] == [0x1F, 0x8B] && is_arch_extension(path) {
                return Ok(PackageFormatType::Arch);
            }
        }
    }

    // Fallback to extensions
    let path_str = path.to_string_lossy().to_lowercase();
    if path_str.ends_with(".rpm") {
        Ok(PackageFormatType::Rpm)
    } else if path_str.ends_with(".deb") {
        Ok(PackageFormatType::Deb)
    } else if is_arch_extension(path) {
        Ok(PackageFormatType::Arch)
    } else {
        Err(Error::InitError(format!(
            "Unknown package format for file: {}",
            path.display()
        )))
    }
}

/// Check if a path has an Arch Linux package extension
fn is_arch_extension(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.ends_with(".pkg.tar.zst") || s.ends_with(".pkg.tar.xz") || s.ends_with(".pkg.tar.gz")
}

/// Parse a package file into a boxed PackageFormat implementation
pub fn parse_package(path: impl AsRef<Path>) -> Result<Box<dyn PackageFormat + Send>> {
    let path = path.as_ref();
    let path_str = path.to_str() 
        .ok_or_else(|| Error::InitError("Package path contains invalid UTF-8".to_string()))?;
        
    match detect_format(path)? {
        PackageFormatType::Rpm => Ok(Box::new(rpm::RpmPackage::parse(path_str)?)),
        PackageFormatType::Deb => Ok(Box::new(deb::DebPackage::parse(path_str)?)),
        PackageFormatType::Arch => Ok(Box::new(arch::ArchPackage::parse(path_str)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_detect_rpm_magic() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&[0xED, 0xAB, 0xEE, 0xDB, 0x00, 0x00]).unwrap();
        assert_eq!(detect_format(file.path()).unwrap(), PackageFormatType::Rpm);
    }

    #[test]
    fn test_detect_deb_magic() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"!<arch>\ncontrol.tar.gz").unwrap();
        assert_eq!(detect_format(file.path()).unwrap(), PackageFormatType::Deb);
    }

    #[test]
    fn test_detect_arch_extension() {
        assert!(is_arch_extension(Path::new("test.pkg.tar.zst")));
        assert!(is_arch_extension(Path::new("test.pkg.tar.xz")));
        assert!(!is_arch_extension(Path::new("test.rpm")));
    }
}
