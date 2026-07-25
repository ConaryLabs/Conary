// conary-core/src/packages/arch.rs

//! Arch Linux package format parser
//!
//! Parses .pkg.tar.zst and .pkg.tar.xz packages, extracting metadata from .PKGINFO

mod alpm_hook;
mod install_script;
mod payload;

pub(crate) use alpm_hook::{
    package_hook_basename, split_alpm_words as alpm_hook_wordsplit,
    targets_match as alpm_hook_targets_match,
};

use crate::compression::{self, CompressionFormat};
use crate::db::models::Trove;
use crate::error::{Error, Result};
use crate::packages::archive_utils::normalize_path;
use crate::packages::common::PackageMetadata;
use crate::packages::traits::{
    ArchInstallScriptletMetadata, ArchNativeScriptletMetadata, ConfigFileInfo, Dependency,
    DependencyType, ExtractedFile, NativeArgumentContract, NativeArgumentValue,
    NativeInvocationContract, NativeLifecyclePath, NativeRootExpectation, NativeScriptletBody,
    NativeScriptletEntry, NativeScriptletFormat, NativeScriptletKind, NativeScriptletMetadata,
    NativeScriptletSupport, NativeStdinContract, NativeTransactionOrder, NativeTransactionPosition,
    PackageFile, PackageFormat,
};
use crate::repository::dependency_model::{RepositoryRequirementGroup, RepositoryRequirementKind};
use crate::repository::package_relation::parse_native_relation;
use crate::repository::versioning::VersionScheme;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use tar::Archive;
use tracing::debug;

/// Arch Linux package representation
pub struct ArchPackage {
    /// Common package metadata
    meta: PackageMetadata,
    // Arch-specific metadata
    url: Option<String>,
    licenses: Vec<String>,
    groups: Vec<String>,
    packager: Option<String>,
    build_date: Option<String>,
    payload: Vec<ExtractedFile>,
}

/// Arch package metadata files that should be skipped during extraction
const ARCH_METADATA_FILES: &[&str] = &[".PKGINFO", ".MTREE", ".BUILDINFO", ".INSTALL"];

impl ArchPackage {
    /// Identify compression from its exact stream header.
    fn detect_compression(file: &mut File) -> Result<CompressionFormat> {
        let mut magic = [0_u8; 8];
        let read = file
            .read(&mut magic)
            .map_err(|e| Error::InitError(format!("Failed to read package header: {e}")))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|e| Error::InitError(format!("Failed to rewind package file: {e}")))?;
        let format = CompressionFormat::from_magic_bytes(&magic[..read]);
        if format == CompressionFormat::None {
            return Err(Error::InitError(
                "Unsupported ALPM package compression header".to_string(),
            ));
        }
        Ok(format)
    }

    /// Open and decompress the package archive
    fn open_archive(path: &str) -> Result<Archive<Box<dyn Read>>> {
        let mut file = File::open(path)
            .map_err(|e| Error::InitError(format!("Failed to open package file: {}", e)))?;

        let format = Self::detect_compression(&mut file)?;
        let reader =
            compression::create_decoder_limited(file, format, compression::MAX_DECOMPRESS_SIZE)
                .map_err(|e| Error::InitError(format!("Failed to create decoder: {}", e)))?;

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
                    "size" => {
                        info.size = Some(value.parse().map_err(|error| {
                            Error::ParseError(format!(
                                "Arch .PKGINFO size has invalid value {value:?}: {error}"
                            ))
                        })?)
                    }
                    "arch" => {
                        info.architecture =
                            Some(crate::packages::common::normalize_architecture(value).to_string())
                    }
                    "license" => info.licenses.push(value.to_string()),
                    "group" => info.groups.push(value.to_string()),
                    "depend" => info.dependencies.push(value.to_string()),
                    "provides" => info.provides.push(value.to_string()),
                    "conflict" => info.conflicts.push(value.to_string()),
                    "replaces" => info.replaces.push(value.to_string()),
                    "optdepend" => info.optional_deps.push(value.to_string()),
                    "makedepend" => info.make_deps.push(value.to_string()),
                    "backup" => info.backup.push(value.to_string()),
                    _ => {} // Ignore unknown keys
                }
            }
        }

        Ok(info)
    }

    fn native_abi_from_install_bytes(bytes: &[u8]) -> Result<Vec<NativeScriptletEntry>> {
        Ok(install_script::parse(bytes)
            .into_iter()
            .map(|function| {
                NativeScriptletEntry {
                    id: format!("arch:{}", function.name),
                    format: NativeScriptletFormat::Arch,
                    kind: NativeScriptletKind::Executable,
                    native_slot: function.name.to_string(),
                    primary_lifecycle: function.lifecycle,
                    lifecycle_paths: vec![function.lifecycle],
                    // The package has no shebang. Conversion resolves this
                    // target from the exact source profile.
                    interpreter: None,
                    interpreter_args: Vec::new(),
                    body: NativeScriptletBody::from_bytes(bytes.to_vec()),
                    invocation: Self::arch_invocation_for_function(function.name),
                    order: NativeTransactionOrder::new(match function.lifecycle {
                        NativeLifecyclePath::PreInstall
                        | NativeLifecyclePath::PreUpgrade
                        | NativeLifecyclePath::PreRemove => {
                            NativeTransactionPosition::BeforePayload
                        }
                        _ => NativeTransactionPosition::AfterPayload,
                    }),
                    support: NativeScriptletSupport::Parsed,
                    metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(
                        ArchInstallScriptletMetadata {
                            install_source_sha256: crate::hash::sha256_prefixed(bytes),
                            function_name: function.name.to_string(),
                            selection_contract:
                                crate::packages::native_abi::ArchInstallSelectionContract::LibalpmGrepV1,
                        },
                    )),
                }
            })
            .collect())
    }

    fn arch_invocation_for_function(function_name: &str) -> NativeInvocationContract {
        let args = match function_name {
            // alpm-install-scriptlet(5) passes upgrade arguments as
            // new-version, old-version for both upgrade hooks.
            "pre_upgrade" => vec![
                NativeArgumentContract {
                    index: 1,
                    name: "new-version".to_string(),
                    value: NativeArgumentValue::NewVersion,
                    required: true,
                },
                NativeArgumentContract {
                    index: 2,
                    name: "old-version".to_string(),
                    value: NativeArgumentValue::OldVersion,
                    required: true,
                },
            ],
            "post_upgrade" => vec![
                NativeArgumentContract {
                    index: 1,
                    name: "new-version".to_string(),
                    value: NativeArgumentValue::NewVersion,
                    required: true,
                },
                NativeArgumentContract {
                    index: 2,
                    name: "old-version".to_string(),
                    value: NativeArgumentValue::OldVersion,
                    required: true,
                },
            ],
            "pre_install" | "post_install" => vec![NativeArgumentContract {
                index: 1,
                name: "new-version".to_string(),
                value: NativeArgumentValue::NewVersion,
                required: true,
            }],
            "pre_remove" | "post_remove" => vec![NativeArgumentContract {
                index: 1,
                name: "old-version".to_string(),
                value: NativeArgumentValue::OldVersion,
                required: true,
            }],
            _ => Vec::new(),
        };

        NativeInvocationContract {
            args,
            environment: Vec::new(),
            stdin: NativeStdinContract::None,
            root: NativeRootExpectation::InstallRoot,
        }
    }

    fn native_abi_from_alpm_hook(path: &str, bytes: &[u8]) -> Result<NativeScriptletEntry> {
        let metadata = alpm_hook::parse(path, bytes)?;
        let stdin = if metadata.action.needs_targets {
            NativeStdinContract::Paths
        } else {
            NativeStdinContract::None
        };

        Ok(NativeScriptletEntry {
            id: format!("arch:alpm-hook:{path}"),
            format: NativeScriptletFormat::Arch,
            kind: NativeScriptletKind::ControlArtifact,
            native_slot: format!("alpm-hook:{path}"),
            primary_lifecycle: NativeLifecyclePath::Trigger,
            lifecycle_paths: vec![NativeLifecyclePath::Trigger],
            interpreter: None,
            interpreter_args: Vec::new(),
            body: NativeScriptletBody::from_bytes(bytes.to_vec()),
            invocation: NativeInvocationContract {
                args: Vec::new(),
                environment: Vec::new(),
                stdin,
                root: NativeRootExpectation::InstallRoot,
            },
            order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
            support: NativeScriptletSupport::Parsed,
            metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(
                metadata,
            )),
        })
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

    fn parse_relations(
        entries: &[String],
        kind: RepositoryRequirementKind,
    ) -> Result<Vec<RepositoryRequirementGroup>> {
        entries
            .iter()
            .map(|entry| {
                parse_native_relation(kind, VersionScheme::Arch, entry).map_err(Error::ParseError)
            })
            .collect()
    }

    fn parse_requirements(
        entries: &[String],
        kind: RepositoryRequirementKind,
    ) -> Result<Vec<RepositoryRequirementGroup>> {
        entries
            .iter()
            .map(|entry| {
                let (native_text, description) = if kind == RepositoryRequirementKind::Optional {
                    entry
                        .split_once(':')
                        .map_or((entry.as_str(), None), |(name, description)| {
                            (name.trim(), Some(description.trim().to_string()))
                        })
                } else {
                    (entry.as_str(), None)
                };
                let mut group = crate::repository::requirement::parse_native_requirement(
                    kind,
                    VersionScheme::Arch,
                    native_text,
                )
                .map_err(Error::ParseError)?;
                group.description = description;
                Ok(group)
            })
            .collect()
    }
}

/// Parsed .PKGINFO metadata
#[derive(Debug, Default)]
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
    provides: Vec<String>,
    conflicts: Vec<String>,
    replaces: Vec<String>,
    optional_deps: Vec<String>,
    make_deps: Vec<String>,
    /// Backup files (config files that should preserve user changes)
    backup: Vec<String>,
}

impl PackageFormat for ArchPackage {
    fn parse(path: &str) -> Result<Self> {
        debug!("Parsing Arch package: {}", path);

        // Single-pass: decompress once and extract all metadata + file list
        let mut archive = Self::open_archive(path)?;
        let mut pkginfo_content = None;
        let mut install_bytes = None;
        let mut alpm_hook_bytes: Vec<(String, Vec<u8>)> = Vec::new();
        let mut payload_entries = Vec::new();
        let mut entries_seen = 0usize;

        for entry in archive
            .entries()
            .map_err(|e| Error::InitError(format!("Failed to read archive: {}", e)))?
        {
            entries_seen += 1;
            compression::check_archive_entry_limit(entries_seen, "Arch package archive")
                .map_err(|e| Error::InitError(format!("Failed to read archive: {}", e)))?;
            let mut entry =
                entry.map_err(|e| Error::InitError(format!("Failed to read entry: {}", e)))?;

            let entry_path = std::str::from_utf8(entry.path_bytes().as_ref())
                .map_err(|error| {
                    Error::ParseError(format!("Arch archive path is not UTF-8: {error}"))
                })?
                .to_string();

            match entry_path.as_str() {
                ".PKGINFO" => {
                    let mut content = String::new();
                    entry
                        .read_to_string(&mut content)
                        .map_err(|e| Error::InitError(format!("Failed to read .PKGINFO: {}", e)))?;
                    pkginfo_content = Some(content);
                }
                ".INSTALL" => {
                    let mut content = Vec::new();
                    entry
                        .read_to_end(&mut content)
                        .map_err(|e| Error::InitError(format!("Failed to read .INSTALL: {}", e)))?;
                    install_bytes = Some(content);
                }
                name if ARCH_METADATA_FILES.contains(&name) => {
                    // Skip other metadata files (.MTREE, .BUILDINFO)
                }
                _ => {
                    let parsed = payload::parse_entry(&mut entry)?;
                    // Only the package-owned system hook directory is
                    // source-package ABI. `/etc/pacman.d/hooks` belongs to
                    // the selected target's explicit Conary hook policy.
                    if alpm_hook::is_package_hook_path(parsed.path())
                        && let Some(content) = parsed.regular_content()
                    {
                        alpm_hook_bytes.push((parsed.path().to_string(), content.to_vec()));
                    }
                    payload_entries.push(parsed);
                }
            }
        }
        let payload = payload::resolve_hardlinks(payload_entries)?;
        let files = payload
            .iter()
            .map(|file| PackageFile {
                path: file.path.clone(),
                node: file.node.clone(),
                content: file.content_authority.clone(),
            })
            .collect::<Vec<_>>();

        let pkginfo_content = pkginfo_content
            .ok_or_else(|| Error::InitError("No .PKGINFO file found in package".to_string()))?;

        // Parse .PKGINFO
        let pkginfo = Self::parse_pkginfo(&pkginfo_content)?;

        let name = pkginfo
            .name
            .ok_or_else(|| Error::InitError("Package name not found in .PKGINFO".to_string()))?;

        let version = pkginfo
            .version
            .ok_or_else(|| Error::InitError("Package version not found in .PKGINFO".to_string()))?;

        let provides = Self::parse_dependencies(&pkginfo.provides, DependencyType::Runtime);
        let mut requirements =
            Self::parse_requirements(&pkginfo.dependencies, RepositoryRequirementKind::Depends)?;
        requirements.extend(Self::parse_requirements(
            &pkginfo.optional_deps,
            RepositoryRequirementKind::Optional,
        )?);
        requirements.extend(Self::parse_requirements(
            &pkginfo.make_deps,
            RepositoryRequirementKind::Build,
        )?);
        let mut relations =
            Self::parse_relations(&pkginfo.conflicts, RepositoryRequirementKind::Conflict)?;
        relations.extend(Self::parse_relations(
            &pkginfo.replaces,
            RepositoryRequirementKind::Replace,
        )?);

        let mut native_scriptlet_abi = install_bytes
            .as_ref()
            .map(|bytes| Self::native_abi_from_install_bytes(bytes))
            .transpose()?
            .unwrap_or_default();
        native_scriptlet_abi.extend(
            alpm_hook_bytes
                .iter()
                .map(|(path, bytes)| Self::native_abi_from_alpm_hook(path, bytes))
                .collect::<Result<Vec<_>>>()?,
        );
        // Convert backup files to ConfigFileInfo
        // Repository metadata may include "path\thash"; the incoming payload
        // and persisted installed baseline supply the other identities for the
        // exact three-hash transaction.
        // All Arch backup files preserve user changes (like noreplace)
        let mut config_files = Vec::new();
        for entry in &pkginfo.backup {
            let path = entry.split('\t').next().unwrap_or(entry);
            config_files.push(ConfigFileInfo {
                path: normalize_path(path)
                    .map_err(|e| Error::InitError(format!("Path normalization failed: {}", e)))?,
                noreplace: true, // Arch backup files always preserve user changes
                ghost: false,
                remove_on_upgrade: false,
            });
        }

        debug!(
            "Parsed Arch package: {} version {} ({} files, {} dependencies, {} native lifecycle entries, {} config files)",
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
            version_scheme: VersionScheme::Arch,
            architecture: pkginfo.architecture,
            description: pkginfo.description,
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
            url: pkginfo.url,
            licenses: pkginfo.licenses,
            groups: pkginfo.groups,
            packager: pkginfo.packager,
            build_date: pkginfo.build_date,
            payload,
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
        Ok(self.payload.clone())
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
#[path = "arch/tests.rs"]
mod tests;
