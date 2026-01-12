// src/packages/deb.rs

//! Debian package format parser
//!
//! Parses .deb packages, which are AR archives containing control and data tarballs

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

/// Debian package representation
pub struct DebPackage {
    package_path: PathBuf,
    name: String,
    version: String,
    architecture: Option<String>,
    description: Option<String>,
    files: Vec<PackageFile>,
    dependencies: Vec<Dependency>,
    // Additional Debian-specific metadata
    maintainer: Option<String>,
    section: Option<String>,
    priority: Option<String>,
    homepage: Option<String>,
    installed_size: Option<u64>,
}

impl DebPackage {
    /// Parse control file from control.tar archive
    fn parse_control(control_content: &str) -> Result<ControlInfo> {
        let mut info = ControlInfo::default();

        let mut current_field = String::new();
        let mut current_value = String::new();

        for line in control_content.lines() {
            // Multi-line fields start with a space
            if line.starts_with(' ') || line.starts_with('\t') {
                if !current_field.is_empty() {
                    current_value.push('\n');
                    current_value.push_str(line.trim());
                }
            } else if let Some((field, value)) = line.split_once(':') {
                // Save previous field
                if !current_field.is_empty() {
                    Self::apply_control_field(&mut info, &current_field, &current_value);
                }

                // Start new field
                current_field = field.trim().to_string();
                current_value = value.trim().to_string();
            }
        }

        // Save last field
        if !current_field.is_empty() {
            Self::apply_control_field(&mut info, &current_field, &current_value);
        }

        Ok(info)
    }

    /// Apply a parsed control field to ControlInfo
    fn apply_control_field(info: &mut ControlInfo, field: &str, value: &str) {
        match field {
            "Package" => info.name = Some(value.to_string()),
            "Version" => info.version = Some(value.to_string()),
            "Architecture" => info.architecture = Some(value.to_string()),
            "Description" => {
                // Description is the short description (first line)
                info.description = Some(value.lines().next().unwrap_or(value).to_string())
            }
            "Maintainer" => info.maintainer = Some(value.to_string()),
            "Section" => info.section = Some(value.to_string()),
            "Priority" => info.priority = Some(value.to_string()),
            "Homepage" => info.homepage = Some(value.to_string()),
            "Installed-Size" => info.installed_size = value.parse().ok(),
            "Depends" => info.dependencies = Self::parse_dependency_list(value),
            "Recommends" => info.recommends = Self::parse_dependency_list(value),
            "Suggests" => info.suggests = Self::parse_dependency_list(value),
            "Build-Depends" => info.build_depends = Self::parse_dependency_list(value),
            _ => {} // Ignore unknown fields
        }
    }

    /// Parse Debian dependency list (comma-separated with optional version constraints)
    fn parse_dependency_list(deps: &str) -> Vec<String> {
        deps.split(',')
            .map(|dep| dep.trim().to_string())
            .filter(|dep| !dep.is_empty())
            .collect()
    }

    /// Parse a single dependency string into name and version constraint
    fn parse_single_dependency(dep: &str) -> (String, Option<String>) {
        // Handle alternatives (foo | bar)
        let dep = dep.split('|').next().unwrap_or(dep).trim();

        // Parse version constraint: package (>= 1.0) or package (<< 2.0)
        if let Some(start) = dep.find('(')
            && let Some(end) = dep.find(')')
        {
            let name = dep[..start].trim().to_string();
            let constraint = dep[start + 1..end].trim().to_string();
            return (name, Some(constraint));
        }

        (dep.to_string(), None)
    }

    /// Extract file from AR archive by name
    fn extract_ar_file(path: &str, filename: &str) -> Result<Vec<u8>> {
        let file = File::open(path)
            .map_err(|e| Error::InitError(format!("Failed to open DEB file: {}", e)))?;

        let mut archive = ar::Archive::new(file);

        while let Some(entry) = archive.next_entry() {
            let mut entry = entry
                .map_err(|e| Error::InitError(format!("Failed to read AR entry: {}", e)))?;

            let entry_name = String::from_utf8_lossy(entry.header().identifier()).to_string();

            if entry_name.starts_with(filename) {
                let mut content = Vec::new();
                entry
                    .read_to_end(&mut content)
                    .map_err(|e| Error::InitError(format!("Failed to read AR file: {}", e)))?;
                return Ok(content);
            }
        }

        Err(Error::InitError(format!(
            "File {} not found in DEB archive",
            filename
        )))
    }

    /// Decompress and extract control.tar.* to get control file
    fn extract_control_file(path: &str) -> Result<String> {
        // Try different compression formats
        for ext in &["control.tar.gz", "control.tar.xz", "control.tar.zst", "control.tar"] {
            if let Ok(tar_data) = Self::extract_ar_file(path, ext) {
                // Decompress based on extension
                let reader: Box<dyn Read> = if ext.ends_with(".gz") {
                    Box::new(GzDecoder::new(&tar_data[..]))
                } else if ext.ends_with(".xz") {
                    Box::new(XzDecoder::new(&tar_data[..]))
                } else if ext.ends_with(".zst") {
                    Box::new(
                        zstd::Decoder::new(&tar_data[..])
                            .map_err(|e| Error::InitError(format!("Failed to create zstd decoder: {}", e)))?,
                    )
                } else {
                    Box::new(&tar_data[..])
                };

                let mut archive = Archive::new(reader);

                // Find control file in tar
                for entry in archive.entries()
                    .map_err(|e| Error::InitError(format!("Failed to read control.tar: {}", e)))?
                {
                    let mut entry = entry
                        .map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;

                    let entry_path = entry.path()
                        .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                        .to_string_lossy()
                        .to_string();

                    if entry_path == "./control" || entry_path == "control" {
                        let mut content = String::new();
                        entry
                            .read_to_string(&mut content)
                            .map_err(|e| Error::InitError(format!("Failed to read control file: {}", e)))?;
                        return Ok(content);
                    }
                }
            }
        }

        Err(Error::InitError(
            "Could not find or extract control file from DEB package".to_string(),
        ))
    }

    /// Extract file list from data.tar.*
    fn extract_file_list(path: &str) -> Result<Vec<PackageFile>> {
        // Try different compression formats
        for ext in &["data.tar.gz", "data.tar.xz", "data.tar.zst", "data.tar"] {
            if let Ok(tar_data) = Self::extract_ar_file(path, ext) {
                // Decompress based on extension
                let reader: Box<dyn Read> = if ext.ends_with(".gz") {
                    Box::new(GzDecoder::new(&tar_data[..]))
                } else if ext.ends_with(".xz") {
                    Box::new(XzDecoder::new(&tar_data[..]))
                } else if ext.ends_with(".zst") {
                    Box::new(
                        zstd::Decoder::new(&tar_data[..])
                            .map_err(|e| Error::InitError(format!("Failed to create zstd decoder: {}", e)))?,
                    )
                } else {
                    Box::new(&tar_data[..])
                };

                let mut archive = Archive::new(reader);
                let mut files = Vec::new();

                for entry in archive.entries()
                    .map_err(|e| Error::InitError(format!("Failed to read data.tar: {}", e)))?
                {
                    let entry = entry
                        .map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;

                    let entry_path = entry.path()
                        .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                        .to_string_lossy()
                        .to_string();

                    // Skip directories
                    if entry.header().entry_type().is_dir() {
                        continue;
                    }

                    let size = entry.header().size()
                        .map_err(|e| Error::InitError(format!("Failed to get file size: {}", e)))?;

                    let mode = entry.header().mode()
                        .map_err(|e| Error::InitError(format!("Failed to get file mode: {}", e)))?;

                    files.push(PackageFile {
                        path: format!("/{}", entry_path.trim_start_matches("./")),
                        size: size as i64,
                        mode: mode as i32,
                        sha256: None,
                    });
                }

                return Ok(files);
            }
        }

        Err(Error::InitError(
            "Could not find or extract data.tar from DEB package".to_string(),
        ))
    }

    /// Convert dependency list to Dependency structs
    fn convert_dependencies(deps: &[String], dep_type: DependencyType) -> Vec<Dependency> {
        deps.iter()
            .map(|dep| {
                let (name, version) = Self::parse_single_dependency(dep);
                Dependency {
                    name,
                    version,
                    dep_type,
                    description: None,
                }
            })
            .collect()
    }
}

/// Parsed control file metadata
#[derive(Default)]
struct ControlInfo {
    name: Option<String>,
    version: Option<String>,
    architecture: Option<String>,
    description: Option<String>,
    maintainer: Option<String>,
    section: Option<String>,
    priority: Option<String>,
    homepage: Option<String>,
    installed_size: Option<u64>,
    dependencies: Vec<String>,
    recommends: Vec<String>,
    suggests: Vec<String>,
    build_depends: Vec<String>,
}

impl PackageFormat for DebPackage {
    fn parse(path: &str) -> Result<Self> {
        debug!("Parsing Debian package: {}", path);

        // Extract and parse control file
        let control_content = Self::extract_control_file(path)?;
        let control = Self::parse_control(&control_content)?;

        let name = control
            .name
            .ok_or_else(|| Error::InitError("Package name not found in control file".to_string()))?;

        let version = control
            .version
            .ok_or_else(|| Error::InitError("Package version not found in control file".to_string()))?;

        // Extract file list
        let files = Self::extract_file_list(path)?;

        // Convert dependencies
        let mut dependencies = Vec::new();
        dependencies.extend(Self::convert_dependencies(&control.dependencies, DependencyType::Runtime));
        dependencies.extend(Self::convert_dependencies(&control.recommends, DependencyType::Optional));
        dependencies.extend(Self::convert_dependencies(&control.suggests, DependencyType::Optional));
        dependencies.extend(Self::convert_dependencies(&control.build_depends, DependencyType::Build));

        debug!(
            "Parsed DEB package: {} version {} ({} files, {} dependencies)",
            name,
            version,
            files.len(),
            dependencies.len()
        );

        Ok(Self {
            package_path: PathBuf::from(path),
            name,
            version,
            architecture: control.architecture,
            description: control.description,
            files,
            dependencies,
            maintainer: control.maintainer,
            section: control.section,
            priority: control.priority,
            homepage: control.homepage,
            installed_size: control.installed_size,
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
        debug!("Extracting file contents from Debian package: {:?}", self.package_path);

        // Try different compression formats
        for ext in &["data.tar.gz", "data.tar.xz", "data.tar.zst", "data.tar"] {
            let path_str = match self.package_path.to_str() {
                Some(s) => s,
                None => return Err(Error::InitError("Package path contains invalid UTF-8".to_string())),
            };
            if let Ok(tar_data) = Self::extract_ar_file(path_str, ext) {
                // Decompress based on extension
                let reader: Box<dyn Read> = if ext.ends_with(".gz") {
                    Box::new(GzDecoder::new(&tar_data[..]))
                } else if ext.ends_with(".xz") {
                    Box::new(XzDecoder::new(&tar_data[..]))
                } else if ext.ends_with(".zst") {
                    Box::new(
                        zstd::Decoder::new(&tar_data[..])
                            .map_err(|e| Error::InitError(format!("Failed to create zstd decoder: {}", e)))?,
                    )
                } else {
                    Box::new(&tar_data[..])
                };

                let mut archive = Archive::new(reader);
                let mut extracted_files = Vec::new();

                for entry in archive.entries()
                    .map_err(|e| Error::InitError(format!("Failed to read data.tar: {}", e)))?
                {
                    let mut entry = entry
                        .map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;

                    let entry_path = entry.path()
                        .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                        .to_string_lossy()
                        .to_string();

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
                    entry
                        .read_to_end(&mut content)
                        .map_err(|e| Error::InitError(format!("Failed to read file content: {}", e)))?;

                    // Compute SHA-256
                    use sha2::{Digest, Sha256};
                    let mut hasher = Sha256::new();
                    hasher.update(&content);
                    let hash = format!("{:x}", hasher.finalize());

                    extracted_files.push(ExtractedFile {
                        path: format!("/{}", entry_path.trim_start_matches("./")),
                        content,
                        size: size as i64,
                        mode: mode as i32,
                        sha256: Some(hash),
                    });
                }

                debug!("Extracted {} files from DEB package", extracted_files.len());
                return Ok(extracted_files);
            }
        }

        Err(Error::InitError(
            "Could not find or extract data.tar from DEB package".to_string(),
        ))
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

impl DebPackage {
    /// Get package maintainer
    pub fn maintainer(&self) -> Option<&str> {
        self.maintainer.as_deref()
    }

    /// Get package section
    pub fn section(&self) -> Option<&str> {
        self.section.as_deref()
    }

    /// Get package priority
    pub fn priority(&self) -> Option<&str> {
        self.priority.as_deref()
    }

    /// Get homepage URL
    pub fn homepage(&self) -> Option<&str> {
        self.homepage.as_deref()
    }

    /// Get installed size in KB
    pub fn installed_size(&self) -> Option<u64> {
        self.installed_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deb_package_structure() {
        // Verify the struct is properly defined
        assert!(std::mem::size_of::<DebPackage>() > 0);
    }

    #[test]
    fn test_package_format_trait_implemented() {
        // Verify DebPackage implements PackageFormat trait
        fn assert_implements_package_format<T: PackageFormat>() {}
        assert_implements_package_format::<DebPackage>();
    }

    #[test]
    fn test_control_parsing() {
        let content = r#"Package: test-package
Version: 1.0.0-1
Architecture: amd64
Description: A test package
 This is a longer description
 that spans multiple lines.
Maintainer: Test User <test@example.com>
Section: utils
Priority: optional
Homepage: https://example.com
Installed-Size: 1024
Depends: libc6 (>= 2.34), zlib1g
Recommends: python3
"#;

        let control = DebPackage::parse_control(content).unwrap();
        assert_eq!(control.name, Some("test-package".to_string()));
        assert_eq!(control.version, Some("1.0.0-1".to_string()));
        assert_eq!(control.architecture, Some("amd64".to_string()));
        assert_eq!(control.description, Some("A test package".to_string()));
        assert_eq!(control.maintainer, Some("Test User <test@example.com>".to_string()));
        assert_eq!(control.section, Some("utils".to_string()));
        assert_eq!(control.priority, Some("optional".to_string()));
        assert_eq!(control.homepage, Some("https://example.com".to_string()));
        assert_eq!(control.installed_size, Some(1024));
        assert_eq!(control.dependencies.len(), 2);
        assert_eq!(control.recommends.len(), 1);
    }

    #[test]
    fn test_dependency_list_parsing() {
        let deps = "libc6 (>= 2.34), zlib1g, python3 | python2";
        let parsed = DebPackage::parse_dependency_list(deps);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], "libc6 (>= 2.34)");
        assert_eq!(parsed[1], "zlib1g");
        assert_eq!(parsed[2], "python3 | python2");
    }

    #[test]
    fn test_single_dependency_parsing() {
        let (name, version) = DebPackage::parse_single_dependency("libc6 (>= 2.34)");
        assert_eq!(name, "libc6");
        assert_eq!(version, Some(">= 2.34".to_string()));

        let (name, version) = DebPackage::parse_single_dependency("zlib1g");
        assert_eq!(name, "zlib1g");
        assert_eq!(version, None);

        // Test alternatives (should take first option)
        let (name, version) = DebPackage::parse_single_dependency("python3 | python2");
        assert_eq!(name, "python3");
        assert_eq!(version, None);
    }
}
