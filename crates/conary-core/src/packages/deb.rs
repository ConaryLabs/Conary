// conary-core/src/packages/deb.rs

//! Debian package format parser
//!
//! Parses .deb packages, which are AR archives containing control and data tarballs

use crate::compression::{self, CompressionFormat};
use crate::db::models::Trove;
use crate::error::{Error, Result};
use crate::packages::archive_utils::normalize_path;
use crate::packages::common::PackageMetadata;
use crate::packages::traits::{
    ConfigFileInfo, DebControlMember, DebMaintainerInvocation, DebMaintainerMode,
    DebNativeScriptletMetadata, Dependency, DependencyType, ExtractedFile, NativeArgumentContract,
    NativeArgumentValue, NativeInvocationContract, NativeLifecyclePath, NativeRootExpectation,
    NativeScriptletBody, NativeScriptletEntry, NativeScriptletFormat, NativeScriptletKind,
    NativeScriptletMetadata, NativeScriptletSupport, NativeStdinContract, NativeTransactionOrder,
    NativeTransactionPosition, PackageFile, PackageFormat,
};
use crate::repository::dependency_model::{RepositoryRequirementGroup, RepositoryRequirementKind};
use crate::repository::package_relation::parse_native_relation;
use crate::repository::versioning::VersionScheme;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use tar::Archive;
use tracing::debug;

pub mod debconf;
pub mod dpkg_lifecycle;
pub mod lifecycle_helpers;
mod native;
mod payload;
pub(crate) mod templates;
mod triggers;

pub(crate) use triggers::parse as parse_trigger_declarations;

const CONTROL_TAR_NAMES: &[&str] = &[
    "control.tar.gz",
    "control.tar.xz",
    "control.tar.zst",
    "control.tar",
];

const DATA_TAR_NAMES: &[&str] = &["data.tar.gz", "data.tar.xz", "data.tar.zst", "data.tar"];

/// Maximum size for a single AR member within a DEB archive (16 MiB)
const MAX_DEB_MEMBER_SIZE: u64 = 16 * 1024 * 1024;

/// Results of single-pass control tarball extraction
#[derive(Default)]
struct ControlTarContents {
    /// Raw text of the control file
    control_text: Option<String>,
    /// Byte-preserving native scriptlet ABI entries extracted from control.tar
    native_scriptlet_abi: Vec<NativeScriptletEntry>,
    /// Config file paths extracted from conffiles
    config_files: Vec<ConfigFileInfo>,
    /// Whether the package-level debconf templates member was already seen.
    templates_seen: bool,
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
    /// Cached data tarball bytes to avoid re-extracting from the AR archive
    data_tar_cache: Vec<u8>,
}

impl DebPackage {
    /// Parse the current `deb-conffiles(5)` grammar without treating flags as
    /// part of the pathname. `remove-on-upgrade` is a transaction operation,
    /// and the referenced path must be absent from the incoming payload.
    fn parse_conffiles(content: &str) -> Result<Vec<ConfigFileInfo>> {
        if !content.is_empty() && !content.ends_with('\n') {
            return Err(Error::InitError(
                "Invalid DEBIAN/conffiles: final entry is missing a newline".to_string(),
            ));
        }

        content
            .lines()
            .enumerate()
            .map(|(index, raw)| {
                let line_number = index + 1;
                let line = raw.trim_end();
                if line.is_empty() {
                    return Err(Error::InitError(format!(
                        "Invalid DEBIAN/conffiles:{line_number}: empty entries are not allowed"
                    )));
                }

                let (path, remove_on_upgrade) = if line.starts_with('/') {
                    (line, false)
                } else {
                    let split = line
                        .find(char::is_whitespace)
                        .ok_or_else(|| {
                            Error::InitError(format!(
                                "Invalid DEBIAN/conffiles:{line_number}: conffile path is not absolute"
                            ))
                        })?;
                    let flag = &line[..split];
                    let path = line[split..].trim_start();
                    if flag != "remove-on-upgrade" {
                        return Err(Error::InitError(format!(
                            "Invalid DEBIAN/conffiles:{line_number}: unsupported flag '{flag}'"
                        )));
                    }
                    (path, true)
                };

                if !path.starts_with('/') {
                    return Err(Error::InitError(format!(
                        "Invalid DEBIAN/conffiles:{line_number}: conffile path '{path}' is not absolute"
                    )));
                }
                let path = normalize_path(path).map_err(|error| {
                    Error::InitError(format!(
                        "Invalid DEBIAN/conffiles:{line_number}: unsafe conffile path '{path}': {error}"
                    ))
                })?;

                Ok(ConfigFileInfo {
                    path,
                    noreplace: true,
                    ghost: false,
                    remove_on_upgrade,
                })
            })
            .collect()
    }

    /// Create a decompressor for tar data using magic byte detection
    fn create_tar_decoder<'a>(tar_data: &'a [u8]) -> Result<Box<dyn Read + 'a>> {
        let format = CompressionFormat::from_magic_bytes(tar_data);
        compression::create_decoder_limited(tar_data, format, compression::MAX_DECOMPRESS_SIZE)
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
                    Self::apply_control_field(&mut info, &current_field, &current_value)?;
                }

                // Start new field
                current_field = field.trim().to_string();
                current_value = value.trim().to_string();
            }
        }

        // Save last field
        if !current_field.is_empty() {
            Self::apply_control_field(&mut info, &current_field, &current_value)?;
        }

        Ok(info)
    }

    /// Apply a parsed control field to ControlInfo
    fn apply_control_field(info: &mut ControlInfo, field: &str, value: &str) -> Result<()> {
        match field {
            "Package" => info.name = Some(value.to_string()),
            "Version" => info.version = Some(value.to_string()),
            "Architecture" => {
                info.architecture =
                    Some(crate::packages::common::normalize_architecture(value).to_string())
            }
            "Description" => {
                // Description is the short description (first line)
                info.description = Some(value.lines().next().unwrap_or(value).to_string())
            }
            "Maintainer" => info.maintainer = Some(value.to_string()),
            "Section" => info.section = Some(value.to_string()),
            "Priority" => info.priority = Some(value.to_string()),
            "Homepage" => info.homepage = Some(value.to_string()),
            "Installed-Size" => {
                info.installed_size = Some(value.parse().map_err(|error| {
                    Error::ParseError(format!(
                        "Debian control field Installed-Size has invalid value {value:?}: {error}"
                    ))
                })?)
            }
            "Epoch" => {
                info.epoch = Some(value.parse().map_err(|error| {
                    Error::ParseError(format!(
                        "Debian control field Epoch has invalid value {value:?}: {error}"
                    ))
                })?)
            }
            "Depends" => info.dependencies = Self::parse_dependency_list(value),
            "Pre-Depends" => info.pre_dependencies = Self::parse_dependency_list(value),
            "Recommends" => info.recommends = Self::parse_dependency_list(value),
            "Suggests" => info.suggests = Self::parse_dependency_list(value),
            "Build-Depends" => info.build_depends = Self::parse_dependency_list(value),
            "Provides" => info.provides = Self::parse_dependency_list(value),
            "Conflicts" => info.conflicts = Self::parse_dependency_list(value),
            "Breaks" => info.breaks = Self::parse_dependency_list(value),
            "Replaces" => info.replaces = Self::parse_dependency_list(value),
            _ => {} // Ignore unknown fields
        }
        Ok(())
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
        let mut entries_seen = 0usize;
        while let Some(entry) = archive.next_entry() {
            entries_seen += 1;
            compression::check_archive_entry_limit(entries_seen, "DEB archive")
                .map_err(|e| Error::InitError(format!("Failed to read DEB archive: {}", e)))?;
            let mut entry =
                entry.map_err(|e| Error::InitError(format!("Failed to read AR entry: {}", e)))?;
            let entry_name = String::from_utf8_lossy(entry.header().identifier()).to_string();
            let trimmed = entry_name.trim_end_matches('/');
            if control_data.is_none() && CONTROL_TAR_NAMES.contains(&trimmed) {
                let entry_size = entry.header().size();
                if entry_size > MAX_DEB_MEMBER_SIZE {
                    return Err(Error::InitError(format!(
                        "DEB archive member too large: {entry_size} bytes"
                    )));
                }
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .map_err(|e| Error::InitError(format!("Failed to read control tar: {}", e)))?;
                control_data = Some(buf);
            } else if data_data.is_none() && DATA_TAR_NAMES.contains(&trimmed) {
                let entry_size = entry.header().size();
                if entry_size > MAX_DEB_MEMBER_SIZE {
                    return Err(Error::InitError(format!(
                        "DEB archive member too large: {entry_size} bytes"
                    )));
                }
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .map_err(|e| Error::InitError(format!("Failed to read data tar: {}", e)))?;
                data_data = Some(buf);
            }
            if control_data.is_some() && data_data.is_some() {
                break;
            }
        }
        let control = control_data
            .ok_or_else(|| Error::InitError("control.tar not found in DEB archive".to_string()))?;
        let data = data_data
            .ok_or_else(|| Error::InitError("data.tar not found in DEB archive".to_string()))?;
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
        let mut entries_seen = 0usize;

        for entry in archive
            .entries()
            .map_err(|e| Error::InitError(format!("Failed to read control.tar: {}", e)))?
        {
            entries_seen += 1;
            compression::check_archive_entry_limit(entries_seen, "DEB control.tar")
                .map_err(|e| Error::InitError(format!("Failed to read control.tar: {}", e)))?;
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
                    entry.read_to_string(&mut text).map_err(|error| {
                        Error::InitError(format!(
                            "Failed to read DEBIAN/conffiles as UTF-8: {error}"
                        ))
                    })?;
                    contents.config_files = Self::parse_conffiles(&text)?;
                }
                "config" | "preinst" | "postinst" | "prerm" | "postrm" => {
                    let mut body = Vec::new();
                    entry.read_to_end(&mut body).map_err(|e| {
                        Error::InitError(format!("Failed to read maintainer script: {}", e))
                    })?;
                    if let Some(native) = Self::native_abi_from_control_member(basename, &body)? {
                        contents.native_scriptlet_abi.push(native);
                    }
                }
                "triggers" => {
                    let mut body = Vec::new();
                    entry.read_to_end(&mut body).map_err(|e| {
                        Error::InitError(format!("Failed to read triggers file: {}", e))
                    })?;
                    if !body.iter().all(|byte| byte.is_ascii_whitespace()) {
                        contents
                            .native_scriptlet_abi
                            .push(Self::native_abi_from_triggers_file(&body)?);
                    }
                }
                "templates" => {
                    if contents.templates_seen {
                        return Err(Error::ParseError(
                            "Invalid DEBIAN/templates: duplicate control member".to_string(),
                        ));
                    }
                    contents.templates_seen = true;
                    if !entry.header().entry_type().is_file() {
                        return Err(Error::ParseError(
                            "Invalid DEBIAN/templates: control member is not a regular file"
                                .to_string(),
                        ));
                    }
                    let entry_size = entry.size();
                    if entry_size > MAX_DEB_MEMBER_SIZE {
                        return Err(Error::ParseError(format!(
                            "Invalid DEBIAN/templates: control member is too large: {entry_size} bytes"
                        )));
                    }
                    let mut body = Vec::new();
                    entry.read_to_end(&mut body).map_err(|error| {
                        Error::InitError(format!("Failed to read templates file: {error}"))
                    })?;
                    if let Some(templates) = templates::native_abi_entry(body)? {
                        contents.native_scriptlet_abi.push(templates);
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
        payload::parse_package_files(data_tar_data)
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

    fn convert_relations(
        entries: &[String],
        kind: RepositoryRequirementKind,
    ) -> Result<Vec<RepositoryRequirementGroup>> {
        entries
            .iter()
            .map(|entry| {
                parse_native_relation(kind, VersionScheme::Debian, entry).map_err(Error::ParseError)
            })
            .collect()
    }

    fn convert_requirements(
        entries: &[String],
        kind: RepositoryRequirementKind,
    ) -> Result<Vec<RepositoryRequirementGroup>> {
        entries
            .iter()
            .map(|entry| {
                crate::repository::requirement::parse_native_requirement(
                    kind,
                    VersionScheme::Debian,
                    entry,
                )
                .map_err(Error::ParseError)
            })
            .collect()
    }
}

/// Parsed control file metadata
#[derive(Debug, Default)]
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
    pre_dependencies: Vec<String>,
    provides: Vec<String>,
    recommends: Vec<String>,
    suggests: Vec<String>,
    build_depends: Vec<String>,
    conflicts: Vec<String>,
    breaks: Vec<String>,
    replaces: Vec<String>,
    epoch: Option<u32>,
}

impl PackageFormat for DebPackage {
    fn parse(path: &str) -> Result<Self> {
        debug!("Parsing Debian package: {}", path);

        // Extract and parse control file
        let (control_data, data_tar_data) = Self::extract_ar_members(path)?;

        // Single-pass extraction of control text, scriptlets, and conffiles
        let control_tar = Self::parse_control_tar_all(&control_data)?;
        let control_text = control_tar.control_text.as_deref().ok_or_else(|| {
            Error::ParseError("DEB control archive does not contain a control file".to_string())
        })?;
        let control = Self::parse_control(control_text)?;

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

        let provides = Self::convert_dependencies(&control.provides, DependencyType::Runtime);
        let mut requirements =
            Self::convert_requirements(&control.dependencies, RepositoryRequirementKind::Depends)?;
        requirements.extend(Self::convert_requirements(
            &control.pre_dependencies,
            RepositoryRequirementKind::PreDepends,
        )?);
        requirements.extend(Self::convert_requirements(
            &control.recommends,
            RepositoryRequirementKind::Optional,
        )?);
        requirements.extend(Self::convert_requirements(
            &control.suggests,
            RepositoryRequirementKind::Optional,
        )?);
        requirements.extend(Self::convert_requirements(
            &control.build_depends,
            RepositoryRequirementKind::Build,
        )?);
        let mut relations =
            Self::convert_relations(&control.conflicts, RepositoryRequirementKind::Conflict)?;
        relations.extend(Self::convert_relations(
            &control.breaks,
            RepositoryRequirementKind::Breaks,
        )?);
        relations.extend(Self::convert_relations(
            &control.replaces,
            RepositoryRequirementKind::Replace,
        )?);

        let native_scriptlet_abi = control_tar.native_scriptlet_abi;
        let config_files = control_tar.config_files;

        debug!(
            "Parsed DEB package: {} version {} ({} files, {} dependencies, {} native lifecycle entries, {} config files)",
            name,
            version,
            files.len(),
            requirements.len(),
            native_scriptlet_abi.len(),
            config_files.len()
        );

        let meta = PackageMetadata {
            package_path: PathBuf::from(path),
            name,
            version,
            version_scheme: VersionScheme::Debian,
            architecture: control.architecture,
            description: control.description,
            files,
            requirements,
            provides,
            relations,
            diagnostic_scriptlet_evidence: Vec::new(),
            native_scriptlet_abi,
            config_files,
        };

        Ok(Self {
            meta,
            maintainer: control.maintainer,
            section: control.section,
            priority: control.priority,
            homepage: control.homepage,
            installed_size: control.installed_size,
            data_tar_cache: data_tar_data,
        })
    }

    fn name(&self) -> &str {
        self.meta.name()
    }

    fn version(&self) -> &str {
        self.meta.version()
    }

    fn version_scheme(&self) -> VersionScheme {
        self.meta.version_scheme()
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

    fn requirements(&self) -> &[RepositoryRequirementGroup] {
        self.meta.requirements()
    }

    fn provides(&self) -> &[Dependency] {
        self.meta.provides()
    }

    fn relations(&self) -> &[RepositoryRequirementGroup] {
        self.meta.relations()
    }

    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>> {
        debug!(
            "Extracting file contents from Debian package: {:?}",
            self.meta.package_path()
        );

        let extracted_files = payload::extract_files(&self.data_tar_cache)?;

        debug!("Extracted {} files from DEB package", extracted_files.len());
        Ok(extracted_files)
    }

    fn to_trove(&self) -> Trove {
        self.meta.to_trove()
    }

    fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
        self.meta.native_scriptlet_abi()
    }

    fn config_files(&self) -> &[ConfigFileInfo] {
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
#[path = "deb/tests.rs"]
mod tests;
