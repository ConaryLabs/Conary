// conary-core/src/packages/registry.rs

//! Package format registry and structural identification
//!
//! Provides centralized identification and parsing from package-owned format
//! markers. File names and extensions are never format authority.

use crate::ccs::CCS_BUDGET;
use crate::compression::{self, CompressionFormat};
use crate::error::{Error, Result};
use crate::packages::traits::PackageFormat;
use crate::packages::{arch, deb, eopkg, rpm};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Supported package formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormatType {
    Rpm,
    Deb,
    Arch,
    Eopkg,
}

impl PackageFormatType {
    /// Get a human-readable name for the format
    pub fn name(&self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Arch => "arch",
            Self::Eopkg => "eopkg",
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        self.name()
    }
}

/// Identify a package format from exact on-disk format markers.
pub fn detect_format(path: impl AsRef<Path>) -> Result<PackageFormatType> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|e| {
        Error::InitError(format!(
            "Failed to open package file {}: {}",
            path.display(),
            e
        ))
    })?;

    let mut magic = [0_u8; 8];
    let bytes_read = file.read(&mut magic)?;
    if bytes_read >= 4 && magic[0..4] == [0xED, 0xAB, 0xEE, 0xDB] {
        return Ok(PackageFormatType::Rpm);
    }
    if bytes_read >= 8 && &magic == b"!<arch>\n" && has_debian_binary_member(path)? {
        return Ok(PackageFormatType::Deb);
    }

    let compression = CompressionFormat::from_magic_bytes(&magic[..bytes_read]);
    if compression != CompressionFormat::None && has_arch_pkginfo(path, compression)? {
        return Ok(PackageFormatType::Arch);
    }
    if bytes_read >= 4
        && matches!(
            &magic[..4],
            [b'P', b'K', 3, 4] | [b'P', b'K', 5, 6] | [b'P', b'K', 7, 8]
        )
        && eopkg::has_package_contract(path)?
    {
        return Ok(PackageFormatType::Eopkg);
    }

    Err(Error::InitError(format!(
        "file does not contain a supported RPM, Debian, ALPM, or eopkg package contract: {}",
        path.display()
    )))
}

fn has_debian_binary_member(path: &Path) -> Result<bool> {
    let bounds = CCS_BUDGET.archive_decode_bounds()?;
    let file = File::open(path)?;
    let mut archive = ar::Archive::new(file);
    let mut entries_seen = 0_u64;
    while let Some(entry) = archive.next_entry() {
        entries_seen += 1;
        bounds.admit_archive_entry("Debian package entries", entries_seen)?;
        let mut entry = entry?;
        let identifier = String::from_utf8_lossy(entry.header().identifier());
        if identifier.trim_end_matches('/') != "debian-binary" {
            continue;
        }
        if entry.header().size() > 16 {
            return Ok(false);
        }
        let mut version = Vec::new();
        entry.read_to_end(&mut version)?;
        return Ok(version == b"2.0\n");
    }
    Ok(false)
}

fn has_arch_pkginfo(path: &Path, format: CompressionFormat) -> Result<bool> {
    let bounds = CCS_BUDGET.archive_decode_bounds()?;
    let file = File::open(path)?;
    let reader =
        compression::create_decoder_limited(file, format, bounds.max_decompressed_stream_bytes)
            .map_err(|error| Error::InitError(error.to_string()))?;
    let mut archive = tar::Archive::new(reader);
    let mut entries_seen = 0_u64;
    let mut declared_payload_bytes = 0_u64;
    for entry in archive.entries()? {
        entries_seen += 1;
        bounds.admit_archive_entry("ALPM package entries", entries_seen)?;
        let entry = entry?;
        declared_payload_bytes = bounds.add_payload_bytes(
            "ALPM package declared bytes",
            declared_payload_bytes,
            entry.size(),
        )?;
        let path = entry.path()?;
        if path == Path::new(".PKGINFO") || path == Path::new("./.PKGINFO") {
            return Ok(entry.header().entry_type().is_file());
        }
    }
    Ok(false)
}

/// Parse a package file into a boxed PackageFormat implementation
pub fn parse_package(path: impl AsRef<Path>) -> Result<Box<dyn PackageFormat + Send>> {
    let path = path.as_ref();
    let path_str = path
        .to_str()
        .ok_or_else(|| Error::InitError("Package path contains invalid UTF-8".to_string()))?;

    match detect_format(path)? {
        PackageFormatType::Rpm => Ok(Box::new(rpm::RpmPackage::parse(path_str)?)),
        PackageFormatType::Deb => Ok(Box::new(deb::DebPackage::parse(path_str)?)),
        PackageFormatType::Arch => Ok(Box::new(arch::ArchPackage::parse(path_str)?)),
        PackageFormatType::Eopkg => Ok(Box::new(eopkg::EopkgPackage::parse(path_str)?)),
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
        file.write_all(&[0xED, 0xAB, 0xEE, 0xDB, 0x00, 0x00])
            .unwrap();
        assert_eq!(detect_format(file.path()).unwrap(), PackageFormatType::Rpm);
    }

    #[test]
    fn ar_magic_without_debian_contract_is_rejected() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"!<arch>\ncontrol.tar.gz").unwrap();
        assert!(detect_format(file.path()).is_err());
    }

    #[test]
    fn misleading_extensions_are_not_format_authority() {
        let mut file = NamedTempFile::with_suffix(".rpm").unwrap();
        file.write_all(b"not an rpm").unwrap();
        assert!(detect_format(file.path()).is_err());
    }
}
