// src/packages/rpm.rs

//! RPM package format parser

use crate::db::models::{Trove, TroveType};
use crate::error::{Error, Result};
use crate::packages::traits::{Dependency, DependencyType, ExtractedFile, PackageFile, PackageFormat};
use rpm::Package;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use tracing::debug;

/// RPM package representation
pub struct RpmPackage {
    package_path: PathBuf,
    name: String,
    version: String,
    architecture: Option<String>,
    description: Option<String>,
    files: Vec<PackageFile>,
    dependencies: Vec<Dependency>,
    // Provenance information
    source_rpm: Option<String>,
    build_host: Option<String>,
    vendor: Option<String>,
    license: Option<String>,
    url: Option<String>,
}

impl RpmPackage {
    /// Extract file list from RPM package with detailed metadata
    fn extract_files(pkg: &Package) -> Vec<PackageFile> {
        let mut files = Vec::new();

        // Use get_file_entries() to get complete file metadata
        if let Ok(file_entries) = pkg.metadata.get_file_entries() {
            for entry in file_entries {
                // FileDigest can be formatted as hex string
                let sha256 = entry.digest.as_ref().map(|d| format!("{}", d));

                files.push(PackageFile {
                    path: entry.path.to_string_lossy().to_string(),
                    size: entry.size as i64,
                    mode: entry.mode.raw_mode() as i32,
                    sha256,
                });
            }
        }

        files
    }

    /// Extract dependencies from RPM package
    fn extract_dependencies(pkg: &Package) -> Vec<Dependency> {
        let mut deps = Vec::new();

        // Extract runtime dependencies (Requires)
        if let Ok(requires) = pkg.metadata.get_requires() {
            for req in requires {
                // Skip rpmlib dependencies and file paths
                if req.name.starts_with("rpmlib(") || req.name.starts_with('/') {
                    continue;
                }

                let version = if !req.version.is_empty() {
                    Some(req.version.to_string())
                } else {
                    None
                };

                deps.push(Dependency {
                    name: req.name.to_string(),
                    version,
                    dep_type: DependencyType::Runtime,
                    description: None,
                });
            }
        }

        deps
    }
}

impl PackageFormat for RpmPackage {
    fn parse(path: &str) -> Result<Self> {
        debug!("Parsing RPM package: {}", path);

        let file = File::open(path)
            .map_err(|e| Error::InitError(format!("Failed to open RPM file: {}", e)))?;

        let mut buf_reader = BufReader::new(file);

        let pkg = Package::parse(&mut buf_reader)
            .map_err(|e| Error::InitError(format!("Failed to parse RPM: {}", e)))?;

        // Extract basic metadata
        let name = pkg
            .metadata
            .get_name()
            .map_err(|e| Error::InitError(format!("Failed to get package name: {}", e)))?
            .to_string();

        let version = pkg
            .metadata
            .get_version()
            .map_err(|e| Error::InitError(format!("Failed to get package version: {}", e)))?
            .to_string();

        let architecture = pkg.metadata.get_arch().ok().map(|s| s.to_string());
        let description = pkg.metadata.get_description().ok().map(|s| s.to_string());

        // Extract provenance information
        let source_rpm = pkg.metadata.get_source_rpm().ok().map(|s| s.to_string());
        let build_host = pkg.metadata.get_build_host().ok().map(|s| s.to_string());
        let vendor = pkg.metadata.get_vendor().ok().map(|s| s.to_string());
        let license = pkg.metadata.get_license().ok().map(|s| s.to_string());
        let url = pkg.metadata.get_url().ok().map(|s| s.to_string());

        let files = Self::extract_files(&pkg);
        let dependencies = Self::extract_dependencies(&pkg);

        debug!(
            "Parsed RPM: {} version {} ({} files, {} dependencies)",
            name,
            version,
            files.len(),
            dependencies.len()
        );

        Ok(Self {
            package_path: PathBuf::from(path),
            name,
            version,
            architecture,
            description,
            files,
            dependencies,
            source_rpm,
            build_host,
            vendor,
            license,
            url,
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
        use std::process::Command;
        use tempfile::TempDir;

        debug!("Extracting file contents from RPM: {:?}", self.package_path);

        // Create temp directory for extraction
        let temp_dir = TempDir::new()
            .map_err(|e| Error::InitError(format!("Failed to create temp dir: {}", e)))?;

        // Extract RPM to temp directory using rpm2cpio | cpio
        // rpm2cpio package.rpm | cpio -idmv -D /tmp/extract
        let rpm2cpio_output = Command::new("rpm2cpio")
            .arg(&self.package_path)
            .output()
            .map_err(|e| Error::InitError(format!("Failed to run rpm2cpio: {}. Is rpm2cpio installed?", e)))?;

        if !rpm2cpio_output.status.success() {
            return Err(Error::InitError(format!(
                "rpm2cpio failed: {}",
                String::from_utf8_lossy(&rpm2cpio_output.stderr)
            )));
        }

        // Extract cpio archive
        let cpio_status = Command::new("cpio")
            .args(["-idm", "--quiet"])
            .current_dir(temp_dir.path())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin.write_all(&rpm2cpio_output.stdout)?;
                }
                child.wait()
            })
            .map_err(|e| Error::InitError(format!("Failed to run cpio: {}. Is cpio installed?", e)))?;

        if !cpio_status.success() {
            return Err(Error::InitError("cpio extraction failed".to_string()));
        }

        // Read extracted files
        let mut extracted_files = Vec::new();

        for file_meta in &self.files {
            let full_path = temp_dir.path().join(file_meta.path.trim_start_matches('/'));

            // Skip if not a regular file (directory, symlink, etc.)
            if !full_path.is_file() {
                continue;
            }

            // Read file content
            let content = std::fs::read(&full_path)
                .map_err(|e| Error::InitError(format!("Failed to read {}: {}", file_meta.path, e)))?;

            extracted_files.push(ExtractedFile {
                path: file_meta.path.clone(),
                content,
                size: file_meta.size,
                mode: file_meta.mode,
                sha256: file_meta.sha256.clone(),
            });
        }

        debug!("Extracted {} files from RPM", extracted_files.len());
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

impl RpmPackage {
    /// Get source RPM name (for provenance tracking)
    pub fn source_rpm(&self) -> Option<&str> {
        self.source_rpm.as_deref()
    }

    /// Get build host (for provenance tracking)
    pub fn build_host(&self) -> Option<&str> {
        self.build_host.as_deref()
    }

    /// Get vendor information
    pub fn vendor(&self) -> Option<&str> {
        self.vendor.as_deref()
    }

    /// Get license information
    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }

    /// Get upstream URL
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpm_package_structure() {
        // Verify the struct is properly defined
        assert!(std::mem::size_of::<RpmPackage>() > 0);
    }

    #[test]
    fn test_package_format_trait_implemented() {
        // Verify RpmPackage implements PackageFormat trait
        // This test ensures the trait is correctly implemented at compile time
        fn assert_implements_package_format<T: PackageFormat>() {}
        assert_implements_package_format::<RpmPackage>();
    }

    #[test]
    fn test_to_trove_conversion() {
        // Create a minimal RpmPackage for testing
        let rpm = RpmPackage {
            package_path: PathBuf::from("/fake/path.rpm"),
            name: "test-package".to_string(),
            version: "1.0.0".to_string(),
            architecture: Some("x86_64".to_string()),
            description: Some("Test package".to_string()),
            files: vec![],
            dependencies: vec![],
            source_rpm: Some("test-package-1.0.0.src.rpm".to_string()),
            build_host: Some("buildhost.example.com".to_string()),
            vendor: Some("Test Vendor".to_string()),
            license: Some("MIT".to_string()),
            url: Some("https://example.com".to_string()),
        };

        let trove = rpm.to_trove();

        assert_eq!(trove.name, "test-package");
        assert_eq!(trove.version, "1.0.0");
        assert_eq!(trove.architecture, Some("x86_64".to_string()));
        assert_eq!(trove.description, Some("Test package".to_string()));
    }

    #[test]
    fn test_provenance_accessors() {
        let rpm = RpmPackage {
            package_path: PathBuf::from("/fake/test.rpm"),
            name: "test".to_string(),
            version: "1.0".to_string(),
            architecture: None,
            description: None,
            files: vec![],
            dependencies: vec![],
            source_rpm: Some("test-1.0.src.rpm".to_string()),
            build_host: Some("builder".to_string()),
            vendor: Some("Vendor".to_string()),
            license: Some("GPL".to_string()),
            url: Some("https://test.com".to_string()),
        };

        assert_eq!(rpm.source_rpm(), Some("test-1.0.src.rpm"));
        assert_eq!(rpm.build_host(), Some("builder"));
        assert_eq!(rpm.vendor(), Some("Vendor"));
        assert_eq!(rpm.license(), Some("GPL"));
        assert_eq!(rpm.url(), Some("https://test.com"));
    }

    #[test]
    fn test_parse_nonexistent_file() {
        // Test that parsing a nonexistent file returns an error
        let result = RpmPackage::parse("/nonexistent/file.rpm");
        assert!(result.is_err());
    }

    #[test]
    fn test_dependency_type_variants() {
        // Ensure all DependencyType variants are accessible
        let runtime = DependencyType::Runtime;
        let build = DependencyType::Build;
        let optional = DependencyType::Optional;

        assert_eq!(runtime, DependencyType::Runtime);
        assert_eq!(build, DependencyType::Build);
        assert_eq!(optional, DependencyType::Optional);
    }
}
