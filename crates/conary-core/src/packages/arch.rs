// conary-core/src/packages/arch.rs

//! Arch Linux package format parser
//!
//! Parses .pkg.tar.zst and .pkg.tar.xz packages, extracting metadata from .PKGINFO

use crate::compression::{self, CompressionFormat};
use crate::db::models::Trove;
use crate::error::{Error, Result};
use crate::hash;
use crate::packages::archive_utils::{check_file_size, normalize_path};
use crate::packages::common::PackageMetadata;
use crate::packages::traits::{
    ArchAlpmHookAction, ArchAlpmHookMetadata, ArchAlpmHookOperation, ArchAlpmHookTrigger,
    ArchAlpmHookTriggerType, ArchFunctionExtractionStatus, ArchInstallScriptletMetadata,
    ArchNativeScriptletMetadata, ConfigFileInfo, Dependency, DependencyType, ExtractedFile,
    NativeArgumentContract, NativeArgumentValue, NativeInvocationContract, NativeLifecyclePath,
    NativeRootExpectation, NativeScriptletBody, NativeScriptletEntry, NativeScriptletFormat,
    NativeScriptletKind, NativeScriptletMetadata, NativeScriptletSupport, NativeStdinContract,
    NativeTransactionOrder, NativeTransactionPosition, PackageFile, PackageFormat, Scriptlet,
    ScriptletPhase,
};
use std::fs::File;
use std::io::Read;
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
}

/// Arch package metadata files that should be skipped during extraction
const ARCH_METADATA_FILES: &[&str] = &[".PKGINFO", ".MTREE", ".BUILDINFO", ".INSTALL"];

const ARCH_INSTALL_FUNCTIONS: &[(&str, ScriptletPhase, NativeLifecyclePath)] = &[
    (
        "pre_install",
        ScriptletPhase::PreInstall,
        NativeLifecyclePath::PreInstall,
    ),
    (
        "post_install",
        ScriptletPhase::PostInstall,
        NativeLifecyclePath::PostInstall,
    ),
    (
        "pre_upgrade",
        ScriptletPhase::PreUpgrade,
        NativeLifecyclePath::PreUpgrade,
    ),
    (
        "post_upgrade",
        ScriptletPhase::PostUpgrade,
        NativeLifecyclePath::PostUpgrade,
    ),
    (
        "pre_remove",
        ScriptletPhase::PreRemove,
        NativeLifecyclePath::PreRemove,
    ),
    (
        "post_remove",
        ScriptletPhase::PostRemove,
        NativeLifecyclePath::PostRemove,
    ),
];

impl ArchPackage {
    /// Detect compression format from file extension
    fn detect_compression(path: &str) -> Result<CompressionFormat> {
        let format = CompressionFormat::from_extension(path);
        if format == CompressionFormat::None {
            return Err(Error::InitError(format!(
                "Unsupported Arch package format: {}. Expected .pkg.tar.zst, .pkg.tar.xz, or .pkg.tar.gz",
                path
            )));
        }
        Ok(format)
    }

    /// Open and decompress the package archive
    fn open_archive(path: &str) -> Result<Archive<Box<dyn Read>>> {
        let file = File::open(path)
            .map_err(|e| Error::InitError(format!("Failed to open package file: {}", e)))?;

        let format = Self::detect_compression(path)?;
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
                    "size" => info.size = value.parse().ok(),
                    "arch" => {
                        info.architecture =
                            Some(crate::packages::common::normalize_architecture(value).to_string())
                    }
                    "license" => info.licenses.push(value.to_string()),
                    "group" => info.groups.push(value.to_string()),
                    "depend" => info.dependencies.push(value.to_string()),
                    "provides" => info.provides.push(value.to_string()),
                    "optdepend" => info.optional_deps.push(value.to_string()),
                    "makedepend" => info.make_deps.push(value.to_string()),
                    "backup" => info.backup.push(value.to_string()),
                    _ => {} // Ignore unknown keys
                }
            }
        }

        Ok(info)
    }

    /// Parse .INSTALL file content to extract scriptlets
    ///
    /// Arch .INSTALL files contain shell functions like:
    /// - pre_install()
    /// - post_install()
    /// - pre_upgrade()
    /// - post_upgrade()
    /// - pre_remove()
    /// - post_remove()
    fn parse_install_script(content: &str) -> Vec<Scriptlet> {
        let mut scriptlets = Vec::new();

        for (func_name, phase, _) in ARCH_INSTALL_FUNCTIONS {
            if let Some(func_content) = Self::extract_function(content, func_name) {
                scriptlets.push(Scriptlet {
                    phase: *phase,
                    interpreter: "/bin/sh".to_string(),
                    content: func_content,
                    flags: None,
                });
            }
        }

        scriptlets
    }

    /// Extract a shell function body from script content
    fn extract_function(content: &str, func_name: &str) -> Option<String> {
        // Look for function definition patterns anchored at the start of a line
        // (after optional whitespace) to avoid substring matches
        let patterns = [
            format!("{}()", func_name),
            format!("{} ()", func_name),
            format!("function {}", func_name),
        ];

        let mut start_idx = None;
        for line in content.lines() {
            let trimmed = line.trim_start();
            for pattern in &patterns {
                if trimmed.starts_with(pattern.as_str()) {
                    // Find the byte offset of this match in the original content
                    let line_start = line.as_ptr() as usize - content.as_ptr() as usize;
                    let trim_offset = trimmed.as_ptr() as usize - line.as_ptr() as usize;
                    start_idx = Some(line_start + trim_offset);
                    break;
                }
            }
            if start_idx.is_some() {
                break;
            }
        }

        let start = start_idx?;

        // Find the opening brace
        let rest = &content[start..];
        let open_brace = rest.find('{')?;
        let func_start = start + open_brace + 1;

        // Find matching closing brace by counting braces, skipping comments.
        // After a '#', skip to end of line before counting braces.
        let mut brace_count = 1;
        let mut end_idx = func_start;
        let mut in_comment = false;

        for (i, ch) in content[func_start..].char_indices() {
            if ch == '\n' {
                in_comment = false;
                continue;
            }
            if in_comment {
                continue;
            }
            if ch == '#' {
                in_comment = true;
                continue;
            }
            match ch {
                '{' => brace_count += 1,
                '}' => {
                    brace_count -= 1;
                    if brace_count == 0 {
                        end_idx = func_start + i;
                        break;
                    }
                }
                _ => {}
            }
        }

        if brace_count != 0 {
            return None; // Unbalanced braces
        }

        let body = content[func_start..end_idx].trim();
        if body.is_empty() {
            None
        } else {
            Some(body.to_string())
        }
    }

    fn native_abi_from_install_bytes(bytes: &[u8]) -> Vec<NativeScriptletEntry> {
        let Ok(source) = std::str::from_utf8(bytes) else {
            return vec![NativeScriptletEntry {
                id: "arch:.INSTALL".to_string(),
                format: NativeScriptletFormat::Arch,
                kind: NativeScriptletKind::ControlArtifact,
                native_slot: ".INSTALL".to_string(),
                primary_lifecycle: NativeLifecyclePath::Trigger,
                compatibility_phase: None,
                lifecycle_paths: Vec::new(),
                interpreter: Some("/bin/sh".to_string()),
                interpreter_args: Vec::new(),
                body: NativeScriptletBody::from_bytes(bytes.to_vec()),
                invocation: NativeInvocationContract {
                    args: Vec::new(),
                    environment: Vec::new(),
                    stdin: NativeStdinContract::None,
                    root: NativeRootExpectation::InstallRoot,
                },
                order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
                support: NativeScriptletSupport::DeferredReview {
                    reason_code: "native-abi-parser-limitation".to_string(),
                },
                metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(
                    ArchInstallScriptletMetadata {
                        install_source_sha256: crate::hash::sha256_prefixed(bytes),
                        function_name: ".INSTALL".to_string(),
                        function_body: None,
                        function_body_sha256: None,
                        extraction_status: ArchFunctionExtractionStatus::DeferredReview {
                            reason_code: "native-abi-parser-limitation".to_string(),
                        },
                    },
                )),
            }];
        };

        ARCH_INSTALL_FUNCTIONS
            .iter()
            .filter(|(function_name, _, _)| {
                Self::contains_function_declaration(source, function_name)
            })
            .map(|(function_name, phase, lifecycle)| {
                let function_body = Self::extract_function(source, function_name);
                let function_extracted = function_body.is_some();
                let extraction_status = if function_extracted {
                    ArchFunctionExtractionStatus::Parsed
                } else {
                    ArchFunctionExtractionStatus::DeferredReview {
                        reason_code: "arch-install-function-extraction-deferred".to_string(),
                    }
                };
                let function_body_sha256 = function_body
                    .as_ref()
                    .map(|body| crate::hash::sha256_prefixed(body.as_bytes()));

                NativeScriptletEntry {
                    id: format!("arch:{function_name}"),
                    format: NativeScriptletFormat::Arch,
                    kind: NativeScriptletKind::Executable,
                    native_slot: (*function_name).to_string(),
                    primary_lifecycle: *lifecycle,
                    compatibility_phase: function_extracted.then_some(*phase),
                    lifecycle_paths: vec![*lifecycle],
                    interpreter: Some("/bin/sh".to_string()),
                    interpreter_args: Vec::new(),
                    body: NativeScriptletBody::from_bytes(bytes.to_vec()),
                    invocation: Self::arch_invocation_for_function(function_name),
                    order: NativeTransactionOrder::new(match *lifecycle {
                        NativeLifecyclePath::PreInstall
                        | NativeLifecyclePath::PreUpgrade
                        | NativeLifecyclePath::PreRemove => {
                            NativeTransactionPosition::BeforePayload
                        }
                        _ => NativeTransactionPosition::AfterPayload,
                    }),
                    support: if function_extracted {
                        NativeScriptletSupport::Parsed
                    } else {
                        NativeScriptletSupport::DeferredReview {
                            reason_code: "arch-install-function-extraction-deferred".to_string(),
                        }
                    },
                    metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(
                        ArchInstallScriptletMetadata {
                            install_source_sha256: crate::hash::sha256_prefixed(bytes),
                            function_name: (*function_name).to_string(),
                            function_body,
                            function_body_sha256,
                            extraction_status,
                        },
                    )),
                }
            })
            .collect()
    }

    fn contains_function_declaration(source: &str, function_name: &str) -> bool {
        source.lines().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with(&format!("{function_name}()"))
                || trimmed.starts_with(&format!("{function_name} ()"))
                || trimmed.starts_with(&format!("function {function_name}"))
        })
    }

    fn arch_invocation_for_function(function_name: &str) -> NativeInvocationContract {
        let args = match function_name {
            // alpm-install-scriptlet(5) reverses upgrade arguments between
            // pre_upgrade and post_upgrade.
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
                    name: "old-version".to_string(),
                    value: NativeArgumentValue::OldVersion,
                    required: true,
                },
                NativeArgumentContract {
                    index: 2,
                    name: "new-version".to_string(),
                    value: NativeArgumentValue::NewVersion,
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

    fn native_abi_from_alpm_hook(path: &str, bytes: &[u8]) -> NativeScriptletEntry {
        let text = std::str::from_utf8(bytes).unwrap_or_default();
        let (triggers, action) = Self::parse_alpm_hook_metadata(text);

        NativeScriptletEntry {
            id: format!("arch:alpm-hook:{path}"),
            format: NativeScriptletFormat::Arch,
            kind: NativeScriptletKind::ControlArtifact,
            native_slot: format!("alpm-hook:{path}"),
            primary_lifecycle: NativeLifecyclePath::Trigger,
            compatibility_phase: None,
            lifecycle_paths: vec![NativeLifecyclePath::Trigger],
            interpreter: None,
            interpreter_args: Vec::new(),
            body: NativeScriptletBody::from_bytes(bytes.to_vec()),
            invocation: NativeInvocationContract {
                args: Vec::new(),
                environment: Vec::new(),
                stdin: if action.as_ref().is_some_and(|action| action.needs_targets) {
                    NativeStdinContract::Paths
                } else {
                    NativeStdinContract::None
                },
                root: NativeRootExpectation::InstallRoot,
            },
            order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
            support: NativeScriptletSupport::DeferredReview {
                reason_code: "arch-alpm-hook-semantics-deferred".to_string(),
            },
            metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(
                ArchAlpmHookMetadata {
                    hook_path: path.to_string(),
                    triggers,
                    action,
                },
            )),
        }
    }

    fn parse_alpm_hook_metadata(
        text: &str,
    ) -> (Vec<ArchAlpmHookTrigger>, Option<ArchAlpmHookAction>) {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Section {
            None,
            Trigger,
            Action,
        }

        let mut section = Section::None;
        let mut triggers = Vec::new();
        let mut current_trigger: Option<ArchAlpmHookTrigger> = None;
        let mut action = ArchAlpmHookAction {
            description: None,
            when: NativeTransactionPosition::AfterTransaction,
            exec: String::new(),
            depends: Vec::new(),
            abort_on_fail: false,
            needs_targets: false,
        };

        for raw_line in text.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            match line {
                "[Trigger]" => {
                    if let Some(trigger) = current_trigger.take() {
                        triggers.push(trigger);
                    }
                    current_trigger = Some(ArchAlpmHookTrigger {
                        operations: Vec::new(),
                        trigger_type: ArchAlpmHookTriggerType::Package,
                        targets: Vec::new(),
                    });
                    section = Section::Trigger;
                    continue;
                }
                "[Action]" => {
                    if let Some(trigger) = current_trigger.take() {
                        triggers.push(trigger);
                    }
                    section = Section::Action;
                    continue;
                }
                "AbortOnFail" if section == Section::Action => {
                    action.abort_on_fail = true;
                    continue;
                }
                "NeedsTargets" if section == Section::Action => {
                    action.needs_targets = true;
                    continue;
                }
                _ => {}
            }

            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().to_string();

            match (section, key) {
                (Section::Trigger, "Operation") => {
                    if let Some(trigger) = current_trigger.as_mut() {
                        match value.as_str() {
                            "Install" => trigger.operations.push(ArchAlpmHookOperation::Install),
                            "Upgrade" => trigger.operations.push(ArchAlpmHookOperation::Upgrade),
                            "Remove" => trigger.operations.push(ArchAlpmHookOperation::Remove),
                            _ => {}
                        }
                    }
                }
                (Section::Trigger, "Type") => {
                    if let Some(trigger) = current_trigger.as_mut() {
                        trigger.trigger_type = if value == "Path" || value == "File" {
                            ArchAlpmHookTriggerType::Path
                        } else {
                            ArchAlpmHookTriggerType::Package
                        };
                    }
                }
                (Section::Trigger, "Target") => {
                    if let Some(trigger) = current_trigger.as_mut() {
                        trigger.targets.push(value);
                    }
                }
                (Section::Action, "Description") => action.description = Some(value),
                (Section::Action, "When") => {
                    action.when = if value == "PreTransaction" {
                        NativeTransactionPosition::BeforeTransaction
                    } else {
                        NativeTransactionPosition::AfterTransaction
                    };
                }
                (Section::Action, "Exec") => action.exec = value,
                (Section::Action, "Depends") => action.depends.push(value),
                _ => {}
            }
        }

        if let Some(trigger) = current_trigger {
            triggers.push(trigger);
        }

        let action = if action.exec.is_empty() {
            None
        } else {
            Some(action)
        };
        (triggers, action)
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
    provides: Vec<String>,
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
        let mut files = Vec::new();
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

            let entry_path = entry
                .path()
                .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                .to_string_lossy()
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
                    let entry_type = entry.header().entry_type();
                    // Collect file list entry (regular files + symlinks, skip directories)
                    if !entry_type.is_dir() {
                        let normalized_path = normalize_path(&entry_path).map_err(|e| {
                            Error::InitError(format!("Path normalization failed: {}", e))
                        })?;
                        let size = entry.header().size().map_err(|e| {
                            Error::InitError(format!("Failed to get file size: {}", e))
                        })?;
                        let mode = entry.header().mode().map_err(|e| {
                            Error::InitError(format!("Failed to get file mode: {}", e))
                        })?;
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
                            path: normalized_path.clone(),
                            size: i64::try_from(size).unwrap_or(i64::MAX),
                            mode: mode as i32,
                            sha256: None,
                            symlink_target,
                        });

                        let is_alpm_hook = normalized_path.starts_with("/usr/share/libalpm/hooks/")
                            && normalized_path.ends_with(".hook");
                        if is_alpm_hook && !entry_type.is_symlink() {
                            let mut content = Vec::new();
                            entry.read_to_end(&mut content).map_err(|e| {
                                Error::InitError(format!("Failed to read ALPM hook: {}", e))
                            })?;
                            alpm_hook_bytes.push((normalized_path, content));
                        }
                    }
                }
            }
        }

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

        // Parse dependencies
        let mut dependencies = Vec::new();
        dependencies.extend(Self::parse_dependencies(
            &pkginfo.dependencies,
            DependencyType::Runtime,
        ));
        dependencies.extend(Self::parse_dependencies(
            &pkginfo.optional_deps,
            DependencyType::Optional,
        ));
        dependencies.extend(Self::parse_dependencies(
            &pkginfo.make_deps,
            DependencyType::Build,
        ));
        let provides = Self::parse_dependencies(&pkginfo.provides, DependencyType::Runtime);

        // Parse scriptlets from .INSTALL file (already extracted in single pass)
        let scriptlets = install_bytes
            .as_ref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .map(Self::parse_install_script)
            .unwrap_or_default();
        let mut native_scriptlet_abi = install_bytes
            .as_ref()
            .map(|bytes| Self::native_abi_from_install_bytes(bytes))
            .unwrap_or_default();
        native_scriptlet_abi.extend(
            alpm_hook_bytes
                .iter()
                .map(|(path, bytes)| Self::native_abi_from_alpm_hook(path, bytes)),
        );

        // Convert backup files to ConfigFileInfo
        // Arch backup format is "path\thash" but we just need the path
        // All Arch backup files preserve user changes (like noreplace)
        let mut config_files = Vec::new();
        for entry in &pkginfo.backup {
            let path = entry.split('\t').next().unwrap_or(entry);
            config_files.push(ConfigFileInfo {
                path: normalize_path(path)
                    .map_err(|e| Error::InitError(format!("Path normalization failed: {}", e)))?,
                noreplace: true, // Arch backup files always preserve user changes
                ghost: false,
            });
        }

        debug!(
            "Parsed Arch package: {} version {} ({} files, {} dependencies, {} scriptlets, {} config files)",
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
            architecture: pkginfo.architecture,
            description: pkginfo.description,
            files,
            dependencies,
            provides,
            scriptlets,
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
            "Extracting file contents from Arch package: {:?}",
            self.meta.package_path()
        );

        let path_str =
            self.meta.package_path().to_str().ok_or_else(|| {
                Error::InitError("Package path contains invalid UTF-8".to_string())
            })?;
        let mut archive = Self::open_archive(path_str)?;
        let mut extracted_files = Vec::new();
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

            let entry_path = entry
                .path()
                .map_err(|e| Error::InitError(format!("Failed to get entry path: {}", e)))?
                .to_string_lossy()
                .to_string();

            if ARCH_METADATA_FILES.contains(&entry_path.as_str()) {
                continue;
            }

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
                // Hash the symlink target for tracking
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

        debug!(
            "Extracted {} files from Arch package",
            extracted_files.len()
        );
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
    use crate::packages::traits::{
        ArchAlpmHookOperation, ArchAlpmHookTriggerType, ArchFunctionExtractionStatus,
        ArchNativeScriptletMetadata, NativeArgumentValue, NativeLifecyclePath,
        NativeScriptletFormat, NativeScriptletKind, NativeScriptletMetadata,
        NativeTransactionPosition,
    };

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
        let deps = vec!["glibc>=2.34".to_string(), "zlib".to_string()];

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
        assert_eq!(
            parsed[0].description,
            Some("for running scripts".to_string())
        );
        assert_eq!(parsed[1].name, "ruby");
        assert_eq!(parsed[1].description, None);
    }

    #[test]
    fn test_extract_function() {
        let content = r#"
post_install() {
    echo "Installing..."
    systemctl daemon-reload
}

post_upgrade() {
    post_install
}
"#;

        // Test extracting post_install
        let body = ArchPackage::extract_function(content, "post_install");
        assert!(body.is_some());
        let body = body.unwrap();
        assert!(body.contains("Installing"));
        assert!(body.contains("daemon-reload"));

        // Test extracting post_upgrade
        let body = ArchPackage::extract_function(content, "post_upgrade");
        assert!(body.is_some());
        assert!(body.unwrap().contains("post_install"));

        // Test non-existent function
        let body = ArchPackage::extract_function(content, "pre_install");
        assert!(body.is_none());
    }

    #[test]
    fn test_extract_function_skips_comment_braces() {
        let content = r#"
post_install() {
    echo "Installing..."
    # This comment has braces { } that should be ignored
    echo "Done"
}
"#;
        let body = ArchPackage::extract_function(content, "post_install");
        assert!(body.is_some());
        let body = body.unwrap();
        assert!(body.contains("Installing"));
        assert!(body.contains("Done"));
        // Ensure the comment line is included in the body
        assert!(body.contains("# This comment has braces"));
    }

    #[test]
    fn test_parse_install_script() {
        let content = r#"
pre_install() {
    echo "Preparing installation"
}

post_install() {
    systemctl daemon-reload
    systemctl enable myservice
}

pre_remove() {
    systemctl stop myservice
    systemctl disable myservice
}
"#;

        let scriptlets = ArchPackage::parse_install_script(content);
        assert_eq!(scriptlets.len(), 3);

        // Check phases
        let phases: Vec<_> = scriptlets.iter().map(|s| s.phase).collect();
        assert!(phases.contains(&ScriptletPhase::PreInstall));
        assert!(phases.contains(&ScriptletPhase::PostInstall));
        assert!(phases.contains(&ScriptletPhase::PreRemove));

        // All should use /bin/sh interpreter
        for s in &scriptlets {
            assert_eq!(s.interpreter, "/bin/sh");
        }
    }

    #[test]
    fn arch_native_abi_preserves_full_install_source_and_function_body() {
        let install = br#"
pre_install() {
    echo "before $1"
}

post_upgrade() {
    echo "after $2 -> $1"
}
"#;

        let entries = ArchPackage::native_abi_from_install_bytes(install);

        assert_eq!(entries.len(), 2);
        let pre = entries
            .iter()
            .find(|entry| entry.native_slot == "pre_install")
            .expect("pre_install entry");
        assert_eq!(pre.format, NativeScriptletFormat::Arch);
        assert_eq!(pre.primary_lifecycle, NativeLifecyclePath::PreInstall);
        assert_eq!(pre.compatibility_phase, Some(ScriptletPhase::PreInstall));
        assert_eq!(pre.interpreter.as_deref(), Some("/bin/sh"));
        assert_eq!(pre.body.bytes, install);

        let NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(meta)) =
            &pre.metadata
        else {
            panic!("expected arch install metadata");
        };
        assert_eq!(meta.function_name, "pre_install");
        assert_eq!(meta.function_body.as_deref(), Some("echo \"before $1\""));
        assert_eq!(
            meta.function_body_sha256.as_deref(),
            Some(crate::hash::sha256_prefixed(b"echo \"before $1\"").as_str())
        );

        let post_upgrade = entries
            .iter()
            .find(|entry| entry.native_slot == "post_upgrade")
            .expect("post_upgrade entry");
        assert_eq!(post_upgrade.invocation.args[0].name, "old-version");
        assert_eq!(
            post_upgrade.invocation.args[0].value,
            NativeArgumentValue::OldVersion
        );
        assert_eq!(post_upgrade.invocation.args[1].name, "new-version");
        assert_eq!(
            post_upgrade.invocation.args[1].value,
            NativeArgumentValue::NewVersion
        );
    }

    #[test]
    fn arch_native_abi_preserves_alpm_hook_control_artifact() {
        let hook = br#"
[Trigger]
Operation = Install
Operation = Upgrade
Type = Path
Target = usr/share/mime/*

[Action]
Description = update mime cache
When = PostTransaction
Exec = /usr/bin/update-mime-database /usr/share/mime
Depends = shared-mime-info
NeedsTargets
"#;

        let entry = ArchPackage::native_abi_from_alpm_hook(
            "/usr/share/libalpm/hooks/30-update-mime.hook",
            hook,
        );

        assert_eq!(entry.kind, NativeScriptletKind::ControlArtifact);
        assert_eq!(
            entry.native_slot,
            "alpm-hook:/usr/share/libalpm/hooks/30-update-mime.hook"
        );
        assert_eq!(entry.primary_lifecycle, NativeLifecyclePath::Trigger);
        assert_eq!(
            entry.order.position,
            NativeTransactionPosition::ControlArtifact
        );
        assert_eq!(entry.interpreter, None);
        assert_eq!(entry.body.bytes, hook);

        let NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(meta)) =
            &entry.metadata
        else {
            panic!("expected arch alpm hook metadata");
        };
        assert_eq!(meta.triggers.len(), 1);
        assert_eq!(
            meta.triggers[0].operations,
            vec![
                ArchAlpmHookOperation::Install,
                ArchAlpmHookOperation::Upgrade,
            ]
        );
        assert_eq!(meta.triggers[0].trigger_type, ArchAlpmHookTriggerType::Path);
        assert_eq!(
            meta.triggers[0].targets,
            vec!["usr/share/mime/*".to_string()]
        );
        assert_eq!(
            meta.action.as_ref().expect("action").when,
            NativeTransactionPosition::AfterTransaction
        );
        assert!(meta.action.as_ref().expect("action").needs_targets);
    }

    #[test]
    fn arch_native_abi_falls_back_when_function_extraction_fails() {
        let install = b"pre_install() {\necho '{' unbalanced\n";
        let entries = ArchPackage::native_abi_from_install_bytes(install);

        let pre = entries
            .iter()
            .find(|entry| entry.native_slot == "pre_install")
            .expect("pre_install entry");

        assert_eq!(
            pre.support.reason_code(),
            Some("arch-install-function-extraction-deferred")
        );

        let NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(meta)) =
            &pre.metadata
        else {
            panic!("expected arch install metadata");
        };
        assert_eq!(
            meta.extraction_status,
            ArchFunctionExtractionStatus::DeferredReview {
                reason_code: "arch-install-function-extraction-deferred".to_string(),
            }
        );
        assert_eq!(meta.function_body, None);
    }

    #[test]
    fn arch_compat_scriptlets_still_return_function_bodies() {
        let install = r#"
post_install() {
    echo "compat body"
}
"#;

        let scriptlets = ArchPackage::parse_install_script(install);

        assert_eq!(scriptlets.len(), 1);
        assert_eq!(scriptlets[0].phase, ScriptletPhase::PostInstall);
        assert_eq!(scriptlets[0].content, "echo \"compat body\"");
    }
}
