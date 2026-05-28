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
    ConfigFileInfo, DebControlMember, DebMaintainerInvocation, DebMaintainerMode,
    DebNativeScriptletMetadata, DebTriggerAwaitMode, DebTriggerDeclaration, DebTriggerDirective,
    Dependency, DependencyType, ExtractedFile, NativeArgumentContract, NativeArgumentValue,
    NativeInvocationContract, NativeLifecyclePath, NativeRootExpectation, NativeScriptletBody,
    NativeScriptletEntry, NativeScriptletFormat, NativeScriptletKind, NativeScriptletMetadata,
    NativeScriptletSupport, NativeStdinContract, NativeTransactionOrder, NativeTransactionPosition,
    PackageFile, PackageFormat, Scriptlet, ScriptletPhase, split_shebang,
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

/// Maximum size for a single AR member within a DEB archive (16 MiB)
const MAX_DEB_MEMBER_SIZE: u64 = 16 * 1024 * 1024;

/// Results of single-pass control tarball extraction
#[derive(Default)]
struct ControlTarContents {
    /// Raw text of the control file
    control_text: Option<String>,
    /// Maintainer scripts extracted from preinst/postinst/prerm/postrm
    scriptlets: Vec<Scriptlet>,
    /// Byte-preserving native scriptlet ABI entries extracted from control.tar
    native_scriptlet_abi: Vec<NativeScriptletEntry>,
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
    /// Cached data tarball bytes to avoid re-extracting from the AR archive
    data_tar_cache: Vec<u8>,
}

impl DebPackage {
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
            "Installed-Size" => info.installed_size = value.parse().ok(),
            "Epoch" => info.epoch = value.parse().ok(),
            "Depends" => info.dependencies = Self::parse_dependency_list(value),
            "Recommends" => info.recommends = Self::parse_dependency_list(value),
            "Suggests" => info.suggests = Self::parse_dependency_list(value),
            "Build-Depends" => info.build_depends = Self::parse_dependency_list(value),
            "Provides" => info.provides = Self::parse_dependency_list(value),
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
                "config" | "preinst" | "postinst" | "prerm" | "postrm" => {
                    let mut body = Vec::new();
                    entry.read_to_end(&mut body).map_err(|e| {
                        Error::InitError(format!("Failed to read maintainer script: {}", e))
                    })?;
                    if let Some(native) = Self::native_abi_from_control_member(basename, &body) {
                        contents.native_scriptlet_abi.push(native);
                    }
                    if let Some(flattened) =
                        Self::flattened_scriptlet_from_control_member(basename, &body)
                    {
                        contents.scriptlets.push(flattened);
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
                            .push(Self::native_abi_from_triggers_file(&body));
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
        let mut entries_seen = 0usize;
        for entry in archive
            .entries()
            .map_err(|e| Error::InitError(format!("Failed to read data.tar: {}", e)))?
        {
            entries_seen += 1;
            compression::check_archive_entry_limit(entries_seen, "DEB data.tar")
                .map_err(|e| Error::InitError(format!("Failed to read data.tar: {}", e)))?;
            let entry =
                entry.map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;
            let entry_type = entry.header().entry_type();
            if entry_type.is_dir() {
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
            let symlink_target = if entry_type.is_symlink() {
                entry
                    .link_name()
                    .ok()
                    .flatten()
                    .map(|l| l.to_string_lossy().into_owned())
            } else {
                None
            };
            files.push(PackageFile {
                path: normalize_path(&entry_path)
                    .map_err(|e| Error::InitError(format!("Path normalization failed: {}", e)))?,
                size: i64::try_from(size).unwrap_or(i64::MAX),
                mode: mode as i32,
                sha256: None,
                symlink_target,
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

    fn native_abi_from_control_member(name: &str, body: &[u8]) -> Option<NativeScriptletEntry> {
        let (control_member, lifecycle, compatibility_phase, stdin) = match name {
            "config" => (
                DebControlMember::Config,
                NativeLifecyclePath::Config,
                None,
                NativeStdinContract::Debconf,
            ),
            "preinst" => (
                DebControlMember::Preinst,
                NativeLifecyclePath::PreInstall,
                Some(ScriptletPhase::PreInstall),
                NativeStdinContract::None,
            ),
            "postinst" => (
                DebControlMember::Postinst,
                NativeLifecyclePath::PostInstall,
                Some(ScriptletPhase::PostInstall),
                NativeStdinContract::None,
            ),
            "prerm" => (
                DebControlMember::Prerm,
                NativeLifecyclePath::PreRemove,
                Some(ScriptletPhase::PreRemove),
                NativeStdinContract::None,
            ),
            "postrm" => (
                DebControlMember::Postrm,
                NativeLifecyclePath::PostRemove,
                Some(ScriptletPhase::PostRemove),
                NativeStdinContract::None,
            ),
            _ => return None,
        };

        if body.iter().all(|byte| byte.is_ascii_whitespace()) {
            return None;
        }

        let text = String::from_utf8(body.to_vec()).ok();
        let (interpreter, interpreter_args) = text
            .as_deref()
            .map(split_shebang)
            .unwrap_or((Some("/bin/sh".to_string()), Vec::new()));
        let order_position = match lifecycle {
            NativeLifecyclePath::PreInstall | NativeLifecyclePath::PreRemove => {
                NativeTransactionPosition::BeforePayload
            }
            NativeLifecyclePath::Config => NativeTransactionPosition::ControlArtifact,
            _ => NativeTransactionPosition::AfterPayload,
        };

        Some(NativeScriptletEntry {
            id: format!("deb:{name}"),
            format: NativeScriptletFormat::Deb,
            kind: NativeScriptletKind::Executable,
            native_slot: name.to_string(),
            primary_lifecycle: lifecycle,
            compatibility_phase,
            lifecycle_paths: Self::deb_lifecycle_paths(control_member),
            interpreter,
            interpreter_args,
            body: NativeScriptletBody::from_bytes(body.to_vec()),
            invocation: NativeInvocationContract {
                args: Vec::new(),
                environment: Vec::new(),
                stdin,
                root: NativeRootExpectation::PackageManagerDefault,
            },
            order: NativeTransactionOrder::new(order_position),
            support: NativeScriptletSupport::Parsed,
            metadata: NativeScriptletMetadata::Deb(DebNativeScriptletMetadata {
                control_member,
                maintainer_modes: Self::deb_maintainer_invocations(control_member),
                trigger_declarations: Vec::new(),
            }),
        })
    }

    fn flattened_scriptlet_from_control_member(name: &str, body: &[u8]) -> Option<Scriptlet> {
        let phase = match name {
            "preinst" => ScriptletPhase::PreInstall,
            "postinst" => ScriptletPhase::PostInstall,
            "prerm" => ScriptletPhase::PreRemove,
            "postrm" => ScriptletPhase::PostRemove,
            _ => return None,
        };
        let content = String::from_utf8(body.to_vec()).ok()?;
        if content.is_empty() {
            return None;
        }
        let interpreter = content
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("#!"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "/bin/sh".to_string());

        Some(Scriptlet {
            phase,
            interpreter,
            content,
            flags: None,
        })
    }

    fn deb_lifecycle_paths(control_member: DebControlMember) -> Vec<NativeLifecyclePath> {
        match control_member {
            DebControlMember::Config => vec![NativeLifecyclePath::Config],
            DebControlMember::Preinst => vec![
                NativeLifecyclePath::PreInstall,
                NativeLifecyclePath::PreUpgrade,
                NativeLifecyclePath::Abort,
            ],
            DebControlMember::Postinst => vec![
                NativeLifecyclePath::PostInstall,
                NativeLifecyclePath::PostUpgrade,
                NativeLifecyclePath::Trigger,
                NativeLifecyclePath::Abort,
            ],
            DebControlMember::Prerm => vec![
                NativeLifecyclePath::PreRemove,
                NativeLifecyclePath::PreUpgrade,
                NativeLifecyclePath::Abort,
            ],
            DebControlMember::Postrm => vec![
                NativeLifecyclePath::PostRemove,
                NativeLifecyclePath::PostUpgrade,
                NativeLifecyclePath::Purge,
                NativeLifecyclePath::Abort,
            ],
            DebControlMember::Triggers => vec![NativeLifecyclePath::Trigger],
        }
    }

    fn deb_action_arg() -> NativeArgumentContract {
        NativeArgumentContract {
            index: 1,
            name: "action".to_string(),
            value: NativeArgumentValue::Action,
            required: true,
        }
    }

    fn deb_arg(
        index: usize,
        name: &str,
        value: NativeArgumentValue,
        required: bool,
    ) -> NativeArgumentContract {
        NativeArgumentContract {
            index,
            name: name.to_string(),
            value,
            required,
        }
    }

    fn deb_old_new_args(required: bool) -> Vec<NativeArgumentContract> {
        vec![
            Self::deb_arg(2, "old-version", NativeArgumentValue::OldVersion, required),
            Self::deb_arg(3, "new-version", NativeArgumentValue::NewVersion, required),
        ]
    }

    fn deb_new_version_arg(index: usize, required: bool) -> NativeArgumentContract {
        Self::deb_arg(
            index,
            "new-version",
            NativeArgumentValue::NewVersion,
            required,
        )
    }

    fn deb_installed_version_arg(index: usize, required: bool) -> NativeArgumentContract {
        Self::deb_arg(
            index,
            "installed-version",
            NativeArgumentValue::InstalledVersion,
            required,
        )
    }

    fn deb_marker_arg(index: usize, marker: &str, required: bool) -> NativeArgumentContract {
        Self::deb_arg(
            index,
            marker,
            NativeArgumentValue::Raw(marker.to_string()),
            required,
        )
    }

    fn deb_package_arg(index: usize, name: &str, required: bool) -> NativeArgumentContract {
        Self::deb_arg(index, name, NativeArgumentValue::PackageName, required)
    }

    fn deb_version_arg(
        index: usize,
        name: &str,
        value: NativeArgumentValue,
        required: bool,
    ) -> NativeArgumentContract {
        Self::deb_arg(index, name, value, required)
    }

    fn deb_invocation(
        mode: DebMaintainerMode,
        mut args: Vec<NativeArgumentContract>,
        lifecycle_paths: Vec<NativeLifecyclePath>,
    ) -> DebMaintainerInvocation {
        let mut full_args = vec![Self::deb_action_arg()];
        full_args.append(&mut args);
        DebMaintainerInvocation {
            mode,
            args: full_args,
            lifecycle_paths,
        }
    }

    fn deb_maintainer_invocations(
        control_member: DebControlMember,
    ) -> Vec<DebMaintainerInvocation> {
        match control_member {
            DebControlMember::Config => vec![
                Self::deb_invocation(
                    DebMaintainerMode::Configure,
                    vec![Self::deb_installed_version_arg(2, false)],
                    vec![NativeLifecyclePath::Config],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Reconfigure,
                    vec![Self::deb_installed_version_arg(2, false)],
                    vec![NativeLifecyclePath::Config],
                ),
            ],
            DebControlMember::Preinst => vec![
                Self::deb_invocation(
                    DebMaintainerMode::Install,
                    Self::deb_old_new_args(false),
                    vec![NativeLifecyclePath::PreInstall],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Upgrade,
                    Self::deb_old_new_args(true),
                    vec![NativeLifecyclePath::PreUpgrade],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortUpgrade,
                    vec![Self::deb_new_version_arg(2, true)],
                    vec![NativeLifecyclePath::Abort],
                ),
            ],
            DebControlMember::Postinst => vec![
                Self::deb_invocation(
                    DebMaintainerMode::Configure,
                    vec![Self::deb_arg(
                        2,
                        "most-recently-configured-version",
                        NativeArgumentValue::Raw("most-recently-configured-version".to_string()),
                        false,
                    )],
                    vec![
                        NativeLifecyclePath::PostInstall,
                        NativeLifecyclePath::PostUpgrade,
                    ],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Triggered,
                    vec![Self::deb_arg(
                        2,
                        "trigger-names",
                        NativeArgumentValue::TriggerNames,
                        true,
                    )],
                    vec![NativeLifecyclePath::Trigger],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortUpgrade,
                    vec![Self::deb_new_version_arg(2, true)],
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortRemove,
                    Vec::new(),
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortRemove,
                    vec![
                        Self::deb_marker_arg(2, "in-favour", true),
                        Self::deb_package_arg(3, "package", true),
                        Self::deb_new_version_arg(4, true),
                    ],
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortDeconfigure,
                    vec![
                        Self::deb_marker_arg(2, "in-favour", true),
                        Self::deb_package_arg(3, "failed-install-package", true),
                        Self::deb_version_arg(
                            4,
                            "failed-install-version",
                            NativeArgumentValue::NewVersion,
                            true,
                        ),
                        Self::deb_marker_arg(5, "removing", false),
                        Self::deb_package_arg(6, "conflicting-package", false),
                        Self::deb_version_arg(
                            7,
                            "conflicting-version",
                            NativeArgumentValue::OldVersion,
                            false,
                        ),
                    ],
                    vec![NativeLifecyclePath::Abort],
                ),
            ],
            DebControlMember::Prerm => vec![
                Self::deb_invocation(
                    DebMaintainerMode::Remove,
                    Vec::new(),
                    vec![NativeLifecyclePath::PreRemove],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Remove,
                    vec![
                        Self::deb_marker_arg(2, "in-favour", true),
                        Self::deb_package_arg(3, "package", true),
                        Self::deb_new_version_arg(4, true),
                    ],
                    vec![NativeLifecyclePath::PreRemove],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Upgrade,
                    vec![Self::deb_new_version_arg(2, true)],
                    vec![NativeLifecyclePath::PreUpgrade],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Deconfigure,
                    vec![
                        Self::deb_marker_arg(2, "in-favour", true),
                        Self::deb_package_arg(3, "package-being-installed", true),
                        Self::deb_version_arg(
                            4,
                            "package-being-installed-version",
                            NativeArgumentValue::NewVersion,
                            true,
                        ),
                        Self::deb_marker_arg(5, "removing", false),
                        Self::deb_package_arg(6, "conflicting-package", false),
                        Self::deb_version_arg(
                            7,
                            "conflicting-version",
                            NativeArgumentValue::OldVersion,
                            false,
                        ),
                    ],
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::FailedUpgrade,
                    Self::deb_old_new_args(true),
                    vec![NativeLifecyclePath::Abort],
                ),
            ],
            DebControlMember::Postrm => vec![
                Self::deb_invocation(
                    DebMaintainerMode::Remove,
                    Vec::new(),
                    vec![NativeLifecyclePath::PostRemove],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Purge,
                    Vec::new(),
                    vec![NativeLifecyclePath::Purge],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Upgrade,
                    vec![Self::deb_new_version_arg(2, true)],
                    vec![NativeLifecyclePath::PostUpgrade],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::Disappear,
                    vec![
                        Self::deb_arg(
                            2,
                            "overwriter-package",
                            NativeArgumentValue::PackageName,
                            true,
                        ),
                        Self::deb_arg(
                            3,
                            "overwriter-version",
                            NativeArgumentValue::NewVersion,
                            true,
                        ),
                    ],
                    vec![NativeLifecyclePath::PostRemove],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::FailedUpgrade,
                    Self::deb_old_new_args(true),
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortInstall,
                    Self::deb_old_new_args(false),
                    vec![NativeLifecyclePath::Abort],
                ),
                Self::deb_invocation(
                    DebMaintainerMode::AbortUpgrade,
                    Self::deb_old_new_args(true),
                    vec![NativeLifecyclePath::Abort],
                ),
            ],
            DebControlMember::Triggers => Vec::new(),
        }
    }

    fn native_abi_from_triggers_file(body: &[u8]) -> NativeScriptletEntry {
        let text = String::from_utf8(body.to_vec()).unwrap_or_default();
        let declarations = Self::parse_deb_trigger_declarations(&text);

        NativeScriptletEntry {
            id: "deb:triggers".to_string(),
            format: NativeScriptletFormat::Deb,
            kind: NativeScriptletKind::ControlArtifact,
            native_slot: "triggers".to_string(),
            primary_lifecycle: NativeLifecyclePath::Trigger,
            compatibility_phase: None,
            lifecycle_paths: vec![NativeLifecyclePath::Trigger],
            interpreter: None,
            interpreter_args: Vec::new(),
            body: NativeScriptletBody::from_bytes(body.to_vec()),
            invocation: NativeInvocationContract {
                args: Vec::new(),
                environment: Vec::new(),
                stdin: NativeStdinContract::None,
                root: NativeRootExpectation::PackageManagerDefault,
            },
            order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
            support: NativeScriptletSupport::DeferredReview {
                reason_code: "deb-trigger-semantics-deferred".to_string(),
            },
            metadata: NativeScriptletMetadata::Deb(DebNativeScriptletMetadata {
                control_member: DebControlMember::Triggers,
                maintainer_modes: Vec::new(),
                trigger_declarations: declarations,
            }),
        }
    }

    fn parse_deb_trigger_declarations(text: &str) -> Vec<DebTriggerDeclaration> {
        text.lines()
            .filter_map(|line| {
                let raw_line = line.to_string();
                let line = line.split('#').next().unwrap_or("").trim();
                if line.is_empty() {
                    return None;
                }
                let mut parts = line.split_whitespace();
                let directive = parts.next()?;
                let trigger_name = parts.next()?.to_string();
                let (directive, await_mode) = match directive {
                    "interest" => (DebTriggerDirective::Interest, DebTriggerAwaitMode::Default),
                    "interest-await" => (DebTriggerDirective::Interest, DebTriggerAwaitMode::Await),
                    "interest-noawait" => {
                        (DebTriggerDirective::Interest, DebTriggerAwaitMode::NoAwait)
                    }
                    "activate" => (DebTriggerDirective::Activate, DebTriggerAwaitMode::Default),
                    "activate-await" => (DebTriggerDirective::Activate, DebTriggerAwaitMode::Await),
                    "activate-noawait" => {
                        (DebTriggerDirective::Activate, DebTriggerAwaitMode::NoAwait)
                    }
                    _ => return None,
                };

                Some(DebTriggerDeclaration {
                    directive,
                    trigger_name,
                    await_mode,
                    raw_line,
                })
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
    provides: Vec<String>,
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
        let provides = Self::convert_dependencies(&control.provides, DependencyType::Runtime);

        let scriptlets = control_tar.scriptlets;
        let native_scriptlet_abi = control_tar.native_scriptlet_abi;
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
            provides,
            scriptlets,
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

    fn provides(&self) -> &[Dependency] {
        self.meta.provides()
    }

    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>> {
        debug!(
            "Extracting file contents from Debian package: {:?}",
            self.meta.package_path()
        );

        // Use cached data tarball instead of re-extracting from the AR archive
        let reader = Self::create_tar_decoder(&self.data_tar_cache)?;
        let mut archive = Archive::new(reader);
        let mut extracted_files = Vec::new();
        let mut entries_seen = 0usize;

        for entry in archive
            .entries()
            .map_err(|e| Error::InitError(format!("Failed to read data.tar: {}", e)))?
        {
            entries_seen += 1;
            compression::check_archive_entry_limit(entries_seen, "DEB data.tar")
                .map_err(|e| Error::InitError(format!("Failed to read data.tar: {}", e)))?;
            let mut entry =
                entry.map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;

            let entry_path = entry
                .path()
                .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                .to_string_lossy()
                .to_string();

            let entry_type = entry.header().entry_type();

            // Skip directories
            if entry_type.is_dir() {
                continue;
            }

            let is_symlink = entry_type.is_symlink();

            let size = entry
                .header()
                .size()
                .map_err(|e| Error::InitError(format!("Failed to get file size: {}", e)))?;

            // Check file size using shared utility (symlinks are small)
            if !is_symlink && !check_file_size(&entry_path, size) {
                continue;
            }

            let mode = entry
                .header()
                .mode()
                .map_err(|e| Error::InitError(format!("Failed to get file mode: {}", e)))?;

            let symlink_target = if is_symlink {
                entry
                    .link_name()
                    .ok()
                    .flatten()
                    .map(|l| l.to_string_lossy().into_owned())
            } else {
                None
            };

            // Read file content (empty for symlinks)
            let mut content = Vec::new();
            if !is_symlink {
                entry
                    .read_to_end(&mut content)
                    .map_err(|e| Error::InitError(format!("Failed to read file content: {}", e)))?;
            }

            // Compute SHA-256 using shared utility
            let hash = if is_symlink {
                hash::sha256(symlink_target.as_deref().unwrap_or("").as_bytes())
            } else {
                hash::sha256(&content)
            };

            extracted_files.push(ExtractedFile {
                path: normalize_path(&entry_path)
                    .map_err(|e| Error::InitError(format!("Path normalization failed: {}", e)))?,
                content,
                size: i64::try_from(size).unwrap_or(i64::MAX),
                mode: mode as i32,
                sha256: Some(hash),
                symlink_target,
            });
        }

        debug!("Extracted {} files from DEB package", extracted_files.len());
        Ok(extracted_files)
    }

    fn to_trove(&self) -> Trove {
        self.meta.to_trove()
    }

    fn scriptlets(&self) -> &[Scriptlet] {
        self.meta.scriptlets()
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
mod tests {
    use super::*;
    use crate::packages::traits::{
        DebMaintainerMode, DebTriggerAwaitMode, DebTriggerDirective, NativeArgumentValue,
        NativeLifecyclePath, NativeScriptletKind, NativeScriptletMetadata, NativeStdinContract,
    };

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

    #[test]
    fn deb_shebang_split_preserves_flattened_interpreter_behavior() {
        let body = b"#!/usr/bin/perl -w\nprint qq(ok\\n);\n";
        let native =
            DebPackage::native_abi_from_control_member("postinst", body).expect("native postinst");

        assert_eq!(native.interpreter.as_deref(), Some("/usr/bin/perl"));
        assert_eq!(native.interpreter_args, vec!["-w".to_string()]);

        let flattened = DebPackage::flattened_scriptlet_from_control_member("postinst", body)
            .expect("flattened postinst");
        assert_eq!(flattened.interpreter, "/usr/bin/perl -w");
    }

    #[test]
    fn deb_native_abi_includes_config_as_native_only() {
        let entry = DebPackage::native_abi_from_control_member(
            "config",
            b"#!/bin/sh\ndb_input high pkg/question\n",
        )
        .expect("config entry");

        assert_eq!(entry.native_slot, "config");
        assert_eq!(entry.primary_lifecycle, NativeLifecyclePath::Config);
        assert_eq!(entry.compatibility_phase, None);
        assert_eq!(entry.invocation.stdin, NativeStdinContract::Debconf);

        let NativeScriptletMetadata::Deb(meta) = &entry.metadata else {
            panic!("expected deb metadata");
        };
        assert!(
            meta.maintainer_modes
                .iter()
                .any(|mode| mode.mode == DebMaintainerMode::Configure)
        );
        assert!(
            meta.maintainer_modes
                .iter()
                .any(|mode| mode.mode == DebMaintainerMode::Reconfigure)
        );
    }

    #[test]
    fn deb_triggers_file_is_control_artifact_with_await_mode() {
        let body = b"interest-noawait update-icon-caches\nactivate-await ldconfig\n";
        let entry = DebPackage::native_abi_from_triggers_file(body);

        assert_eq!(entry.kind, NativeScriptletKind::ControlArtifact);
        assert_eq!(entry.native_slot, "triggers");
        assert_eq!(entry.interpreter, None);
        assert_eq!(entry.body.bytes, body);

        let NativeScriptletMetadata::Deb(meta) = &entry.metadata else {
            panic!("expected deb metadata");
        };
        assert_eq!(meta.trigger_declarations.len(), 2);
        assert_eq!(
            meta.trigger_declarations[0].directive,
            DebTriggerDirective::Interest
        );
        assert_eq!(
            meta.trigger_declarations[0].await_mode,
            DebTriggerAwaitMode::NoAwait
        );
        assert_eq!(
            meta.trigger_declarations[1].directive,
            DebTriggerDirective::Activate
        );
        assert_eq!(
            meta.trigger_declarations[1].await_mode,
            DebTriggerAwaitMode::Await
        );
    }

    #[test]
    fn deb_maintainer_invocation_table_preserves_mode_specific_arguments() {
        let preinst = DebPackage::native_abi_from_control_member("preinst", b"#!/bin/sh\n:")
            .expect("preinst entry");
        let NativeScriptletMetadata::Deb(preinst_meta) = &preinst.metadata else {
            panic!("expected deb metadata");
        };
        let upgrade = preinst_meta
            .maintainer_modes
            .iter()
            .find(|mode| mode.mode == DebMaintainerMode::Upgrade)
            .expect("preinst upgrade mode");
        assert_eq!(upgrade.args[0].name, "action");
        assert_eq!(upgrade.args[1].name, "old-version");
        assert_eq!(upgrade.args[1].index, 2);
        assert_eq!(upgrade.args[2].name, "new-version");
        assert_eq!(upgrade.args[2].index, 3);

        let postinst = DebPackage::native_abi_from_control_member("postinst", b"#!/bin/sh\n:")
            .expect("postinst entry");
        let NativeScriptletMetadata::Deb(postinst_meta) = &postinst.metadata else {
            panic!("expected deb metadata");
        };
        let triggered = postinst_meta
            .maintainer_modes
            .iter()
            .find(|mode| mode.mode == DebMaintainerMode::Triggered)
            .expect("postinst triggered mode");
        assert_eq!(triggered.args[1].name, "trigger-names");
        assert_eq!(triggered.args[1].value, NativeArgumentValue::TriggerNames);

        let abort_deconfigure = postinst_meta
            .maintainer_modes
            .iter()
            .find(|mode| mode.mode == DebMaintainerMode::AbortDeconfigure)
            .expect("postinst abort-deconfigure mode");
        assert_eq!(abort_deconfigure.args[1].name, "in-favour");
        assert_eq!(abort_deconfigure.args[2].name, "failed-install-package");
        assert_eq!(abort_deconfigure.args[3].index, 4);
        assert_eq!(abort_deconfigure.args[5].name, "conflicting-package");
        assert_eq!(abort_deconfigure.args[6].name, "conflicting-version");

        let prerm = DebPackage::native_abi_from_control_member("prerm", b"#!/bin/sh\n:")
            .expect("prerm entry");
        let NativeScriptletMetadata::Deb(prerm_meta) = &prerm.metadata else {
            panic!("expected deb metadata");
        };
        let deconfigure = prerm_meta
            .maintainer_modes
            .iter()
            .find(|mode| mode.mode == DebMaintainerMode::Deconfigure)
            .expect("prerm deconfigure mode");
        assert_eq!(deconfigure.args[1].name, "in-favour");
        assert_eq!(deconfigure.args[2].name, "package-being-installed");
        assert_eq!(deconfigure.args[3].index, 4);
        assert_eq!(deconfigure.args[5].name, "conflicting-package");
        assert_eq!(deconfigure.args[6].name, "conflicting-version");

        let postrm = DebPackage::native_abi_from_control_member("postrm", b"#!/bin/sh\n:")
            .expect("postrm entry");
        let NativeScriptletMetadata::Deb(postrm_meta) = &postrm.metadata else {
            panic!("expected deb metadata");
        };
        let disappear = postrm_meta
            .maintainer_modes
            .iter()
            .find(|mode| mode.mode == DebMaintainerMode::Disappear)
            .expect("postrm disappear mode");
        assert_eq!(disappear.args[1].name, "overwriter-package");
        assert_eq!(disappear.args[2].name, "overwriter-version");
    }
}
