// src/packages/rpm.rs

//! RPM package format parser

use crate::db::models::Trove;
use crate::error::{Error, Result};
use crate::packages::common::{PackageMetadata, MAX_EXTRACTION_FILE_SIZE};
use crate::packages::cpio::CpioReader;
use crate::compression::{self, CompressionFormat};
use crate::packages::traits::{
    ConfigFileInfo, Dependency, DependencyType, ExtractedFile, PackageFile, PackageFormat,
    Scriptlet, ScriptletPhase,
};
use rpm::Package;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use tracing::{debug, warn};

/// RPM package representation
pub struct RpmPackage {
    /// Common package metadata
    meta: PackageMetadata,
    // RPM-specific provenance information
    source_rpm: Option<String>,
    build_host: Option<String>,
    vendor: Option<String>,
    license: Option<String>,
    url: Option<String>,
}

impl RpmPackage {
    /// Extract scriptlets from RPM package using metadata
    fn extract_scriptlets(pkg: &Package) -> Vec<Scriptlet> {
        let mut scriptlets = Vec::new();

        // Helper to add scriptlet
        let mut add_scriptlet = |phase: ScriptletPhase, result: std::result::Result<rpm::Scriptlet, rpm::Error>| {
            if let Ok(s) = result {
                let content = s.script; // Accessing field directly
                if !content.is_empty() {
                    let interpreter = if let Some(progs) = s.program {
                        progs.first().cloned().unwrap_or_else(|| "/bin/sh".to_string())
                    } else {
                        "/bin/sh".to_string()
                    };
                    
                    scriptlets.push(Scriptlet {
                        phase,
                        interpreter,
                        content,
                        flags: None,
                    });
                }
            }
        };

        add_scriptlet(ScriptletPhase::PreInstall, pkg.metadata.get_pre_install_script());
        add_scriptlet(ScriptletPhase::PostInstall, pkg.metadata.get_post_install_script());
        add_scriptlet(ScriptletPhase::PreRemove, pkg.metadata.get_pre_uninstall_script());
        add_scriptlet(ScriptletPhase::PostRemove, pkg.metadata.get_post_uninstall_script());
        add_scriptlet(ScriptletPhase::PreTransaction, pkg.metadata.get_pre_trans_script());
        add_scriptlet(ScriptletPhase::PostTransaction, pkg.metadata.get_post_trans_script());

        scriptlets
    }

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

    /// Extract config files from RPM package using metadata
    fn extract_config_files(pkg: &Package) -> Vec<ConfigFileInfo> {
        use rpm::FileFlags;
        let mut config_files = Vec::new();

        if let Ok(file_entries) = pkg.metadata.get_file_entries() {
            for entry in file_entries {
                if entry.flags.contains(FileFlags::CONFIG) {
                    config_files.push(ConfigFileInfo {
                        path: entry.path.to_string_lossy().to_string(),
                        noreplace: entry.flags.contains(FileFlags::NOREPLACE),
                        ghost: entry.flags.contains(FileFlags::GHOST),
                    });
                }
            }
        }

        config_files
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

                // Convert DependencyFlags to constraint string with operator
                let version = if !req.version.is_empty() {
                    let operator = flags_to_operator(req.flags);
                    Some(format!("{}{}", operator, req.version))
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

/// Convert RPM DependencyFlags to constraint operator string
fn flags_to_operator(flags: rpm::DependencyFlags) -> &'static str {
    use rpm::DependencyFlags;

    // Check for combined flags first
    if flags.contains(DependencyFlags::LESS) && flags.contains(DependencyFlags::EQUAL) {
        "<= "
    } else if flags.contains(DependencyFlags::GREATER) && flags.contains(DependencyFlags::EQUAL) {
        ">= "
    } else if flags.contains(DependencyFlags::LESS) {
        "< "
    } else if flags.contains(DependencyFlags::GREATER) {
        "> "
    } else if flags.contains(DependencyFlags::EQUAL) {
        "= "
    } else {
        // No comparison flags (ANY) - return empty
        ""
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

        // Extract scriptlets and config files using package metadata
        let scriptlets = Self::extract_scriptlets(&pkg);
        let config_files = Self::extract_config_files(&pkg);

        debug!(
            "Parsed RPM: {} version {} ({} files, {} dependencies, {} scriptlets, {} config files)",
            name,
            version,
            files.len(),
            dependencies.len(),
            scriptlets.len(),
            config_files.len()
        );

        let meta = PackageMetadata {
            package_path: PathBuf::from(path),
            name,
            version,
            architecture,
            description,
            files,
            dependencies,
            scriptlets,
            config_files,
        };

        Ok(Self {
            meta,
            source_rpm,
            build_host,
            vendor,
            license,
            url,
        })
    }

    fn name(&self) -> &str {
        self.meta.name()
    }

    fn version(&self) -> &str {
        self.meta.version()
    }

    fn architecture(&self) -> Option<&str> {
        self.meta.architecture()
    }

    fn description(&self) -> Option<&str> {
        self.meta.description()
    }

    fn files(&self) -> &[PackageFile] {
        self.meta.files()
    }

    fn dependencies(&self) -> &[Dependency] {
        self.meta.dependencies()
    }

    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>> {
        debug!(
            "Extracting file contents from RPM: {:?}",
            self.meta.package_path()
        );

        let file = File::open(self.meta.package_path())
            .map_err(|e| Error::InitError(format!("Failed to open RPM file: {}", e)))?;
        let mut reader = BufReader::new(file);

        // Parse the package - this gives us access to the payload content
        let pkg = Package::parse(&mut reader)
            .map_err(|e| Error::InitError(format!("Failed to parse RPM: {}", e)))?;

        // Get the compressed payload from the Package struct
        let payload = &pkg.content;
        if payload.is_empty() {
            debug!("RPM has empty payload");
            return Ok(Vec::new());
        }

        // Detect compression from payload magic bytes
        let format = CompressionFormat::from_magic_bytes(payload);
        debug!("Detected payload compression: {}", format);

        // Create decompressor from the payload
        let cursor = std::io::Cursor::new(payload.clone());
        let decoder = compression::create_decoder(cursor, format)
            .map_err(|e| Error::InitError(format!("Failed to create decoder: {}", e)))?;

        // Map paths to metadata for O(1) lookup
        let file_map: HashMap<&str, &PackageFile> = self.meta.files.iter()
            .map(|f| (f.path.as_str(), f))
            .collect();

        // Extract CPIO archive
        let mut cpio = CpioReader::new(decoder);
        let mut extracted_files = Vec::new();

        while let Some((entry, content)) = cpio.next_entry().map_err(|e| Error::InitError(format!("CPIO error: {}", e)))? {
            // Check if regular file (S_IFREG = 0o100000)
            if (entry.mode & 0o170000) != 0o100000 {
                continue;
            }

            // Check file size to prevent memory exhaustion
            if entry.size > MAX_EXTRACTION_FILE_SIZE {
                warn!(
                    "Skipping oversized file '{}' ({} bytes) in RPM - exceeds {} byte limit",
                    entry.name, entry.size, MAX_EXTRACTION_FILE_SIZE
                );
                continue;
            }

            // Normalize path: CPIO paths are relative (e.g. "./usr/bin"), RPM metadata is absolute ("/usr/bin")
            let rel_path = entry.name.trim_start_matches('.');
            let abs_path = if rel_path.starts_with('/') {
                rel_path.to_string()
            } else {
                format!("/{}", rel_path)
            };

            // Match with metadata to get SHA256 and confirm it's a tracked file
            if let Some(meta) = file_map.get(abs_path.as_str()) {
                extracted_files.push(ExtractedFile {
                    path: abs_path,
                    content,
                    size: entry.size as i64,
                    mode: entry.mode as i32,
                    sha256: meta.sha256.clone(),
                });
            }
        }

        debug!("Extracted {} files from RPM", extracted_files.len());
        Ok(extracted_files)
    }

    fn to_trove(&self) -> Trove {
        self.meta.to_trove()
    }

    fn scriptlets(&self) -> Vec<Scriptlet> {
        self.meta.scriptlets()
    }

    fn config_files(&self) -> Vec<ConfigFileInfo> {
        self.meta.config_files()
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
    fn test_to_trove_conversion() {
        // Create a minimal RpmPackage for testing
        let rpm = RpmPackage {
            meta: PackageMetadata::new(
                PathBuf::from("/fake/path.rpm"),
                "test-package".to_string(),
                "1.0.0".to_string(),
            ),
            source_rpm: Some("test-package-1.0.0.src.rpm".to_string()),
            build_host: Some("buildhost.example.com".to_string()),
            vendor: Some("Test Vendor".to_string()),
            license: Some("MIT".to_string()),
            url: Some("https://example.com".to_string()),
        };

        let trove = rpm.to_trove();

        assert_eq!(trove.name, "test-package");
        assert_eq!(trove.version, "1.0.0");
    }

    #[test]
    fn test_provenance_accessors() {
        let rpm = RpmPackage {
            meta: PackageMetadata::new(
                PathBuf::from("/fake/test.rpm"),
                "test".to_string(),
                "1.0".to_string(),
            ),
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
}
