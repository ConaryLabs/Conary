// conary-core/src/packages/deb.rs

//! Debian package format parser
//!
//! Parses .deb packages, which are AR archives containing control and data tarballs

use crate::compression::{self, CompressionFormat};
use crate::db::models::Trove;
use crate::error::{Error, Result};
use crate::hash;
use crate::packages::archive_utils::{check_file_size, normalize_path};
use crate::packages::common::PackageMetadata;
use crate::packages::traits::{
    ConfigFileInfo, Dependency, DependencyType, ExtractedFile, PackageFile, PackageFormat,
    Scriptlet, ScriptletPhase,
};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use tar::Archive;
use tracing::debug;

const CONTROL_TAR_NAMES: &[&str] = &[
    "control.tar.gz",
    "control.tar.xz",
    "control.tar.zst",
    "control.tar",
];

const DATA_TAR_NAMES: &[&str] = &["data.tar.gz", "data.tar.xz", "data.tar.zst", "data.tar"];

/// Results of single-pass control tarball extraction
#[derive(Default)]
struct ControlTarContents {
    /// Raw text of the control file
    control_text: Option<String>,
    /// Maintainer scripts extracted from preinst/postinst/prerm/postrm
    scriptlets: Vec<Scriptlet>,
    /// Config file paths extracted from conffiles
    config_files: Vec<ConfigFileInfo>,
}

/// Debian package representation
pub struct DebPackage {
    /// Common package metadata
    meta: PackageMetadata,
    // Debian-specific metadata
    maintainer: Option<String>,
    section: Option<String>,
    priority: Option<String>,
    homepage: Option<String>,
    installed_size: Option<u64>,
}

impl DebPackage {
    /// Create a decompressor for tar data using magic byte detection
    fn create_tar_decoder<'a>(tar_data: &'a [u8]) -> Result<Box<dyn Read + 'a>> {
        let format = CompressionFormat::from_magic_bytes(tar_data);
        compression::create_decoder(tar_data, format)
            .map_err(|e| Error::InitError(format!("Failed to create decoder: {}", e)))
    }

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
            "Architecture" => {
                info.architecture = Some(
                    crate::packages::common::normalize_architecture(value).to_string(),
                )
            }
            "Description" => {
                // Description is the short description (first line)
                info.description = Some(value.lines().next().unwrap_or(value).to_string())
            }
            "Maintainer" => info.maintainer = Some(value.to_string()),
            "Section" => info.section = Some(value.to_string()),
            "Priority" => info.priority = Some(value.to_string()),
            "Homepage" => info.homepage = Some(value.to_string()),
            "Installed-Size" => info.installed_size = value.parse().ok(),
            "Epoch" => info.epoch = value.parse().ok(),
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

    /// Single-pass extraction of control and data tarballs from the AR archive.
    fn extract_ar_members(path: &str) -> Result<(Vec<u8>, Vec<u8>)> {
        let file = File::open(path)
            .map_err(|e| Error::InitError(format!("Failed to open DEB file: {}", e)))?;
        let mut archive = ar::Archive::new(file);
        let mut control_data: Option<Vec<u8>> = None;
        let mut data_data: Option<Vec<u8>> = None;
        while let Some(entry) = archive.next_entry() {
            let mut entry =
                entry.map_err(|e| Error::InitError(format!("Failed to read AR entry: {}", e)))?;
            let entry_name = String::from_utf8_lossy(entry.header().identifier()).to_string();
            let trimmed = entry_name.trim_end_matches('/');
            if control_data.is_none() && CONTROL_TAR_NAMES.contains(&trimmed) {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)
                    .map_err(|e| Error::InitError(format!("Failed to read control tar: {}", e)))?;
                control_data = Some(buf);
            } else if data_data.is_none() && DATA_TAR_NAMES.contains(&trimmed) {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)
                    .map_err(|e| Error::InitError(format!("Failed to read data tar: {}", e)))?;
                data_data = Some(buf);
            }
            if control_data.is_some() && data_data.is_some() {
                break;
            }
        }
        let control = control_data.ok_or_else(|| {
            Error::InitError("control.tar not found in DEB archive".to_string())
        })?;
        let data = data_data.ok_or_else(|| {
            Error::InitError("data.tar not found in DEB archive".to_string())
        })?;
        Ok((control, data))
    }

    /// Single-pass extraction of control text, scriptlets, and conffiles from the control tarball.
    ///
    /// Replaces three separate functions that each decompressed and iterated the
    /// control tarball independently. One decompression, one iteration.
    fn parse_control_tar_all(control_data: &[u8]) -> Result<ControlTarContents> {
        let reader = Self::create_tar_decoder(control_data)?;
        let mut archive = Archive::new(reader);
        let mut contents = ControlTarContents::default();

        for entry in archive
            .entries()
            .map_err(|e| Error::InitError(format!("Failed to read control.tar: {}", e)))?
        {
            let mut entry =
                entry.map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;
            let entry_path = entry
                .path()
                .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                .to_string_lossy()
                .to_string();
            let basename = entry_path.trim_start_matches("./");

            match basename {
                "control" => {
                    let mut text = String::new();
                    entry.read_to_string(&mut text).map_err(|e| {
                        Error::InitError(format!("Failed to read control file: {}", e))
                    })?;
                    contents.control_text = Some(text);
                }
                "conffiles" => {
                    let mut text = String::new();
                    if entry.read_to_string(&mut text).is_ok() {
                        contents.config_files = text
                            .lines()
                            .filter(|line| !line.is_empty() && line.starts_with('/'))
                            .map(|line| ConfigFileInfo {
                                path: line.trim().to_string(),
                                noreplace: true,
                                ghost: false,
                            })
                            .collect();
                    }
                }
                "preinst" | "postinst" | "prerm" | "postrm" => {
                    let phase = match basename {
                        "preinst" => ScriptletPhase::PreInstall,
                        "postinst" => ScriptletPhase::PostInstall,
                        "prerm" => ScriptletPhase::PreRemove,
                        "postrm" => ScriptletPhase::PostRemove,
                        _ => unreachable!(),
                    };
                    let mut script_content = String::new();
                    if entry.read_to_string(&mut script_content).is_ok()
                        && !script_content.is_empty()
                    {
                        let interpreter = script_content
                            .lines()
                            .next()
                            .and_then(|line| line.strip_prefix("#!"))
                            .map(|s| s.trim().to_string())
                            .unwrap_or_else(|| "/bin/sh".to_string());
                        contents.scriptlets.push(Scriptlet {
                            phase,
                            interpreter,
                            content: script_content,
                            flags: None,
                        });
                    }
                }
                _ => {}
            }
        }

        if contents.control_text.is_none() {
            return Err(Error::InitError(
                "control file not found in control.tar".to_string(),
            ));
        }

        Ok(contents)
    }

    /// Parse the data tarball to extract the file list.
    fn parse_data_tar(data_tar_data: &[u8]) -> Result<Vec<PackageFile>> {
        let reader = Self::create_tar_decoder(data_tar_data)?;
        let mut archive = Archive::new(reader);
        let mut files = Vec::new();
        for entry in archive
            .entries()
            .map_err(|e| Error::InitError(format!("Failed to read data.tar: {}", e)))?
        {
            let entry =
                entry.map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;
            if entry.header().entry_type().is_dir() {
                continue;
            }
            let entry_path = entry
                .path()
                .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                .to_string_lossy()
                .to_string();
            let size = entry
                .header()
                .size()
                .map_err(|e| Error::InitError(format!("Failed to get file size: {}", e)))?;
            let mode = entry
                .header()
                .mode()
                .map_err(|e| Error::InitError(format!("Failed to get file mode: {}", e)))?;
            files.push(PackageFile {
                path: normalize_path(&entry_path)
                    .map_err(|e| Error::InitError(format!("Path normalization failed: {}", e)))?,
                size: i64::try_from(size).unwrap_or(i64::MAX),
                mode: mode as i32,
                sha256: None,
            });
        }
        Ok(files)
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
    epoch: Option<u32>,
}

impl PackageFormat for DebPackage {
    fn parse(path: &str) -> Result<Self> {
        debug!("Parsing Debian package: {}", path);

        // Extract and parse control file
        let (control_data, data_tar_data) = Self::extract_ar_members(path)?;

        // Single-pass extraction of control text, scriptlets, and conffiles
        let control_tar = Self::parse_control_tar_all(&control_data)?;
        let control = Self::parse_control(control_tar.control_text.as_deref().unwrap_or(""))?;

        let name = control.name.ok_or_else(|| {
            Error::InitError("Package name not found in control file".to_string())
        })?;

        let mut version = control.version.ok_or_else(|| {
            Error::InitError("Package version not found in control file".to_string())
        })?;

        // Prepend epoch if present (e.g., "2:1.0.0-1")
        if let Some(epoch) = control.epoch {
            version = format!("{epoch}:{version}");
        }

        // Extract file list
        let files = Self::parse_data_tar(&data_tar_data)?;

        // Convert dependencies
        let mut dependencies = Vec::new();
        dependencies.extend(Self::convert_dependencies(
            &control.dependencies,
            DependencyType::Runtime,
        ));
        dependencies.extend(Self::convert_dependencies(
            &control.recommends,
            DependencyType::Optional,
        ));
        dependencies.extend(Self::convert_dependencies(
            &control.suggests,
            DependencyType::Optional,
        ));
        dependencies.extend(Self::convert_dependencies(
            &control.build_depends,
            DependencyType::Build,
        ));

        let scriptlets = control_tar.scriptlets;
        let config_files = control_tar.config_files;

        debug!(
            "Parsed DEB package: {} version {} ({} files, {} dependencies, {} scriptlets, {} config files)",
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
            architecture: control.architecture,
            description: control.description,
            files,
            dependencies,
            scriptlets,
            config_files,
        };

        Ok(Self {
            meta,
            maintainer: control.maintainer,
            section: control.section,
            priority: control.priority,
            homepage: control.homepage,
            installed_size: control.installed_size,
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
            "Extracting file contents from Debian package: {:?}",
            self.meta.package_path()
        );

        let path_str =
            self.meta.package_path().to_str().ok_or_else(|| {
                Error::InitError("Package path contains invalid UTF-8".to_string())
            })?;
        // Use single-pass AR extraction, then extract files from data tar
        let (_control_data, data_tar_data) = Self::extract_ar_members(path_str)?;

        let reader = Self::create_tar_decoder(&data_tar_data)?;
        let mut archive = Archive::new(reader);
        let mut extracted_files = Vec::new();

        for entry in archive
            .entries()
            .map_err(|e| Error::InitError(format!("Failed to read data.tar: {}", e)))?
        {
            let mut entry = entry
                .map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;

            let entry_path = entry
                .path()
                .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                .to_string_lossy()
                .to_string();

            // Skip directories
            if entry.header().entry_type().is_dir() {
                continue;
            }

            let size = entry
                .header()
                .size()
                .map_err(|e| Error::InitError(format!("Failed to get file size: {}", e)))?;

            // Check file size using shared utility
            if !check_file_size(&entry_path, size) {
                continue;
            }

            let mode = entry
                .header()
                .mode()
                .map_err(|e| Error::InitError(format!("Failed to get file mode: {}", e)))?;

            // Read file content
            let mut content = Vec::new();
            entry.read_to_end(&mut content).map_err(|e| {
                Error::InitError(format!("Failed to read file content: {}", e))
            })?;

            // Compute SHA-256 using shared utility
            let hash = hash::sha256(&content);

            extracted_files.push(ExtractedFile {
                path: normalize_path(&entry_path).map_err(|e| {
                    Error::InitError(format!("Path normalization failed: {}", e))
                })?,
                content,
                size: i64::try_from(size).unwrap_or(i64::MAX),
                mode: mode as i32,
                sha256: Some(hash),
            });
        }

        debug!("Extracted {} files from DEB package", extracted_files.len());
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
        assert_eq!(
            control.maintainer,
            Some("Test User <test@example.com>".to_string())
        );
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
