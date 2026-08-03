// conary-core/src/packages/deb.rs

//! Debian package format parser
//!
//! Parses .deb packages, which are AR archives containing control and data tarballs

use crate::ccs::{ArchiveDecodeBounds, CCS_BUDGET};
use crate::compression::{self, CompressionFormat};
use crate::db::models::Trove;
use crate::error::{Error, Result};
use crate::packages::deb::authority::{
    DebianConfigDeclaration, DebianDeclaredCapability, DebianPackageAuthority,
};
use crate::packages::payload::{
    PAYLOAD_IO_BUFFER_SIZE, PackagePayload, PayloadSpool, ReopenablePayload,
};
use crate::packages::source_authority::SourcePackageAuthority;
use crate::packages::traits::{
    DebControlMember, DebMaintainerInvocation, DebMaintainerMode, DebNativeScriptletMetadata,
    NativeArgumentContract, NativeArgumentValue, NativeInvocationContract, NativeLifecyclePath,
    NativeRootExpectation, NativeScriptletBody, NativeScriptletEntry, NativeScriptletFormat,
    NativeScriptletKind, NativeScriptletMetadata, NativeScriptletSupport, NativeStdinContract,
    NativeTransactionOrder, NativeTransactionPosition, PackageFile, PackageFormat,
};
use crate::repository::dependency_model::{
    DebianMultiArch, RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::package_relation::parse_native_relation;
use crate::repository::versioning::VersionScheme;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Write};
use tar::Archive;
use tracing::debug;

pub mod authority;
pub mod debconf;
pub mod dpkg_lifecycle;
pub mod lifecycle_helpers;
mod native;
mod payload;
pub mod templates;
mod triggers;

pub(crate) use triggers::parse as parse_trigger_declarations;

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
    /// Byte-preserving native scriptlet ABI entries extracted from control.tar
    native_scriptlet_abi: Vec<NativeScriptletEntry>,
    /// Config file paths extracted from conffiles
    config: Vec<DebianConfigDeclaration>,
    /// Whether the package-level debconf templates member was already seen.
    templates_seen: bool,
}

fn copy_declared_entry(
    reader: &mut impl Read,
    writer: &mut impl Write,
    declared: u64,
    label: &str,
) -> Result<()> {
    let mut limited = reader.take(declared.saturating_add(1));
    let mut actual = 0_u64;
    let mut buffer = [0_u8; PAYLOAD_IO_BUFFER_SIZE];
    loop {
        let read = limited
            .read(&mut buffer)
            .map_err(|error| Error::ParseError(format!("read {label}: {error}")))?;
        if read == 0 {
            break;
        }
        actual = actual
            .checked_add(read as u64)
            .ok_or_else(|| Error::ParseError(format!("{label} size overflows u64")))?;
        if actual > declared {
            return Err(Error::ParseError(format!(
                "{label} declares {declared} bytes but yields at least {actual}"
            )));
        }
        writer.write_all(&buffer[..read])?;
    }
    if actual != declared {
        return Err(Error::ParseError(format!(
            "{label} declares {declared} bytes but yields {actual}"
        )));
    }
    Ok(())
}

fn read_declared_entry(reader: &mut impl Read, declared: u64, label: &str) -> Result<Vec<u8>> {
    let mut content = Vec::with_capacity(
        usize::try_from(declared)
            .map_err(|_| Error::ParseError(format!("{label} exceeds addressable memory")))?,
    );
    copy_declared_entry(reader, &mut content, declared, label)?;
    Ok(content)
}

/// Debian package representation
pub struct DebPackage {
    authority: DebianPackageAuthority,
    description: Option<String>,
    files: Vec<PackageFile>,
    requirements: Vec<RepositoryRequirementGroup>,
    relations: Vec<RepositoryRequirementGroup>,
    native_scriptlet_abi: Vec<NativeScriptletEntry>,
    // Debian-specific metadata
    maintainer: Option<String>,
    section: Option<String>,
    priority: Option<String>,
    homepage: Option<String>,
    installed_size: Option<u64>,
    /// File-backed payload parsed from the data tar member.
    payload: PackagePayload,
}

impl DebPackage {
    /// Parse the current `deb-conffiles(5)` grammar without treating flags as
    /// part of the pathname. `remove-on-upgrade` is a transaction operation,
    /// and the referenced path must be absent from the incoming payload.
    /// Create a decompressor for tar data using magic byte detection
    fn create_tar_decoder<'a>(
        tar_data: &'a [u8],
        bounds: &ArchiveDecodeBounds,
    ) -> Result<Box<dyn Read + 'a>> {
        let format = CompressionFormat::from_magic_bytes(tar_data);
        compression::create_decoder_limited(tar_data, format, bounds.max_metadata_bytes)
            .map_err(|e| Error::InitError(format!("Failed to create decoder: {}", e)))
    }

    /// Parse control file from control.tar archive
    fn parse_control(control_content: &str) -> Result<ControlInfo> {
        let mut info = ControlInfo::default();

        let mut current_field = String::new();
        let mut current_value = String::new();
        let mut seen_fields = BTreeSet::new();
        let mut paragraph_ended = false;

        for (index, line) in control_content.lines().enumerate() {
            let line_number = index + 1;
            if line.is_empty() {
                if !current_field.is_empty() {
                    Self::apply_control_field(&mut info, &current_field, &current_value)?;
                    current_field.clear();
                    current_value.clear();
                }
                paragraph_ended = true;
                continue;
            }
            if paragraph_ended {
                return Err(Error::ParseError(format!(
                    "Debian binary control metadata contains a second paragraph at line {line_number}"
                )));
            }

            // Multi-line fields start with a space
            if line.starts_with(' ') || line.starts_with('\t') {
                if current_field.is_empty() {
                    return Err(Error::ParseError(format!(
                        "Debian control line {line_number} is an orphan continuation"
                    )));
                }
                current_value.push('\n');
                current_value.push_str(line.trim());
                continue;
            }

            let (field, value) = line.split_once(':').ok_or_else(|| {
                Error::ParseError(format!(
                    "Debian control line {line_number} is not a field or continuation"
                ))
            })?;
            if field.is_empty()
                || !field.bytes().enumerate().all(|(position, byte)| {
                    byte.is_ascii_alphanumeric() || (position > 0 && byte == b'-')
                })
            {
                return Err(Error::ParseError(format!(
                    "Debian control line {line_number} has invalid field name {field:?}"
                )));
            }
            let canonical_field = field.to_ascii_lowercase();
            if !seen_fields.insert(canonical_field) {
                return Err(Error::ParseError(format!(
                    "Debian control field {field:?} is repeated at line {line_number}"
                )));
            }

            if !current_field.is_empty() {
                Self::apply_control_field(&mut info, &current_field, &current_value)?;
            }
            current_field = field.to_string();
            current_value = value.trim().to_string();
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
            "Architecture" => info.architecture = Some(value.to_string()),
            "Multi-Arch" => {
                info.multi_arch = DebianMultiArch::parse_exact(value).map_err(Error::ParseError)?;
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
            "Depends" => info.dependencies = Self::parse_dependency_list(value)?,
            "Pre-Depends" => info.pre_dependencies = Self::parse_dependency_list(value)?,
            "Recommends" => info.recommends = Self::parse_dependency_list(value)?,
            "Suggests" => info.suggests = Self::parse_dependency_list(value)?,
            "Build-Depends" => info.build_depends = Self::parse_dependency_list(value)?,
            "Provides" => info.provides = Self::parse_dependency_list(value)?,
            "Conflicts" => info.conflicts = Self::parse_dependency_list(value)?,
            "Breaks" => info.breaks = Self::parse_dependency_list(value)?,
            "Replaces" => info.replaces = Self::parse_dependency_list(value)?,
            _ => {} // Ignore unknown fields
        }
        Ok(())
    }

    /// Parse Debian dependency list (comma-separated with optional version constraints)
    fn parse_dependency_list(deps: &str) -> Result<Vec<String>> {
        deps.split(',')
            .enumerate()
            .map(|(index, dependency)| {
                let dependency = dependency.trim();
                if dependency.is_empty() {
                    return Err(Error::ParseError(format!(
                        "Debian dependency list contains an empty atom at position {}",
                        index + 1
                    )));
                }
                Ok(dependency.to_string())
            })
            .collect()
    }

    /// Single-pass extraction of control and data tarballs from the AR archive.
    fn extract_ar_members(path: &str) -> Result<(Vec<u8>, ReopenablePayload)> {
        let bounds = CCS_BUDGET.archive_decode_bounds()?;
        let file = File::open(path)
            .map_err(|e| Error::InitError(format!("Failed to open DEB file: {}", e)))?;
        let mut archive = ar::Archive::new(file);
        let mut control_data: Option<Vec<u8>> = None;
        let mut data_source = None;
        let mut entries_seen = 0_u64;
        while let Some(entry) = archive.next_entry() {
            entries_seen += 1;
            bounds.admit_archive_entry("DEB archive entries", entries_seen)?;
            let mut entry =
                entry.map_err(|e| Error::InitError(format!("Failed to read AR entry: {}", e)))?;
            let entry_name = String::from_utf8_lossy(entry.header().identifier()).to_string();
            let trimmed = entry_name.trim_end_matches('/');
            if control_data.is_none() && CONTROL_TAR_NAMES.contains(&trimmed) {
                let entry_size = entry.header().size();
                bounds.admit_metadata_bytes("DEB compressed control member", entry_size)?;
                let mut buf = Vec::with_capacity(usize::try_from(entry_size).map_err(|_| {
                    Error::ParseError(
                        "DEB compressed control member exceeds addressable memory".to_string(),
                    )
                })?);
                copy_declared_entry(&mut entry, &mut buf, entry_size, "DEB control member")?;
                control_data = Some(buf);
            } else if data_source.is_none() && DATA_TAR_NAMES.contains(&trimmed) {
                let entry_size = entry.header().size();
                bounds.admit_payload_object("DEB compressed data member", entry_size)?;
                let spool = PayloadSpool::new(entry_size)?;
                let path = spool.indexed_path(0);
                let mut output = File::create(&path)?;
                copy_declared_entry(&mut entry, &mut output, entry_size, "DEB data member")?;
                output.sync_all()?;
                data_source = Some(spool.source(path));
            }
            if control_data.is_some() && data_source.is_some() {
                break;
            }
        }
        let control = control_data
            .ok_or_else(|| Error::InitError("control.tar not found in DEB archive".to_string()))?;
        let data = data_source
            .ok_or_else(|| Error::InitError("data.tar not found in DEB archive".to_string()))?;
        Ok((control, data))
    }

    /// Single-pass extraction of control text, scriptlets, and conffiles from the control tarball.
    ///
    /// Replaces three separate functions that each decompressed and iterated the
    /// control tarball independently. One decompression, one iteration.
    fn parse_control_tar_all(control_data: &[u8]) -> Result<ControlTarContents> {
        let bounds = CCS_BUDGET.archive_decode_bounds()?;
        let reader = Self::create_tar_decoder(control_data, &bounds)?;
        let mut archive = Archive::new(reader);
        let mut contents = ControlTarContents::default();
        let mut entries_seen = 0_u64;
        let mut metadata_bytes = 0_u64;

        for entry in archive
            .entries()
            .map_err(|e| Error::InitError(format!("Failed to read control.tar: {}", e)))?
        {
            entries_seen += 1;
            bounds.admit_archive_entry("DEB control.tar entries", entries_seen)?;
            let mut entry =
                entry.map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;
            metadata_bytes = bounds.add_metadata_bytes(
                "DEB control.tar metadata",
                metadata_bytes,
                entry.size(),
            )?;
            let entry_path = entry
                .path()
                .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                .to_string_lossy()
                .to_string();
            let basename = entry_path.trim_start_matches("./");

            match basename {
                "control" => {
                    let declared = entry.size();
                    let body = read_declared_entry(&mut entry, declared, "DEB control file")?;
                    let text = String::from_utf8(body).map_err(|error| {
                        Error::ParseError(format!("DEB control file is not UTF-8: {error}"))
                    })?;
                    contents.control_text = Some(text);
                }
                "conffiles" => {
                    let declared = entry.size();
                    let body = read_declared_entry(&mut entry, declared, "DEBIAN/conffiles")?;
                    let text = String::from_utf8(body).map_err(|error| {
                        Error::ParseError(format!("DEBIAN/conffiles is not valid UTF-8: {error}"))
                    })?;
                    contents.config = authority::parse_config_declarations(&text)?;
                }
                "config" | "preinst" | "postinst" | "prerm" | "postrm" => {
                    let declared = entry.size();
                    let body = read_declared_entry(&mut entry, declared, "DEB maintainer script")?;
                    if let Some(native) = Self::native_abi_from_control_member(basename, &body)? {
                        contents.native_scriptlet_abi.push(native);
                    }
                }
                "triggers" => {
                    let declared = entry.size();
                    let body = read_declared_entry(&mut entry, declared, "DEB triggers file")?;
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
                    let declared = entry.size();
                    let body = read_declared_entry(&mut entry, declared, "DEBIAN/templates")?;
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
    fn parse_data_tar(data_tar: &ReopenablePayload) -> Result<PackagePayload> {
        payload::parse_package_payload(data_tar)
    }

    fn convert_declared_capability_records(
        entries: &[String],
    ) -> Result<Vec<DebianDeclaredCapability>> {
        let mut provides = Vec::new();
        for (record_index, entry) in entries.iter().enumerate() {
            let native = crate::repository::package_relation::parse_debian_provide(entry)
                .map_err(Error::ParseError)?;
            let provide = DebianDeclaredCapability {
                control_index: u32::try_from(record_index).map_err(|_| {
                    Error::ParseError("Debian provide record index exceeds u32".to_string())
                })?,
                kind: native.kind,
                name: native.name,
                version: native.version,
                version_relation: native.version_relation,
                architecture_qualifier: native.architecture_qualifier,
            };
            provides.push(provide);
        }
        Ok(provides)
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
    multi_arch: DebianMultiArch,
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

        let source_version = control.version.ok_or_else(|| {
            Error::InitError("Package version not found in control file".to_string())
        })?;

        // Prepend epoch if present (e.g., "2:1.0.0-1")
        let version = control.epoch.map_or_else(
            || source_version.clone(),
            |epoch| format!("{epoch}:{source_version}"),
        );

        // Extract file list
        let payload = Self::parse_data_tar(&data_tar_data)?;
        let files: Vec<PackageFile> = payload
            .files()
            .iter()
            .map(|file| PackageFile {
                path: file.path.clone(),
                node: file.node.clone(),
                content: file.content_authority.clone(),
            })
            .collect();

        let provides = Self::convert_declared_capability_records(&control.provides)?;
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
        let config = control_tar.config;

        debug!(
            "Parsed DEB package: {} version {} ({} files, {} dependencies, {} native lifecycle entries, {} config files)",
            name,
            version,
            files.len(),
            requirements.len(),
            native_scriptlet_abi.len(),
            config.len()
        );

        let authority = DebianPackageAuthority {
            name,
            epoch: control.epoch,
            source_version,
            version,
            architecture: control.architecture.clone().ok_or_else(|| {
                Error::InitError("Package architecture not found in control file".to_string())
            })?,
            multi_arch: control.multi_arch,
            provides,
            config,
        };

        Ok(Self {
            authority,
            description: control.description,
            files,
            requirements,
            relations,
            native_scriptlet_abi,
            maintainer: control.maintainer,
            section: control.section,
            priority: control.priority,
            homepage: control.homepage,
            installed_size: control.installed_size,
            payload,
        })
    }

    fn name(&self) -> &str {
        &self.authority.name
    }

    fn version(&self) -> &str {
        &self.authority.version
    }

    fn version_scheme(&self) -> VersionScheme {
        VersionScheme::Debian
    }

    fn architecture(&self) -> Option<&str> {
        Some(&self.authority.architecture)
    }

    fn debian_multi_arch(&self) -> Option<DebianMultiArch> {
        Some(self.authority.multi_arch)
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn files(&self) -> &[PackageFile] {
        &self.files
    }

    fn requirements(&self) -> &[RepositoryRequirementGroup] {
        &self.requirements
    }

    fn resolution_capabilities(
        &self,
    ) -> Result<Vec<crate::repository::dependency_model::ProvidedCapability>> {
        SourcePackageAuthority::Debian(self.authority.clone()).resolution_capabilities()
    }

    fn source_authority(&self) -> Result<SourcePackageAuthority> {
        Ok(SourcePackageAuthority::Debian(self.authority.clone()))
    }

    fn relations(&self) -> &[RepositoryRequirementGroup] {
        &self.relations
    }

    fn package_payload(&self) -> Result<PackagePayload> {
        Ok(self.payload.clone())
    }

    fn to_trove(&self) -> Trove {
        SourcePackageAuthority::Debian(self.authority.clone()).to_trove(self.description.clone())
    }

    fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
        &self.native_scriptlet_abi
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

    /// Exact Debian `Multi-Arch` behavior from the package control archive.
    pub fn multi_arch(&self) -> DebianMultiArch {
        self.authority.multi_arch
    }
}

#[cfg(test)]
#[path = "deb/tests.rs"]
mod tests;
