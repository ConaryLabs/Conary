// conary-core/src/packages/rpm/scriptlets.rs

use super::*;
use crate::scriptlet::RpmClassAuthority;

mod runtime_context;
use runtime_context::ParsedRpmRuntimeContext;

impl RpmPackage {
    pub(super) fn rpm_scriptlet_program(scriptlet: &rpm::Scriptlet) -> (String, Vec<String>) {
        let mut program = scriptlet.program.clone().unwrap_or_default().into_iter();
        let interpreter = program.next().unwrap_or_else(|| "/bin/sh".to_string());
        let args = program.collect();
        (interpreter, args)
    }

    pub(super) fn rpm_scriptlet_flags_metadata(
        flags: rpm::ScriptletFlags,
        authority: RpmClassAuthority,
    ) -> RpmScriptletFlagsMetadata {
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

        let known_bits = rpm::ScriptletFlags::EXPAND.bits()
            | rpm::ScriptletFlags::QFORMAT.bits()
            | rpm::ScriptletFlags::CRITICAL.bits();
        let header_critical = flags.contains(rpm::ScriptletFlags::CRITICAL);
        let criticality = authority.effective_criticality(header_critical);

        RpmScriptletFlagsMetadata {
            names,
            raw_bits: flags.bits(),
            unknown_bits: flags.bits() & !known_bits,
            expand: flags.contains(rpm::ScriptletFlags::EXPAND),
            query_format: flags.contains(rpm::ScriptletFlags::QFORMAT),
            critical: criticality.is_critical(),
            criticality,
        }
    }

    fn rpm_runtime_metadata(
        interpreter: &str,
        flags: rpm::ScriptletFlags,
        authority: RpmClassAuthority,
        install_prefixes: &[String],
        context: &ParsedRpmRuntimeContext,
    ) -> RpmScriptletRuntimeMetadata {
        let needs_macros = flags.contains(rpm::ScriptletFlags::EXPAND) || interpreter == "<lua>";
        RpmScriptletRuntimeMetadata {
            program: if interpreter == "<lua>" {
                RpmScriptletProgram::EmbeddedLua
            } else {
                RpmScriptletProgram::External
            },
            flags: Self::rpm_scriptlet_flags_metadata(flags, authority),
            install_prefixes: install_prefixes.to_vec(),
            macro_context: if needs_macros {
                context.macros.clone()
            } else {
                Default::default()
            },
            header_context: if flags.contains(rpm::ScriptletFlags::QFORMAT) {
                context.header.clone()
            } else {
                Default::default()
            },
            package_rpm_version: context.package_rpm_version.clone(),
        }
    }

    pub(super) fn rpm_trigger_action(flags: rpm::DependencyFlags) -> RpmTriggerAction {
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

    pub(super) fn rpm_trigger_comparison(flags: rpm::DependencyFlags) -> Option<String> {
        let operator = flags_to_operator(flags).trim();
        if operator.is_empty() {
            None
        } else {
            Some(operator.to_string())
        }
    }

    pub(super) fn extract_native_scriptlet_abi(pkg: &Package) -> Result<Vec<NativeScriptletEntry>> {
        let mut entries = Vec::new();
        let install_prefixes = Self::rpm_install_prefixes(&pkg.metadata)?;
        let runtime_context = Self::rpm_runtime_context(&pkg.metadata)?;

        Self::add_rpm_sysusers_entries(
            &mut entries,
            Self::extract_rpm_sysusers(pkg)?,
            &install_prefixes,
            &runtime_context,
        );

        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%pre",
            RpmScriptletSlot::Pre,
            NativeLifecyclePath::PreInstall,
            pkg.metadata.get_pre_install_script(),
            &install_prefixes,
            &runtime_context,
        )?;
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%post",
            RpmScriptletSlot::Post,
            NativeLifecyclePath::PostInstall,
            pkg.metadata.get_post_install_script(),
            &install_prefixes,
            &runtime_context,
        )?;
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%preun",
            RpmScriptletSlot::PreUn,
            NativeLifecyclePath::PreRemove,
            pkg.metadata.get_pre_uninstall_script(),
            &install_prefixes,
            &runtime_context,
        )?;
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%postun",
            RpmScriptletSlot::PostUn,
            NativeLifecyclePath::PostRemove,
            pkg.metadata.get_post_uninstall_script(),
            &install_prefixes,
            &runtime_context,
        )?;
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%pretrans",
            RpmScriptletSlot::PreTrans,
            NativeLifecyclePath::PreTransaction,
            pkg.metadata.get_pre_trans_script(),
            &install_prefixes,
            &runtime_context,
        )?;
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%posttrans",
            RpmScriptletSlot::PostTrans,
            NativeLifecyclePath::PostTransaction,
            pkg.metadata.get_post_trans_script(),
            &install_prefixes,
            &runtime_context,
        )?;
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%preuntrans",
            RpmScriptletSlot::PreUnTrans,
            NativeLifecyclePath::PreUntransaction,
            pkg.metadata.get_pre_untrans_script(),
            &install_prefixes,
            &runtime_context,
        )?;
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%postuntrans",
            RpmScriptletSlot::PostUnTrans,
            NativeLifecyclePath::PostUntransaction,
            pkg.metadata.get_post_untrans_script(),
            &install_prefixes,
            &runtime_context,
        )?;
        Self::add_rpm_scriptlet_entry(
            &mut entries,
            "%verify",
            RpmScriptletSlot::Verify,
            NativeLifecyclePath::Verify,
            pkg.metadata.get_verify_script(),
            &install_prefixes,
            &runtime_context,
        )?;

        Self::add_rpm_triggers(
            &mut entries,
            RpmTriggerFamily::Package,
            pkg.metadata.get_triggers(),
            &[],
            &Self::rpm_trigger_flags(&pkg.metadata, rpm::IndexTag::RPMTAG_TRIGGERSCRIPTFLAGS)?,
            &install_prefixes,
            &runtime_context,
        )?;
        let file_trigger_priorities = Self::rpm_trigger_priorities(
            &pkg.metadata,
            rpm::IndexTag::RPMTAG_FILETRIGGERPRIORITIES,
        )?;
        Self::add_rpm_triggers(
            &mut entries,
            RpmTriggerFamily::File,
            pkg.metadata.get_file_triggers(),
            &file_trigger_priorities,
            &Self::rpm_trigger_flags(&pkg.metadata, rpm::IndexTag::RPMTAG_FILETRIGGERSCRIPTFLAGS)?,
            &install_prefixes,
            &runtime_context,
        )?;
        let transaction_file_trigger_priorities = Self::rpm_trigger_priorities(
            &pkg.metadata,
            rpm::IndexTag::RPMTAG_TRANSFILETRIGGERPRIORITIES,
        )?;
        Self::add_rpm_triggers(
            &mut entries,
            RpmTriggerFamily::TransactionFile,
            pkg.metadata.get_trans_file_triggers(),
            &transaction_file_trigger_priorities,
            &Self::rpm_trigger_flags(
                &pkg.metadata,
                rpm::IndexTag::RPMTAG_TRANSFILETRIGGERSCRIPTFLAGS,
            )?,
            &install_prefixes,
            &runtime_context,
        )?;

        Ok(entries)
    }

    fn rpm_scriptlet_invocation(install_prefixes: &[String]) -> NativeInvocationContract {
        NativeInvocationContract {
            args: vec![NativeArgumentContract {
                index: 1,
                name: "package-instance-count".to_string(),
                value: NativeArgumentValue::PackageInstanceCount,
                required: true,
            }],
            environment: Self::rpm_prefix_environment(install_prefixes),
            stdin: NativeStdinContract::None,
            root: NativeRootExpectation::PackageManagerDefault,
        }
    }

    fn rpm_prefix_environment(install_prefixes: &[String]) -> Vec<NativeEnvironmentFact> {
        let mut environment = install_prefixes
            .iter()
            .enumerate()
            .map(|(index, prefix)| NativeEnvironmentFact {
                name: format!("RPM_INSTALL_PREFIX{index}"),
                value: Some(prefix.clone()),
            })
            .collect::<Vec<_>>();
        if let Some(prefix) = install_prefixes.first() {
            environment.push(NativeEnvironmentFact {
                name: "RPM_INSTALL_PREFIX".to_string(),
                value: Some(prefix.clone()),
            });
        }
        environment
    }

    fn rpm_install_prefixes(metadata: &rpm::PackageMetadata) -> Result<Vec<String>> {
        match metadata
            .header
            .get_entry_data_as_string_array(rpm::IndexTag::RPMTAG_PREFIXES)
        {
            Ok(prefixes) => Ok(prefixes.into_iter().map(str::to_string).collect()),
            Err(rpm::Error::TagNotFound(_)) => Ok(Vec::new()),
            Err(error) => Err(Error::ParseError(format!(
                "Failed to read RPM install prefixes: {error}"
            ))),
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
        scriptlet: std::result::Result<rpm::Scriptlet, rpm::Error>,
        install_prefixes: &[String],
        runtime_context: &ParsedRpmRuntimeContext,
    ) -> Result<()> {
        let scriptlet = match scriptlet {
            Ok(scriptlet) => scriptlet,
            Err(rpm::Error::TagNotFound(_)) => return Ok(()),
            Err(error) => {
                return Err(Error::ParseError(format!(
                    "Failed to read RPM {native_slot} scriptlet: {error}"
                )));
            }
        };
        if scriptlet.script.trim().is_empty() {
            return Ok(());
        }

        let (interpreter, interpreter_args) = Self::rpm_scriptlet_program(&scriptlet);
        let flags = scriptlet
            .flags
            .unwrap_or_else(|| rpm::ScriptletFlags::from_bits_retain(0));
        let runtime = Self::rpm_runtime_metadata(
            &interpreter,
            flags,
            crate::scriptlet::rpm_package_slot_authority(slot),
            install_prefixes,
            runtime_context,
        );
        entries.push(NativeScriptletEntry {
            id: format!("rpm:{native_slot}"),
            format: NativeScriptletFormat::Rpm,
            kind: NativeScriptletKind::Executable,
            native_slot: native_slot.to_string(),
            primary_lifecycle: lifecycle,
            lifecycle_paths: Self::rpm_lifecycle_paths_for_slot(slot, lifecycle),
            interpreter: Some(interpreter),
            interpreter_args,
            body: NativeScriptletBody::from_bytes(scriptlet.script.as_bytes().to_vec()),
            invocation: Self::rpm_scriptlet_invocation(install_prefixes),
            order: Self::rpm_order_for_lifecycle(lifecycle),
            support: NativeScriptletSupport::Parsed,
            metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
                slot,
                runtime,
                trigger: None,
                sysusers: None,
            }),
        });
        Ok(())
    }

    fn add_rpm_sysusers_entries(
        entries: &mut Vec<NativeScriptletEntry>,
        groups: Vec<RpmSysusersMetadata>,
        install_prefixes: &[String],
        runtime_context: &ParsedRpmRuntimeContext,
    ) {
        for (index, metadata) in groups.into_iter().enumerate() {
            let mut interpreter_args = Vec::new();
            if let Some(source_path) = &metadata.source_path {
                interpreter_args.push("--replace".to_string());
                interpreter_args.push(source_path.clone());
            }
            interpreter_args.push("-".to_string());
            let mut body = metadata.lines.join("\n").into_bytes();
            body.push(b'\n');
            entries.push(NativeScriptletEntry {
                id: format!("rpm:%sysusers:{index}"),
                format: NativeScriptletFormat::Rpm,
                kind: NativeScriptletKind::Executable,
                native_slot: "%sysusers".to_string(),
                primary_lifecycle: NativeLifecyclePath::Sysusers,
                lifecycle_paths: vec![NativeLifecyclePath::Sysusers],
                interpreter: Some("/usr/bin/systemd-sysusers".to_string()),
                interpreter_args,
                body: NativeScriptletBody::from_bytes(body),
                invocation: NativeInvocationContract {
                    args: Vec::new(),
                    environment: Self::rpm_prefix_environment(install_prefixes),
                    stdin: NativeStdinContract::Sysusers,
                    root: NativeRootExpectation::PackageManagerDefault,
                },
                order: NativeTransactionOrder::new(NativeTransactionPosition::BeforePayload),
                support: NativeScriptletSupport::Parsed,
                metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
                    slot: RpmScriptletSlot::Sysusers,
                    runtime: RpmScriptletRuntimeMetadata {
                        program: RpmScriptletProgram::Sysusers,
                        flags: Self::rpm_scriptlet_flags_metadata(
                            rpm::ScriptletFlags::from_bits_retain(0),
                            crate::scriptlet::rpm_package_slot_authority(
                                RpmScriptletSlot::Sysusers,
                            ),
                        ),
                        install_prefixes: install_prefixes.to_vec(),
                        macro_context: RpmMacroContextMetadata::default(),
                        header_context: RpmHeaderContextMetadata::default(),
                        package_rpm_version: runtime_context.package_rpm_version.clone(),
                    },
                    trigger: None,
                    sysusers: Some(metadata),
                }),
            });
        }
    }

    fn rpm_trigger_invocation(
        family: RpmTriggerFamily,
        action: RpmTriggerAction,
        install_prefixes: &[String],
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
            environment: Self::rpm_prefix_environment(install_prefixes),
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
        priorities: &[i32],
        script_flags: &[u32],
        install_prefixes: &[String],
        runtime_context: &ParsedRpmRuntimeContext,
    ) -> Result<()> {
        let triggers = triggers.map_err(|error| {
            Error::InitError(format!("Failed to parse {family:?} RPM triggers: {error}"))
        })?;
        if !script_flags.is_empty() && script_flags.len() != triggers.len() {
            return Err(Error::ParseError(format!(
                "RPM {family:?} trigger script flags count {} does not match script count {}",
                script_flags.len(),
                triggers.len()
            )));
        }

        for (trigger_index, trigger) in triggers.into_iter().enumerate() {
            let native_slot = Self::rpm_trigger_slot_name(family, &trigger);
            let primary_action = trigger
                .conditions
                .first()
                .map(|condition| Self::rpm_trigger_action(condition.flags))
                .unwrap_or(RpmTriggerAction::Unknown { raw_flags: 0 });
            if matches!(primary_action, RpmTriggerAction::Unknown { .. })
                || trigger
                    .conditions
                    .iter()
                    .any(|condition| Self::rpm_trigger_action(condition.flags) != primary_action)
            {
                return Err(Error::InitError(format!(
                    "RPM trigger {trigger_index} has missing, unknown, or mixed action flags"
                )));
            }
            let mut program = trigger.program.clone().into_iter();
            let interpreter = program.next().unwrap_or_else(|| "/bin/sh".to_string());
            let interpreter_args = program.collect();
            let flags = rpm::ScriptletFlags::from_bits_retain(
                script_flags.get(trigger_index).copied().unwrap_or(0),
            );
            let runtime = Self::rpm_runtime_metadata(
                &interpreter,
                flags,
                crate::scriptlet::rpm_trigger_authority(family, primary_action),
                install_prefixes,
                runtime_context,
            );
            let lifecycle = match family {
                RpmTriggerFamily::Package => NativeLifecyclePath::Trigger,
                RpmTriggerFamily::File => NativeLifecyclePath::FileTrigger,
                RpmTriggerFamily::TransactionFile => NativeLifecyclePath::TransactionFileTrigger,
            };
            let path_prefixes = if family == RpmTriggerFamily::File
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
                lifecycle_paths: vec![lifecycle],
                interpreter: Some(interpreter),
                interpreter_args,
                body: NativeScriptletBody::from_bytes(trigger.script.as_bytes().to_vec()),
                invocation: Self::rpm_trigger_invocation(family, primary_action, install_prefixes),
                order: Self::rpm_trigger_order(family, primary_action),
                support: NativeScriptletSupport::Parsed,
                metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
                    slot: RpmScriptletSlot::Trigger,
                    runtime,
                    trigger: Some(RpmTriggerMetadata {
                        family,
                        conditions,
                        path_prefixes,
                        priority: priorities
                            .get(trigger_index)
                            .copied()
                            .or_else(|| (family == RpmTriggerFamily::File).then_some(100_000)),
                    }),
                    sysusers: None,
                }),
            });
        }
        Ok(())
    }

    fn rpm_trigger_priorities(
        metadata: &rpm::PackageMetadata,
        tag: rpm::IndexTag,
    ) -> Result<Vec<i32>> {
        match metadata.header.get_entry_data_as_u32_array(tag) {
            Ok(values) => values
                .into_iter()
                .map(|value| {
                    i32::try_from(value).map_err(|_| {
                        Error::InitError(format!(
                            "RPM trigger priority {value} does not fit the supported range"
                        ))
                    })
                })
                .collect(),
            Err(rpm::Error::TagNotFound(_)) => Ok(Vec::new()),
            Err(error) => Err(Error::InitError(format!(
                "Failed to parse RPM trigger priorities: {error}"
            ))),
        }
    }

    fn rpm_trigger_flags(metadata: &rpm::PackageMetadata, tag: rpm::IndexTag) -> Result<Vec<u32>> {
        match metadata.header.get_entry_data_as_u32_array(tag) {
            Ok(values) => Ok(values),
            Err(rpm::Error::TagNotFound(_)) => Ok(Vec::new()),
            Err(error) => Err(Error::ParseError(format!(
                "Failed to parse RPM trigger script flags from {tag:?}: {error}"
            ))),
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
}
