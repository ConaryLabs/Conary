// src/packages/arch.rs

//! Arch Linux package format parser
//!
//! Parses .pkg.tar.zst and .pkg.tar.xz packages, extracting metadata from .PKGINFO

use crate::db::models::{Trove, TroveType};
use crate::error::{Error, Result};
use crate::packages::traits::{Dependency, DependencyType, ExtractedFile, PackageFile, PackageFormat};
use flate2::read::GzDecoder;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use tar::Archive;
use tracing::debug;
use xz2::read::XzDecoder;

/// Arch Linux package representation
pub struct ArchPackage {
    package_path: PathBuf,
    name: String,
    version: String,
    architecture: Option<String>,
    description: Option<String>,
    files: Vec<PackageFile>,
    dependencies: Vec<Dependency>,
    // Additional Arch-specific metadata
    url: Option<String>,
    licenses: Vec<String>,
    groups: Vec<String>,
    packager: Option<String>,
    build_date: Option<String>,
}

impl ArchPackage {
    /// Detect compression format from file extension
    fn detect_compression(path: &str) -> Result<CompressionFormat> {
        if path.ends_with(".pkg.tar.zst") {
            Ok(CompressionFormat::Zstd)
        } else if path.ends_with(".pkg.tar.xz") {
            Ok(CompressionFormat::Xz)
        } else if path.ends_with(".pkg.tar.gz") {
            Ok(CompressionFormat::Gzip)
        } else {
            Err(Error::InitError(format!(
                "Unsupported Arch package format: {}. Expected .pkg.tar.zst, .pkg.tar.xz, or .pkg.tar.gz",
                path
            )))
        }
    }

    /// Open and decompress the package archive
    fn open_archive(path: &str) -> Result<Archive<Box<dyn Read>>> {
        let file = File::open(path)
            .map_err(|e| Error::InitError(format!("Failed to open package file: {}", e)))?;

        let compression = Self::detect_compression(path)?;

        let reader: Box<dyn Read> = match compression {
            CompressionFormat::Zstd => {
                let decoder = zstd::Decoder::new(file)
                    .map_err(|e| Error::InitError(format!("Failed to create zstd decoder: {}", e)))?;
                Box::new(decoder)
            }
            CompressionFormat::Xz => {
                let decoder = XzDecoder::new(file);
                Box::new(decoder)
            }
            CompressionFormat::Gzip => {
                let decoder = GzDecoder::new(file);
                Box::new(decoder)
            }
        };

        Ok(Archive::new(reader))
    }

    /// Parse .PKGINFO file content
    fn parse_pkginfo(content: &str) -> Result<PkgInfo> {
        let mut info = PkgInfo::default();

        for line in content.lines() {
            let line = line.trim();

            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Parse key = value pairs
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();

                match key {
                    "pkgname" => info.name = Some(value.to_string()),
                    "pkgver" => info.version = Some(value.to_string()),
                    "pkgdesc" => info.description = Some(value.to_string()),
                    "url" => info.url = Some(value.to_string()),
                    "builddate" => info.build_date = Some(value.to_string()),
                    "packager" => info.packager = Some(value.to_string()),
                    "size" => info.size = value.parse().ok(),
                    "arch" => info.architecture = Some(value.to_string()),
                    "license" => info.licenses.push(value.to_string()),
                    "group" => info.groups.push(value.to_string()),
                    "depend" => info.dependencies.push(value.to_string()),
                    "optdepend" => info.optional_deps.push(value.to_string()),
                    "makedepend" => info.make_deps.push(value.to_string()),
                    _ => {} // Ignore unknown keys
                }
            }
        }

        Ok(info)
    }

    /// Extract file list from tar archive
    fn extract_file_list(path: &str) -> Result<Vec<PackageFile>> {
        let mut archive = Self::open_archive(path)?;
        let mut files = Vec::new();

        for entry in archive.entries()
            .map_err(|e| Error::InitError(format!("Failed to read archive entries: {}", e)))?
        {
            let entry = entry
                .map_err(|e| Error::InitError(format!("Failed to read archive entry: {}", e)))?;

            let entry_path = entry.path()
                .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                .to_string_lossy()
                .to_string();

            // Skip .PKGINFO, .MTREE, .BUILDINFO, and .INSTALL files
            if entry_path == ".PKGINFO"
                || entry_path == ".MTREE"
                || entry_path == ".BUILDINFO"
                || entry_path == ".INSTALL" {
                continue;
            }

            // Skip directories
            if entry.header().entry_type().is_dir() {
                continue;
            }

            let size = entry.header().size()
                .map_err(|e| Error::InitError(format!("Failed to get file size: {}", e)))?;

            let mode = entry.header().mode()
                .map_err(|e| Error::InitError(format!("Failed to get file mode: {}", e)))?;

            files.push(PackageFile {
                path: format!("/{}", entry_path), // Ensure absolute path
                size: size as i64,
                mode: mode as i32,
                sha256: None, // We'll compute this during extraction if needed
            });
        }

        Ok(files)
    }

    /// Parse dependencies from strings like "glibc>=2.34" or "package: description"
    fn parse_dependencies(deps: &[String], dep_type: DependencyType) -> Vec<Dependency> {
        deps.iter()
            .map(|dep| {
                // For optional dependencies, format is "package: description"
                let (name, description) = if dep_type == DependencyType::Optional {
                    if let Some((pkg, desc)) = dep.split_once(':') {
                        (pkg.trim(), Some(desc.trim().to_string()))
                    } else {
                        (dep.as_str(), None)
                    }
                } else {
                    (dep.as_str(), None)
                };

                // Parse version constraint (e.g., "glibc>=2.34")
                let (pkg_name, version) = if let Some(pos) = name.find(['>', '<', '=']) {
                    let (n, v) = name.split_at(pos);
                    (n.trim(), Some(v.trim().to_string()))
                } else {
                    (name, None)
                };

                Dependency {
                    name: pkg_name.to_string(),
                    version,
                    dep_type,
                    description,
                }
            })
            .collect()
    }
}

/// Package compression format
enum CompressionFormat {
    Zstd,
    Xz,
    Gzip,
}

/// Parsed .PKGINFO metadata
#[derive(Default)]
struct PkgInfo {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    url: Option<String>,
    architecture: Option<String>,
    build_date: Option<String>,
    packager: Option<String>,
    size: Option<u64>,
    licenses: Vec<String>,
    groups: Vec<String>,
    dependencies: Vec<String>,
    optional_deps: Vec<String>,
    make_deps: Vec<String>,
}

impl PackageFormat for ArchPackage {
    fn parse(path: &str) -> Result<Self> {
        debug!("Parsing Arch package: {}", path);

        // Open archive and find .PKGINFO
        let mut archive = Self::open_archive(path)?;
        let mut pkginfo_content = None;

        for entry in archive.entries()
            .map_err(|e| Error::InitError(format!("Failed to read archive: {}", e)))?
        {
            let mut entry = entry
                .map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;

            let entry_path = entry.path()
                .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                .to_string_lossy()
                .to_string();

            if entry_path == ".PKGINFO" {
                let mut content = String::new();
                entry.read_to_string(&mut content)
                    .map_err(|e| Error::InitError(format!("Failed to read .PKGINFO: {}", e)))?;
                pkginfo_content = Some(content);
                break;
            }
        }

        let pkginfo_content = pkginfo_content
            .ok_or_else(|| Error::InitError("No .PKGINFO file found in package".to_string()))?;

        // Parse .PKGINFO
        let pkginfo = Self::parse_pkginfo(&pkginfo_content)?;

        let name = pkginfo.name
            .ok_or_else(|| Error::InitError("Package name not found in .PKGINFO".to_string()))?;

        let version = pkginfo.version
            .ok_or_else(|| Error::InitError("Package version not found in .PKGINFO".to_string()))?;

        // Extract file list
        let files = Self::extract_file_list(path)?;

        // Parse dependencies
        let mut dependencies = Vec::new();
        dependencies.extend(Self::parse_dependencies(&pkginfo.dependencies, DependencyType::Runtime));
        dependencies.extend(Self::parse_dependencies(&pkginfo.optional_deps, DependencyType::Optional));
        dependencies.extend(Self::parse_dependencies(&pkginfo.make_deps, DependencyType::Build));

        debug!(
            "Parsed Arch package: {} version {} ({} files, {} dependencies)",
            name,
            version,
            files.len(),
            dependencies.len()
        );

        Ok(Self {
            package_path: PathBuf::from(path),
            name,
            version,
            architecture: pkginfo.architecture,
            description: pkginfo.description,
            files,
            dependencies,
            url: pkginfo.url,
            licenses: pkginfo.licenses,
            groups: pkginfo.groups,
            packager: pkginfo.packager,
            build_date: pkginfo.build_date,
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn architecture(&self) -> Option<&str> {
        self.architecture.as_deref()
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn files(&self) -> &[PackageFile] {
        &self.files
    }

    fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>> {
        debug!("Extracting file contents from Arch package: {:?}", self.package_path);

        let path_str = self.package_path.to_str()
            .ok_or_else(|| Error::InitError("Package path contains invalid UTF-8".to_string()))?;
        let mut archive = Self::open_archive(path_str)?;
        let mut extracted_files = Vec::new();

        for entry in archive.entries()
            .map_err(|e| Error::InitError(format!("Failed to read archive: {}", e)))?
        {
            let mut entry = entry
                .map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;

            let entry_path = entry.path()
                .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                .to_string_lossy()
                .to_string();

            // Skip metadata files
            if entry_path == ".PKGINFO"
                || entry_path == ".MTREE"
                || entry_path == ".BUILDINFO"
                || entry_path == ".INSTALL" {
                continue;
            }

            // Skip directories
            if entry.header().entry_type().is_dir() {
                continue;
            }

            let size = entry.header().size()
                .map_err(|e| Error::InitError(format!("Failed to get file size: {}", e)))?;

            let mode = entry.header().mode()
                .map_err(|e| Error::InitError(format!("Failed to get file mode: {}", e)))?;

            // Read file content
            let mut content = Vec::new();
            entry.read_to_end(&mut content)
                .map_err(|e| Error::InitError(format!("Failed to read file content: {}", e)))?;

            // Compute SHA-256
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&content);
            let hash = format!("{:x}", hasher.finalize());

            extracted_files.push(ExtractedFile {
                path: format!("/{}", entry_path), // Ensure absolute path
                content,
                size: size as i64,
                mode: mode as i32,
                sha256: Some(hash),
            });
        }

        debug!("Extracted {} files from Arch package", extracted_files.len());
        Ok(extracted_files)
    }

    fn to_trove(&self) -> Trove {
        let mut trove = Trove::new(
            self.name().to_string(),
            self.version().to_string(),
            TroveType::Package,
        );

        trove.architecture = self.architecture().map(|s| s.to_string());
        trove.description = self.description().map(|s| s.to_string());

        trove
    }
}

impl ArchPackage {
    /// Get upstream URL
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Get package licenses
    pub fn licenses(&self) -> &[String] {
        &self.licenses
    }

    /// Get package groups
    pub fn groups(&self) -> &[String] {
        &self.groups
    }

    /// Get packager information
    pub fn packager(&self) -> Option<&str> {
        self.packager.as_deref()
    }

    /// Get build date
    pub fn build_date(&self) -> Option<&str> {
        self.build_date.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arch_package_structure() {
        // Verify the struct is properly defined
        assert!(std::mem::size_of::<ArchPackage>() > 0);
    }

    #[test]
    fn test_package_format_trait_implemented() {
        // Verify ArchPackage implements PackageFormat trait
        fn assert_implements_package_format<T: PackageFormat>() {}
        assert_implements_package_format::<ArchPackage>();
    }

    #[test]
    fn test_compression_detection() {
        assert!(matches!(
            ArchPackage::detect_compression("test.pkg.tar.zst"),
            Ok(CompressionFormat::Zstd)
        ));
        assert!(matches!(
            ArchPackage::detect_compression("test.pkg.tar.xz"),
            Ok(CompressionFormat::Xz)
        ));
        assert!(matches!(
            ArchPackage::detect_compression("test.pkg.tar.gz"),
            Ok(CompressionFormat::Gzip)
        ));
        assert!(ArchPackage::detect_compression("test.rpm").is_err());
    }

    #[test]
    fn test_pkginfo_parsing() {
        let content = r#"
# Sample .PKGINFO
pkgname = test-package
pkgver = 1.0.0-1
pkgdesc = A test package
url = https://example.com
arch = x86_64
license = MIT
license = Apache
depend = glibc>=2.34
depend = zlib
optdepend = python: for scripts
makedepend = gcc
"#;

        let info = ArchPackage::parse_pkginfo(content).unwrap();
        assert_eq!(info.name, Some("test-package".to_string()));
        assert_eq!(info.version, Some("1.0.0-1".to_string()));
        assert_eq!(info.description, Some("A test package".to_string()));
        assert_eq!(info.architecture, Some("x86_64".to_string()));
        assert_eq!(info.licenses.len(), 2);
        assert_eq!(info.dependencies.len(), 2);
        assert_eq!(info.optional_deps.len(), 1);
        assert_eq!(info.make_deps.len(), 1);
    }

    #[test]
    fn test_dependency_parsing() {
        let deps = vec![
            "glibc>=2.34".to_string(),
            "zlib".to_string(),
        ];

        let parsed = ArchPackage::parse_dependencies(&deps, DependencyType::Runtime);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "glibc");
        assert_eq!(parsed[0].version, Some(">=2.34".to_string()));
        assert_eq!(parsed[1].name, "zlib");
        assert_eq!(parsed[1].version, None);
    }

    #[test]
    fn test_optional_dependency_parsing() {
        let deps = vec![
            "python: for running scripts".to_string(),
            "ruby".to_string(),
        ];

        let parsed = ArchPackage::parse_dependencies(&deps, DependencyType::Optional);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "python");
        assert_eq!(parsed[0].description, Some("for running scripts".to_string()));
        assert_eq!(parsed[1].name, "ruby");
        assert_eq!(parsed[1].description, None);
    }
}
