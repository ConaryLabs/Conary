// conary-core/src/packages/rpm.rs

//! RPM package format parser

use crate::compression::{self, CompressionFormat};
use crate::db::models::Trove;
use crate::error::{Error, Result};
use crate::packages::archive_utils::{check_file_size, is_regular_file_mode, normalize_path};
use crate::packages::common::PackageMetadata;
use crate::packages::cpio::CpioReader;
use crate::packages::traits::{
    ConfigFileInfo, Dependency, DependencyType, ExtractedFile, NativeArgumentContract,
    NativeArgumentValue, NativeInvocationContract, NativeLifecyclePath, NativeRootExpectation,
    NativeScriptletBody, NativeScriptletEntry, NativeScriptletFormat, NativeScriptletKind,
    NativeScriptletMetadata, NativeScriptletSupport, NativeStdinContract, NativeTransactionOrder,
    NativeTransactionPosition, PackageFile, PackageFormat, RpmNativeScriptletMetadata,
    RpmScriptletFlagsMetadata, RpmScriptletSlot, RpmTriggerAction, RpmTriggerCondition,
    RpmTriggerFamily, RpmTriggerMetadata, Scriptlet, ScriptletPhase,
};
use rpm::Package;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use tracing::debug;

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
        let mut add_scriptlet =
            |phase: ScriptletPhase, result: std::result::Result<rpm::Scriptlet, rpm::Error>| {
                if let Ok(s) = result {
                    let content = s.script; // Accessing field directly
                    if !content.is_empty() {
                        let interpreter = s
                            .program
                            .and_then(|progs| progs.into_iter().next())
                            .unwrap_or_else(|| "/bin/sh".to_string());

                        scriptlets.push(Scriptlet {
                            phase,
                            interpreter,
                            content,
                            flags: None,
                        });
                    }
                }
            };

        add_scriptlet(
            ScriptletPhase::PreInstall,
            pkg.metadata.get_pre_install_script(),
        );
        add_scriptlet(
            ScriptletPhase::PostInstall,
            pkg.metadata.get_post_install_script(),
        );
        add_scriptlet(
            ScriptletPhase::PreRemove,
            pkg.metadata.get_pre_uninstall_script(),
        );
        add_scriptlet(
            ScriptletPhase::PostRemove,
            pkg.metadata.get_post_uninstall_script(),
        );
        add_scriptlet(
            ScriptletPhase::PreTransaction,
            pkg.metadata.get_pre_trans_script(),
        );
        add_scriptlet(
            ScriptletPhase::PostTransaction,
            pkg.metadata.get_post_trans_script(),
        );

        scriptlets
    }

    fn rpm_scriptlet_program(scriptlet: &rpm::Scriptlet) -> (String, Vec<String>) {
        let mut program = scriptlet.program.clone().unwrap_or_default().into_iter();
        let interpreter = program.next().unwrap_or_else(|| "/bin/sh".to_string());
        let args = program.collect();
        (interpreter, args)
    }

    fn rpm_scriptlet_flags_metadata(flags: rpm::ScriptletFlags) -> RpmScriptletFlagsMetadata {
        let mut names = Vec::new();
        if flags.contains(rpm::ScriptletFlags::EXPAND) {
            names.push("EXPAND".to_string());
        }
        if flags.contains(rpm::ScriptletFlags::QFORMAT) {
            names.push("QFORMAT".to_string());
        }
        if flags.contains(rpm::ScriptletFlags::CRITICAL) {
            names.push("CRITICAL".to_string());
        }

        RpmScriptletFlagsMetadata {
            names,
            raw_bits: flags.bits(),
        }
    }

    fn rpm_trigger_action(flags: rpm::DependencyFlags) -> RpmTriggerAction {
        if flags.contains(rpm::DependencyFlags::TRIGGERPREIN) {
            RpmTriggerAction::PreInstall
        } else if flags.contains(rpm::DependencyFlags::TRIGGERIN) {
            RpmTriggerAction::Install
        } else if flags.contains(rpm::DependencyFlags::TRIGGERUN) {
            RpmTriggerAction::Uninstall
        } else if flags.contains(rpm::DependencyFlags::TRIGGERPOSTUN) {
            RpmTriggerAction::PostUninstall
        } else {
            RpmTriggerAction::Unknown {
                raw_flags: flags.bits(),
            }
        }
    }

    fn rpm_trigger_comparison(flags: rpm::DependencyFlags) -> Option<String> {
        let operator = flags_to_operator(flags).trim();
        if operator.is_empty() {
            None
        } else {
            Some(operator.to_string())
        }
    }

    fn extract_native_scriptlet_abi(pkg: &Package) -> Vec<NativeScriptletEntry> {
        let mut entries = Vec::new();

        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%pre",
            RpmScriptletSlot::Pre,
            NativeLifecyclePath::PreInstall,
            Some(ScriptletPhase::PreInstall),
            pkg.metadata.get_pre_install_script(),
        );
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%post",
            RpmScriptletSlot::Post,
            NativeLifecyclePath::PostInstall,
            Some(ScriptletPhase::PostInstall),
            pkg.metadata.get_post_install_script(),
        );
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%preun",
            RpmScriptletSlot::PreUn,
            NativeLifecyclePath::PreRemove,
            Some(ScriptletPhase::PreRemove),
            pkg.metadata.get_pre_uninstall_script(),
        );
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%postun",
            RpmScriptletSlot::PostUn,
            NativeLifecyclePath::PostRemove,
            Some(ScriptletPhase::PostRemove),
            pkg.metadata.get_post_uninstall_script(),
        );
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%pretrans",
            RpmScriptletSlot::PreTrans,
            NativeLifecyclePath::PreTransaction,
            Some(ScriptletPhase::PreTransaction),
            pkg.metadata.get_pre_trans_script(),
        );
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%posttrans",
            RpmScriptletSlot::PostTrans,
            NativeLifecyclePath::PostTransaction,
            Some(ScriptletPhase::PostTransaction),
            pkg.metadata.get_post_trans_script(),
        );
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%preuntrans",
            RpmScriptletSlot::PreUnTrans,
            NativeLifecyclePath::PreUntransaction,
            None,
            pkg.metadata.get_pre_untrans_script(),
        );
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%postuntrans",
            RpmScriptletSlot::PostUnTrans,
            NativeLifecyclePath::PostUntransaction,
            None,
            pkg.metadata.get_post_untrans_script(),
        );
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%verify",
            RpmScriptletSlot::Verify,
            NativeLifecyclePath::Verify,
            None,
            pkg.metadata.get_verify_script(),
        );

        Self::add_rpm_triggers(
            &mut entries,
            RpmTriggerFamily::Package,
            pkg.metadata.get_triggers(),
        );
        Self::add_rpm_triggers(
            &mut entries,
            RpmTriggerFamily::File,
            pkg.metadata.get_file_triggers(),
        );
        Self::add_rpm_triggers(
            &mut entries,
            RpmTriggerFamily::TransactionFile,
            pkg.metadata.get_trans_file_triggers(),
        );

        entries
    }

    fn rpm_scriptlet_invocation() -> NativeInvocationContract {
        NativeInvocationContract {
            args: vec![NativeArgumentContract {
                index: 1,
                name: "package-instance-count".to_string(),
                value: NativeArgumentValue::PackageInstanceCount,
                required: true,
            }],
            environment: Vec::new(),
            stdin: NativeStdinContract::None,
            root: NativeRootExpectation::PackageManagerDefault,
        }
    }

    fn rpm_lifecycle_paths_for_slot(
        slot: RpmScriptletSlot,
        primary: NativeLifecyclePath,
    ) -> Vec<NativeLifecyclePath> {
        match slot {
            RpmScriptletSlot::Pre => vec![
                NativeLifecyclePath::PreInstall,
                NativeLifecyclePath::PreUpgrade,
            ],
            RpmScriptletSlot::Post => vec![
                NativeLifecyclePath::PostInstall,
                NativeLifecyclePath::PostUpgrade,
            ],
            RpmScriptletSlot::PreUn => vec![
                NativeLifecyclePath::PreRemove,
                NativeLifecyclePath::PreUpgrade,
            ],
            RpmScriptletSlot::PostUn => vec![
                NativeLifecyclePath::PostRemove,
                NativeLifecyclePath::PostUpgrade,
            ],
            _ => vec![primary],
        }
    }

    fn rpm_order_for_lifecycle(lifecycle: NativeLifecyclePath) -> NativeTransactionOrder {
        NativeTransactionOrder::new(match lifecycle {
            NativeLifecyclePath::PreInstall
            | NativeLifecyclePath::PreUpgrade
            | NativeLifecyclePath::PreRemove => NativeTransactionPosition::BeforePayload,
            NativeLifecyclePath::PostInstall
            | NativeLifecyclePath::PostUpgrade
            | NativeLifecyclePath::PostRemove => NativeTransactionPosition::AfterPayload,
            NativeLifecyclePath::PreTransaction => NativeTransactionPosition::BeforeTransaction,
            NativeLifecyclePath::PostTransaction => NativeTransactionPosition::AfterTransaction,
            NativeLifecyclePath::PreUntransaction | NativeLifecyclePath::PostUntransaction => {
                NativeTransactionPosition::Untransaction
            }
            NativeLifecyclePath::Verify => NativeTransactionPosition::Verification,
            _ => NativeTransactionPosition::ControlArtifact,
        })
    }

    fn add_rpm_scriptlet_entry(
        entries: &mut Vec<NativeScriptletEntry>,
        native_slot: &str,
        slot: RpmScriptletSlot,
        lifecycle: NativeLifecyclePath,
        compatibility_phase: Option<ScriptletPhase>,
        scriptlet: std::result::Result<rpm::Scriptlet, rpm::Error>,
    ) {
        let Ok(scriptlet) = scriptlet else {
            return;
        };
        if scriptlet.script.trim().is_empty() {
            return;
        }

        let (interpreter, interpreter_args) = Self::rpm_scriptlet_program(&scriptlet);
        let support = if slot == RpmScriptletSlot::Verify {
            NativeScriptletSupport::DeferredReview {
                reason_code: "rpm-verify-scriptlet-deferred".to_string(),
            }
        } else {
            NativeScriptletSupport::Parsed
        };

        entries.push(NativeScriptletEntry {
            id: format!("rpm:{native_slot}"),
            format: NativeScriptletFormat::Rpm,
            kind: NativeScriptletKind::Executable,
            native_slot: native_slot.to_string(),
            primary_lifecycle: lifecycle,
            compatibility_phase,
            lifecycle_paths: Self::rpm_lifecycle_paths_for_slot(slot, lifecycle),
            interpreter: Some(interpreter),
            interpreter_args,
            body: NativeScriptletBody::from_bytes(scriptlet.script.as_bytes().to_vec()),
            invocation: Self::rpm_scriptlet_invocation(),
            order: Self::rpm_order_for_lifecycle(lifecycle),
            support,
            metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
                slot,
                scriptlet_flags: scriptlet.flags.map(Self::rpm_scriptlet_flags_metadata),
                trigger: None,
            }),
        });
    }

    fn rpm_trigger_invocation(
        family: RpmTriggerFamily,
        action: RpmTriggerAction,
    ) -> NativeInvocationContract {
        let mut args = vec![NativeArgumentContract {
            index: 1,
            name: "triggered-package-count".to_string(),
            value: NativeArgumentValue::TriggerCount,
            required: true,
        }];
        if family != RpmTriggerFamily::TransactionFile {
            args.push(NativeArgumentContract {
                index: 2,
                name: "triggering-package-count".to_string(),
                value: NativeArgumentValue::TriggerCount,
                required: true,
            });
        }

        NativeInvocationContract {
            args,
            environment: Vec::new(),
            stdin: Self::rpm_trigger_stdin(family, action),
            root: NativeRootExpectation::PackageManagerDefault,
        }
    }

    fn rpm_trigger_stdin(
        family: RpmTriggerFamily,
        action: RpmTriggerAction,
    ) -> NativeStdinContract {
        match (family, action) {
            (RpmTriggerFamily::File, _) => NativeStdinContract::Paths,
            (RpmTriggerFamily::TransactionFile, RpmTriggerAction::Install)
            | (RpmTriggerFamily::TransactionFile, RpmTriggerAction::Uninstall) => {
                NativeStdinContract::Paths
            }
            (RpmTriggerFamily::TransactionFile, RpmTriggerAction::PostUninstall) => {
                NativeStdinContract::None
            }
            _ => NativeStdinContract::None,
        }
    }

    fn rpm_trigger_order(
        family: RpmTriggerFamily,
        action: RpmTriggerAction,
    ) -> NativeTransactionOrder {
        NativeTransactionOrder::new(match (family, action) {
            (RpmTriggerFamily::TransactionFile, RpmTriggerAction::Uninstall) => {
                NativeTransactionPosition::BeforeTransaction
            }
            (RpmTriggerFamily::TransactionFile, RpmTriggerAction::Install)
            | (RpmTriggerFamily::TransactionFile, RpmTriggerAction::PostUninstall) => {
                NativeTransactionPosition::AfterTransaction
            }
            _ => NativeTransactionPosition::Trigger,
        })
    }

    fn add_rpm_triggers(
        entries: &mut Vec<NativeScriptletEntry>,
        family: RpmTriggerFamily,
        triggers: std::result::Result<Vec<rpm::Trigger>, rpm::Error>,
    ) {
        let Ok(triggers) = triggers else {
            return;
        };

        for (trigger_index, trigger) in triggers.into_iter().enumerate() {
            let native_slot = Self::rpm_trigger_slot_name(family, &trigger);
            let primary_action = trigger
                .conditions
                .first()
                .map(|condition| Self::rpm_trigger_action(condition.flags))
                .unwrap_or(RpmTriggerAction::Unknown { raw_flags: 0 });
            let mut program = trigger.program.clone().into_iter();
            let interpreter = program.next().unwrap_or_else(|| "/bin/sh".to_string());
            let interpreter_args = program.collect();
            let lifecycle = match family {
                RpmTriggerFamily::Package => NativeLifecyclePath::Trigger,
                RpmTriggerFamily::File => NativeLifecyclePath::FileTrigger,
                RpmTriggerFamily::TransactionFile => NativeLifecyclePath::TransactionFileTrigger,
            };
            let file_globs = if family == RpmTriggerFamily::File
                || family == RpmTriggerFamily::TransactionFile
            {
                trigger
                    .conditions
                    .iter()
                    .map(|condition| condition.name.clone())
                    .collect()
            } else {
                Vec::new()
            };
            let conditions = trigger
                .conditions
                .iter()
                .map(|condition| RpmTriggerCondition {
                    name: condition.name.clone(),
                    action: Self::rpm_trigger_action(condition.flags),
                    version: if condition.version.is_empty() {
                        None
                    } else {
                        Some(condition.version.clone())
                    },
                    comparison: Self::rpm_trigger_comparison(condition.flags),
                    raw_flags: condition.flags.bits(),
                })
                .collect();

            entries.push(NativeScriptletEntry {
                id: format!("rpm:{native_slot}:{trigger_index}"),
                format: NativeScriptletFormat::Rpm,
                kind: NativeScriptletKind::Executable,
                native_slot,
                primary_lifecycle: lifecycle,
                compatibility_phase: None,
                lifecycle_paths: vec![lifecycle],
                interpreter: Some(interpreter),
                interpreter_args,
                body: NativeScriptletBody::from_bytes(trigger.script.as_bytes().to_vec()),
                invocation: Self::rpm_trigger_invocation(family, primary_action),
                order: Self::rpm_trigger_order(family, primary_action),
                support: NativeScriptletSupport::DeferredReview {
                    reason_code: match family {
                        RpmTriggerFamily::Package => "rpm-trigger-semantics-deferred",
                        RpmTriggerFamily::File => "rpm-file-trigger-semantics-deferred",
                        RpmTriggerFamily::TransactionFile => {
                            "rpm-trans-file-trigger-semantics-deferred"
                        }
                    }
                    .to_string(),
                },
                metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
                    slot: RpmScriptletSlot::Trigger,
                    scriptlet_flags: None,
                    trigger: Some(RpmTriggerMetadata {
                        family,
                        conditions,
                        file_globs,
                    }),
                }),
            });
        }
    }

    fn rpm_trigger_slot_name(family: RpmTriggerFamily, trigger: &rpm::Trigger) -> String {
        let action = trigger
            .conditions
            .first()
            .map(|condition| Self::rpm_trigger_action(condition.flags));
        match (family, action) {
            (RpmTriggerFamily::Package, Some(RpmTriggerAction::PreInstall)) => "%triggerprein",
            (RpmTriggerFamily::Package, Some(RpmTriggerAction::Install)) => "%triggerin",
            (RpmTriggerFamily::Package, Some(RpmTriggerAction::Uninstall)) => "%triggerun",
            (RpmTriggerFamily::Package, Some(RpmTriggerAction::PostUninstall)) => "%triggerpostun",
            (RpmTriggerFamily::File, Some(RpmTriggerAction::Install)) => "%filetriggerin",
            (RpmTriggerFamily::File, Some(RpmTriggerAction::Uninstall)) => "%filetriggerun",
            (RpmTriggerFamily::File, Some(RpmTriggerAction::PostUninstall)) => "%filetriggerpostun",
            (RpmTriggerFamily::TransactionFile, Some(RpmTriggerAction::Install)) => {
                "%transfiletriggerin"
            }
            (RpmTriggerFamily::TransactionFile, Some(RpmTriggerAction::Uninstall)) => {
                "%transfiletriggerun"
            }
            (RpmTriggerFamily::TransactionFile, Some(RpmTriggerAction::PostUninstall)) => {
                "%transfiletriggerpostun"
            }
            _ => "%trigger",
        }
        .to_string()
    }

    /// Extract file list from RPM package with detailed metadata
    fn extract_files(pkg: &Package) -> Vec<PackageFile> {
        let mut files = Vec::new();

        // Use get_file_entries() to get complete file metadata
        if let Ok(file_entries) = pkg.metadata.get_file_entries() {
            for entry in file_entries {
                // FileDigest can be formatted as hex string
                let sha256 = entry.digest().map(|d| format!("{}", d));

                let symlink_target = entry.linkto().map(str::to_string);
                files.push(PackageFile {
                    path: entry.path().to_string_lossy().to_string(),
                    size: i64::try_from(entry.size()).unwrap_or(i64::MAX),
                    mode: entry.mode().raw_mode() as i32,
                    sha256,
                    symlink_target,
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
                if entry.flags().contains(FileFlags::CONFIG) {
                    config_files.push(ConfigFileInfo {
                        path: entry.path().to_string_lossy().to_string(),
                        noreplace: entry.flags().contains(FileFlags::NOREPLACE),
                        ghost: entry.flags().contains(FileFlags::GHOST),
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
                // Skip RPM-internal requirements that do not represent
                // installable package dependencies.
                if is_ignored_rpm_requirement_name(&req.name) {
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

    /// Extract native provides from RPM package metadata.
    fn extract_provides(pkg: &Package) -> Vec<Dependency> {
        let mut provides = Vec::new();

        if let Ok(entries) = pkg.metadata.get_provides() {
            for entry in entries {
                if entry.name.starts_with("rpmlib(") {
                    continue;
                }

                let version = if !entry.version.is_empty() {
                    let operator = flags_to_operator(entry.flags);
                    Some(format!("{}{}", operator, entry.version))
                } else {
                    None
                };

                provides.push(Dependency {
                    name: entry.name.to_string(),
                    version,
                    dep_type: DependencyType::Runtime,
                    description: None,
                });
            }
        }

        provides
    }
}

fn is_ignored_rpm_requirement_name(name: &str) -> bool {
    name.starts_with("rpmlib(") || name.starts_with("config(") || name.starts_with('/')
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

        // Combine version and release (e.g., "2.2.1" + "2.fc44" -> "2.2.1-2.fc44")
        let version = if let Ok(release) = pkg.metadata.get_release() {
            format!("{}-{}", version, release)
        } else {
            version
        };

        let architecture = pkg
            .metadata
            .get_arch()
            .ok()
            .map(|s| crate::packages::common::normalize_architecture(s).to_string());
        let description = pkg.metadata.get_description().ok().map(|s| s.to_string());

        // Extract provenance information
        let source_rpm = pkg.metadata.get_source_rpm().ok().map(|s| s.to_string());
        let build_host = pkg.metadata.get_build_host().ok().map(|s| s.to_string());
        let vendor = pkg.metadata.get_vendor().ok().map(|s| s.to_string());
        let license = pkg.metadata.get_license().ok().map(|s| s.to_string());
        let url = pkg.metadata.get_url().ok().map(|s| s.to_string());

        let files = Self::extract_files(&pkg);
        let dependencies = Self::extract_dependencies(&pkg);
        let provides = Self::extract_provides(&pkg);

        // Extract scriptlets and config files using package metadata
        let scriptlets = Self::extract_scriptlets(&pkg);
        let native_scriptlet_abi = Self::extract_native_scriptlet_abi(&pkg);
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
            provides,
            scriptlets,
            native_scriptlet_abi,
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

    fn provides(&self) -> &[Dependency] {
        self.meta.provides()
    }

    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>> {
        debug!(
            "Extracting file contents from RPM: {:?}",
            self.meta.package_path()
        );

        // Check file size before reading (2 GiB limit to prevent OOM on malicious files)
        const MAX_RPM_FILE_SIZE: u64 = 2 * 1024 * 1024 * 1024;
        let file_meta = std::fs::metadata(self.meta.package_path())
            .map_err(|e| Error::InitError(format!("Failed to stat RPM file: {}", e)))?;
        if file_meta.len() > MAX_RPM_FILE_SIZE {
            return Err(Error::InitError(format!(
                "RPM file too large ({} bytes, max {} bytes)",
                file_meta.len(),
                MAX_RPM_FILE_SIZE
            )));
        }

        // Read entire file into memory to avoid TOCTOU (file could change between
        // metadata parse and content extraction)
        let data = std::fs::read(self.meta.package_path())
            .map_err(|e| Error::InitError(format!("Failed to read RPM file: {}", e)))?;
        let mut cursor = std::io::Cursor::new(&data);

        // Parse the package - this gives us access to the payload content
        let pkg = Package::parse(&mut cursor)
            .map_err(|e| Error::InitError(format!("Failed to parse RPM: {}", e)))?;

        // Get the compressed payload from the Package struct
        let payload = &pkg.payload;
        if payload.is_empty() {
            // Check if we expected files from metadata
            let expected_file_count = self
                .meta
                .files
                .iter()
                .filter(|f| is_regular_file_mode(f.mode as u32)) // Regular files only
                .count();
            if expected_file_count > 0 {
                tracing::warn!(
                    "RPM {} has empty payload but metadata declares {} files - package may be corrupted",
                    self.meta.name,
                    expected_file_count
                );
            } else {
                debug!(
                    "RPM {} has empty payload (meta-package with no files)",
                    self.meta.name
                );
            }
            return Ok(Vec::new());
        }

        // Detect compression from payload magic bytes
        let format = CompressionFormat::from_magic_bytes(payload);
        debug!("Detected payload compression: {}", format);

        // Create decompressor from the payload
        let cursor = std::io::Cursor::new(payload);
        let decoder =
            compression::create_decoder_limited(cursor, format, compression::MAX_DECOMPRESS_SIZE)
                .map_err(|e| Error::InitError(format!("Failed to create decoder: {}", e)))?;

        // Map paths to metadata for O(1) lookup
        let file_map: HashMap<&str, &PackageFile> = self
            .meta
            .files
            .iter()
            .map(|f| (f.path.as_str(), f))
            .collect();

        // Extract CPIO archive
        let mut cpio = CpioReader::new(decoder);
        let mut extracted_files = Vec::new();

        while let Some((entry, content)) = cpio
            .next_entry()
            .map_err(|e| Error::InitError(format!("CPIO error: {}", e)))?
        {
            let is_symlink = (entry.mode & 0o170000) == 0o120000;
            let is_regular = is_regular_file_mode(entry.mode);

            if !is_regular && !is_symlink {
                continue;
            }

            // Check file size using shared utility (symlinks are small)
            if is_regular && !check_file_size(&entry.name, entry.size) {
                continue;
            }

            // Normalize path using shared utility
            let abs_path = normalize_path(&entry.name)
                .map_err(|e| Error::InitError(format!("Path normalization failed: {}", e)))?;

            // Match with metadata to get SHA256 and confirm it's a tracked file
            if let Some(meta) = file_map.get(abs_path.as_str()) {
                let symlink_target = if is_symlink {
                    // In CPIO, the symlink target is stored as the file content
                    Some(String::from_utf8_lossy(&content).into_owned())
                } else {
                    meta.symlink_target.clone()
                };

                extracted_files.push(ExtractedFile {
                    path: abs_path,
                    content: if is_symlink { Vec::new() } else { content },
                    size: i64::try_from(entry.size).unwrap_or(i64::MAX),
                    mode: entry.mode as i32,
                    sha256: meta.sha256.clone(),
                    symlink_target,
                });
            }
        }

        // Validate extraction completeness
        let expected_regular_files = self
            .meta
            .files
            .iter()
            .filter(|f| is_regular_file_mode(f.mode as u32)) // Regular files only
            .count();

        if extracted_files.len() < expected_regular_files {
            let missing = expected_regular_files - extracted_files.len();
            tracing::warn!(
                "RPM {} extracted {} files but metadata declares {} regular files ({} missing)",
                self.meta.name,
                extracted_files.len(),
                expected_regular_files,
                missing
            );
        }

        debug!("Extracted {} files from RPM", extracted_files.len());
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
    use crate::packages::traits::{
        NativeArgumentValue, NativeLifecyclePath, NativeScriptletMetadata, NativeStdinContract,
        RpmTriggerAction,
    };

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

    #[test]
    fn ignores_rpm_internal_requirement_names() {
        assert!(is_ignored_rpm_requirement_name(
            "rpmlib(CompressedFileNames)"
        ));
        assert!(is_ignored_rpm_requirement_name(
            "config(phase4-runtime-fixture)"
        ));
        assert!(is_ignored_rpm_requirement_name("/bin/sh"));
        assert!(!is_ignored_rpm_requirement_name("openssl-libs"));
    }

    #[test]
    fn rpm_program_vector_splits_interpreter_and_args() {
        let scriptlet = rpm::Scriptlet::new("echo posttrans").prog(vec![
            "/usr/lib/rpm/lua",
            "--",
            "script.lua",
        ]);

        let (interpreter, args) = RpmPackage::rpm_scriptlet_program(&scriptlet);

        assert_eq!(interpreter, "/usr/lib/rpm/lua");
        assert_eq!(args, vec!["--".to_string(), "script.lua".to_string()]);
    }

    #[test]
    fn rpm_scriptlet_flags_preserve_names_and_bits() {
        let flags = RpmPackage::rpm_scriptlet_flags_metadata(rpm::ScriptletFlags::EXPAND);

        assert!(flags.names.contains(&"EXPAND".to_string()));
        assert_eq!(flags.raw_bits, rpm::ScriptletFlags::EXPAND.bits());
    }

    #[test]
    fn rpm_trigger_action_and_comparison_are_split_from_raw_flags() {
        let flags = rpm::DependencyFlags::TRIGGERPOSTUN | rpm::DependencyFlags::LE;

        assert_eq!(
            RpmPackage::rpm_trigger_action(flags),
            RpmTriggerAction::PostUninstall
        );
        assert_eq!(
            RpmPackage::rpm_trigger_comparison(flags),
            Some("<=".to_string())
        );

        let unknown = RpmPackage::rpm_trigger_action(rpm::DependencyFlags::from_bits_retain(0));
        assert_eq!(
            unknown,
            RpmTriggerAction::Unknown {
                raw_flags: rpm::DependencyFlags::from_bits_retain(0).bits(),
            }
        );
    }

    #[test]
    fn rpm_native_abi_preserves_untransaction_verify_and_all_trigger_actions() {
        let mut builder = rpm::PackageBuilder::new(
            "native-abi-fixture",
            "1.0.0",
            "MIT",
            "x86_64",
            "native abi fixture",
        );
        builder
            .pre_install_script("echo pre")
            .post_install_script("echo post")
            .pre_uninstall_script("echo preun")
            .post_uninstall_script("echo postun")
            .pre_trans_script("echo pretrans")
            .post_trans_script("echo posttrans")
            .pre_untrans_script("echo preuntrans")
            .post_untrans_script("echo postuntrans")
            .verify_script("echo verify")
            .trigger_prein("bash", None, "echo triggerprein")
            .trigger_in(
                "bash",
                Some((rpm::DependencyFlags::GREATER, "5.0")),
                "echo triggerin",
            )
            .trigger_un("bash", None, "echo triggerun")
            .trigger_postun("bash", None, "echo triggerpostun")
            .file_trigger_in("/usr/lib", None, "echo filetriggerin")
            .file_trigger_un("/usr/lib", None, "echo filetriggerun")
            .file_trigger_postun("/usr/lib", None, "echo filetriggerpostun")
            .trans_file_trigger_in("/usr/bin", None, "echo transfiletriggerin")
            .trans_file_trigger_un("/usr/bin", None, "echo transfiletriggerun")
            .trans_file_trigger_postun("/usr/bin", None, "echo transfiletriggerpostun");

        let package = builder.build().expect("fixture rpm package");
        let entries = RpmPackage::extract_native_scriptlet_abi(&package);
        let slots: Vec<_> = entries
            .iter()
            .map(|entry| entry.native_slot.as_str())
            .collect();

        for slot in [
            "%pre",
            "%post",
            "%preun",
            "%postun",
            "%pretrans",
            "%posttrans",
            "%preuntrans",
            "%postuntrans",
            "%verify",
            "%triggerprein",
            "%triggerin",
            "%triggerun",
            "%triggerpostun",
            "%filetriggerin",
            "%filetriggerun",
            "%filetriggerpostun",
            "%transfiletriggerin",
            "%transfiletriggerun",
            "%transfiletriggerpostun",
        ] {
            assert!(slots.contains(&slot), "missing native slot {slot}");
        }

        let verify = entries
            .iter()
            .find(|entry| entry.native_slot == "%verify")
            .expect("verify entry");
        assert_eq!(verify.primary_lifecycle, NativeLifecyclePath::Verify);
        assert_eq!(verify.compatibility_phase, None);
        assert_eq!(
            verify.support.reason_code(),
            Some("rpm-verify-scriptlet-deferred")
        );

        let pre = entries
            .iter()
            .find(|entry| entry.native_slot == "%pre")
            .expect("pre entry");
        assert_eq!(
            pre.lifecycle_paths,
            vec![
                NativeLifecyclePath::PreInstall,
                NativeLifecyclePath::PreUpgrade
            ]
        );
        assert_eq!(
            pre.invocation.args[0].value,
            NativeArgumentValue::PackageInstanceCount
        );

        let preun = entries
            .iter()
            .find(|entry| entry.native_slot == "%preun")
            .expect("preun entry");
        assert_eq!(
            preun.lifecycle_paths,
            vec![
                NativeLifecyclePath::PreRemove,
                NativeLifecyclePath::PreUpgrade
            ]
        );

        let file_trigger = entries
            .iter()
            .find(|entry| entry.native_slot == "%filetriggerin")
            .expect("file trigger entry");
        assert_eq!(file_trigger.invocation.stdin, NativeStdinContract::Paths);
        let NativeScriptletMetadata::Rpm(meta) = &file_trigger.metadata else {
            panic!("expected rpm metadata");
        };
        let trigger = meta.trigger.as_ref().expect("trigger metadata");
        assert_eq!(trigger.file_globs, vec!["/usr/lib".to_string()]);

        let trans_postun = entries
            .iter()
            .find(|entry| entry.native_slot == "%transfiletriggerpostun")
            .expect("transaction file trigger postun entry");
        assert_eq!(trans_postun.invocation.stdin, NativeStdinContract::None);
        assert_eq!(trans_postun.invocation.args.len(), 1);
        let NativeScriptletMetadata::Rpm(meta) = &trans_postun.metadata else {
            panic!("expected rpm metadata");
        };
        assert_eq!(
            meta.trigger.as_ref().expect("trigger metadata").file_globs,
            vec!["/usr/bin".to_string()]
        );
    }
}
