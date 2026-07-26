// conary-core/src/ccs/convert/scriptlet_bundle/format_metadata.rs

use crate::ccs::native_lifecycle::{
    ArchHookActionMetadata, ArchHookMetadata, ArchHookOperation as BundleArchHookOperation,
    ArchHookTriggerMetadata, ArchHookTriggerType as BundleArchHookTriggerType, ArchHookWhen,
    ArchInstallMetadata, ArchInstallSelection, DebControlMember as BundleDebControlMember,
    DebMaintainerArgumentMetadata, DebMaintainerArgumentValue as BundleDebMaintainerArgumentValue,
    DebMaintainerInvocationMetadata, DebMaintainerMetadata,
    DebMaintainerMode as BundleDebMaintainerMode, DebTriggerAwaitMode as BundleDebTriggerAwaitMode,
    DebTriggerDirective as BundleDebTriggerDirective, DebTriggerMetadata, RpmBodyTransform,
    RpmCriticality, RpmHeaderContext, RpmHeaderFact, RpmHeaderFactSource, RpmHeaderValue,
    RpmMacroContext, RpmMacroDefinition, RpmMacroDefinitionSource, RpmProgram, RpmRuntimeMetadata,
    RpmSysusersDirective as BundleRpmSysusersDirective,
    RpmSysusersMetadata as BundleRpmSysusersMetadata, RpmTriggerAction as BundleRpmTriggerAction,
    RpmTriggerKind, RpmTriggerMetadata as BundleRpmTriggerMetadata, RpmTriggerTargetConstraint,
};
use crate::packages::native_abi::{
    ArchAlpmHookMetadata, ArchAlpmHookOperation, ArchAlpmHookTriggerType,
    ArchInstallScriptletMetadata, ArchInstallSelectionContract, ArchNativeScriptletMetadata,
    DebControlMember, DebMaintainerMode, DebNativeScriptletMetadata, DebTriggerAwaitMode,
    DebTriggerDirective, NativeArgumentValue, NativeInvocationContract, NativeScriptletEntry,
    NativeScriptletMetadata, NativeStdinContract, RpmHeaderFactSource as NativeRpmHeaderFactSource,
    RpmHeaderValueMetadata as NativeRpmHeaderValueMetadata,
    RpmMacroDefinitionSource as NativeRpmMacroDefinitionSource, RpmNativeScriptletMetadata,
    RpmScriptletCriticality, RpmScriptletProgram,
    RpmSysusersDirective as NativeRpmSysusersDirective, RpmTriggerAction, RpmTriggerFamily,
};
pub(super) struct FormatMetadataProjection {
    pub(super) rpm_runtime: Option<RpmRuntimeMetadata>,
    pub(super) rpm_sysusers: Option<BundleRpmSysusersMetadata>,
    pub(super) rpm_trigger: Option<BundleRpmTriggerMetadata>,
    pub(super) deb_maintainer: Option<DebMaintainerMetadata>,
    pub(super) arch_install: Option<ArchInstallMetadata>,
    pub(super) arch_hook: Option<ArchHookMetadata>,
}

pub(super) fn project_format_metadata(native: &NativeScriptletEntry) -> FormatMetadataProjection {
    match &native.metadata {
        NativeScriptletMetadata::Rpm(metadata) => {
            let (runtime, sysusers, trigger) = project_rpm_metadata(metadata);
            FormatMetadataProjection {
                rpm_runtime: Some(runtime),
                rpm_sysusers: sysusers,
                rpm_trigger: trigger,
                deb_maintainer: None,
                arch_install: None,
                arch_hook: None,
            }
        }
        NativeScriptletMetadata::Deb(metadata) => FormatMetadataProjection {
            rpm_runtime: None,
            rpm_sysusers: None,
            rpm_trigger: None,
            deb_maintainer: Some(project_deb_metadata(metadata, &native.invocation)),
            arch_install: None,
            arch_hook: None,
        },
        NativeScriptletMetadata::DebconfTemplates(_) => FormatMetadataProjection {
            rpm_runtime: None,
            rpm_sysusers: None,
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            arch_hook: None,
        },
        NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(metadata)) => {
            FormatMetadataProjection {
                rpm_runtime: None,
                rpm_sysusers: None,
                rpm_trigger: None,
                deb_maintainer: None,
                arch_install: Some(project_arch_install_metadata(metadata)),
                arch_hook: None,
            }
        }
        NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(metadata)) => {
            FormatMetadataProjection {
                rpm_runtime: None,
                rpm_sysusers: None,
                rpm_trigger: None,
                deb_maintainer: None,
                arch_install: None,
                arch_hook: Some(project_arch_hook_metadata(metadata)),
            }
        }
    }
}

fn project_rpm_metadata(
    metadata: &RpmNativeScriptletMetadata,
) -> (
    RpmRuntimeMetadata,
    Option<BundleRpmSysusersMetadata>,
    Option<BundleRpmTriggerMetadata>,
) {
    let runtime = RpmRuntimeMetadata {
        program: match metadata.runtime.program {
            RpmScriptletProgram::External => RpmProgram::External,
            RpmScriptletProgram::EmbeddedLua => RpmProgram::EmbeddedLua,
            RpmScriptletProgram::Sysusers => RpmProgram::Sysusers,
        },
        body_transforms: [
            metadata
                .runtime
                .flags
                .expand
                .then_some(RpmBodyTransform::MacroExpand),
            metadata
                .runtime
                .flags
                .query_format
                .then_some(RpmBodyTransform::HeaderQueryFormat),
        ]
        .into_iter()
        .flatten()
        .collect(),
        critical: metadata.runtime.flags.critical,
        criticality: match metadata.runtime.flags.criticality {
            RpmScriptletCriticality::Header => RpmCriticality::Header,
            RpmScriptletCriticality::SlotDefault => RpmCriticality::SlotDefault,
            RpmScriptletCriticality::WarningOnly => RpmCriticality::WarningOnly,
            RpmScriptletCriticality::ForcedWarningOnly => RpmCriticality::ForcedWarningOnly,
        },
        raw_flags: metadata.runtime.flags.raw_bits,
        unknown_flags: metadata.runtime.flags.unknown_bits,
        install_prefixes: metadata.runtime.install_prefixes.clone(),
        macro_context: RpmMacroContext {
            definitions: metadata
                .runtime
                .macro_context
                .definitions
                .iter()
                .map(|definition| RpmMacroDefinition {
                    name: definition.name.clone(),
                    body: definition.body.clone(),
                    source: match definition.source {
                        NativeRpmMacroDefinitionSource::PackageHeader => {
                            RpmMacroDefinitionSource::PackageHeader
                        }
                    },
                })
                .collect(),
        },
        header_context: RpmHeaderContext {
            facts: metadata
                .runtime
                .header_context
                .facts
                .iter()
                .map(|fact| RpmHeaderFact {
                    tag: fact.tag,
                    name: fact.name.clone(),
                    value: match &fact.value {
                        NativeRpmHeaderValueMetadata::Null => RpmHeaderValue::Null,
                        NativeRpmHeaderValueMetadata::Binary(value) => {
                            use base64::Engine as _;
                            RpmHeaderValue::Binary(
                                base64::engine::general_purpose::STANDARD.encode(value),
                            )
                        }
                        NativeRpmHeaderValueMetadata::Integer(value) => {
                            RpmHeaderValue::Integer(value.clone())
                        }
                        NativeRpmHeaderValueMetadata::String(value) => {
                            RpmHeaderValue::String(value.clone())
                        }
                        NativeRpmHeaderValueMetadata::StringArray(value) => {
                            RpmHeaderValue::StringArray(value.clone())
                        }
                        NativeRpmHeaderValueMetadata::I18nString(value) => {
                            RpmHeaderValue::I18nString(value.clone())
                        }
                    },
                    source: match fact.source {
                        NativeRpmHeaderFactSource::Header => RpmHeaderFactSource::Header,
                        NativeRpmHeaderFactSource::TransactionDerived => {
                            RpmHeaderFactSource::TransactionDerived
                        }
                    },
                })
                .collect(),
            format_environment: Default::default(),
            database_instance: 0,
        },
        package_rpm_version: metadata.runtime.package_rpm_version.clone(),
    };
    let sysusers = metadata
        .sysusers
        .as_ref()
        .map(|sysusers| BundleRpmSysusersMetadata {
            source_path: sysusers.source_path.clone(),
            lines: sysusers.lines.clone(),
            directives: sysusers
                .directives
                .iter()
                .map(|directive| match directive {
                    NativeRpmSysusersDirective::User {
                        name,
                        id,
                        description,
                        home,
                        shell,
                        locked,
                    } => BundleRpmSysusersDirective::User {
                        name: name.clone(),
                        id: id.clone(),
                        description: description.clone(),
                        home: home.clone(),
                        shell: shell.clone(),
                        locked: *locked,
                    },
                    NativeRpmSysusersDirective::Group { name, id } => {
                        BundleRpmSysusersDirective::Group {
                            name: name.clone(),
                            id: id.clone(),
                        }
                    }
                    NativeRpmSysusersDirective::Member { user, group } => {
                        BundleRpmSysusersDirective::Member {
                            user: user.clone(),
                            group: group.clone(),
                        }
                    }
                })
                .collect(),
        });
    let trigger = metadata
        .trigger
        .as_ref()
        .map(|trigger| BundleRpmTriggerMetadata {
            kind: rpm_trigger_family(trigger.family),
            target_constraints: trigger
                .conditions
                .iter()
                .map(|condition| RpmTriggerTargetConstraint {
                    package: condition.name.clone(),
                    action: rpm_trigger_action(condition.action),
                    operator: condition.comparison.clone(),
                    version: condition.version.clone(),
                    raw_flags: condition.raw_flags,
                })
                .collect(),
            priority: trigger.priority,
            path_prefixes: trigger.path_prefixes.clone(),
        });
    (runtime, sysusers, trigger)
}

fn project_deb_metadata(
    metadata: &DebNativeScriptletMetadata,
    invocation: &NativeInvocationContract,
) -> DebMaintainerMetadata {
    DebMaintainerMetadata {
        control_member: match metadata.control_member {
            DebControlMember::Config => BundleDebControlMember::Config,
            DebControlMember::Preinst => BundleDebControlMember::Preinst,
            DebControlMember::Postinst => BundleDebControlMember::Postinst,
            DebControlMember::Prerm => BundleDebControlMember::Prerm,
            DebControlMember::Postrm => BundleDebControlMember::Postrm,
            DebControlMember::Triggers => BundleDebControlMember::Triggers,
        },
        invocations: metadata
            .maintainer_modes
            .iter()
            .map(|invocation| DebMaintainerInvocationMetadata {
                mode: deb_maintainer_mode(invocation.mode),
                arguments: invocation
                    .args
                    .iter()
                    .map(|argument| {
                        let (value, literal) = deb_argument_value(&argument.value);
                        DebMaintainerArgumentMetadata {
                            index: argument.index,
                            name: argument.name.clone(),
                            value,
                            literal,
                            required: argument.required,
                        }
                    })
                    .collect(),
                lifecycle_paths: invocation
                    .lifecycle_paths
                    .iter()
                    .copied()
                    .map(super::native_contracts::phase_from_native_lifecycle)
                    .collect(),
            })
            .collect(),
        trigger_declarations: metadata
            .trigger_declarations
            .iter()
            .map(|declaration| DebTriggerMetadata {
                directive: match declaration.directive {
                    DebTriggerDirective::Interest => BundleDebTriggerDirective::Interest,
                    DebTriggerDirective::Activate => BundleDebTriggerDirective::Activate,
                },
                trigger_name: declaration.trigger_name.clone(),
                await_mode: match declaration.await_mode {
                    DebTriggerAwaitMode::Default => BundleDebTriggerAwaitMode::Default,
                    DebTriggerAwaitMode::Await => BundleDebTriggerAwaitMode::Await,
                    DebTriggerAwaitMode::NoAwait => BundleDebTriggerAwaitMode::NoAwait,
                },
                raw_line: declaration.raw_line.clone(),
            })
            .collect(),
        noninteractive: invocation.stdin != NativeStdinContract::Debconf,
    }
}

fn project_arch_install_metadata(metadata: &ArchInstallScriptletMetadata) -> ArchInstallMetadata {
    ArchInstallMetadata {
        install_digest: metadata.install_source_sha256.clone(),
        called_function: metadata.function_name.clone(),
        selection: match metadata.selection_contract {
            ArchInstallSelectionContract::LibalpmGrepV1 => ArchInstallSelection::LibalpmGrepV1,
        },
    }
}

fn rpm_trigger_family(family: RpmTriggerFamily) -> RpmTriggerKind {
    match family {
        RpmTriggerFamily::Package => RpmTriggerKind::Package,
        RpmTriggerFamily::File => RpmTriggerKind::File,
        RpmTriggerFamily::TransactionFile => RpmTriggerKind::TransactionFile,
    }
}

fn rpm_trigger_action(action: RpmTriggerAction) -> BundleRpmTriggerAction {
    match action {
        RpmTriggerAction::PreInstall => BundleRpmTriggerAction::PreInstall,
        RpmTriggerAction::Install => BundleRpmTriggerAction::Install,
        RpmTriggerAction::Uninstall => BundleRpmTriggerAction::Uninstall,
        RpmTriggerAction::PostUninstall => BundleRpmTriggerAction::PostUninstall,
        RpmTriggerAction::Unknown { .. } => {
            unreachable!("unknown RPM trigger action cannot pass package parsing")
        }
    }
}

fn deb_maintainer_mode(mode: DebMaintainerMode) -> BundleDebMaintainerMode {
    match mode {
        DebMaintainerMode::Install => BundleDebMaintainerMode::Install,
        DebMaintainerMode::Configure => BundleDebMaintainerMode::Configure,
        DebMaintainerMode::Reconfigure => BundleDebMaintainerMode::Reconfigure,
        DebMaintainerMode::Upgrade => BundleDebMaintainerMode::Upgrade,
        DebMaintainerMode::Remove => BundleDebMaintainerMode::Remove,
        DebMaintainerMode::Purge => BundleDebMaintainerMode::Purge,
        DebMaintainerMode::Triggered => BundleDebMaintainerMode::Triggered,
        DebMaintainerMode::Disappear => BundleDebMaintainerMode::Disappear,
        DebMaintainerMode::Deconfigure => BundleDebMaintainerMode::Deconfigure,
        DebMaintainerMode::FailedUpgrade => BundleDebMaintainerMode::FailedUpgrade,
        DebMaintainerMode::AbortInstall => BundleDebMaintainerMode::AbortInstall,
        DebMaintainerMode::AbortUpgrade => BundleDebMaintainerMode::AbortUpgrade,
        DebMaintainerMode::AbortRemove => BundleDebMaintainerMode::AbortRemove,
        DebMaintainerMode::AbortDeconfigure => BundleDebMaintainerMode::AbortDeconfigure,
    }
}

fn deb_argument_value(
    value: &NativeArgumentValue,
) -> (BundleDebMaintainerArgumentValue, Option<String>) {
    match value {
        NativeArgumentValue::Action => (BundleDebMaintainerArgumentValue::Action, None),
        NativeArgumentValue::OldVersion => (BundleDebMaintainerArgumentValue::OldVersion, None),
        NativeArgumentValue::NewVersion => (BundleDebMaintainerArgumentValue::NewVersion, None),
        NativeArgumentValue::PackageInstanceCount => {
            (BundleDebMaintainerArgumentValue::PackageInstanceCount, None)
        }
        NativeArgumentValue::PackageName => (BundleDebMaintainerArgumentValue::PackageName, None),
        NativeArgumentValue::InstallingPackageName => (
            BundleDebMaintainerArgumentValue::InstallingPackageName,
            None,
        ),
        NativeArgumentValue::InstallingPackageVersion => (
            BundleDebMaintainerArgumentValue::InstallingPackageVersion,
            None,
        ),
        NativeArgumentValue::ConflictingPackageMarker => (
            BundleDebMaintainerArgumentValue::ConflictingPackageMarker,
            None,
        ),
        NativeArgumentValue::ConflictingPackageName => (
            BundleDebMaintainerArgumentValue::ConflictingPackageName,
            None,
        ),
        NativeArgumentValue::ConflictingPackageVersion => (
            BundleDebMaintainerArgumentValue::ConflictingPackageVersion,
            None,
        ),
        NativeArgumentValue::TriggerName => (BundleDebMaintainerArgumentValue::TriggerName, None),
        NativeArgumentValue::TriggerNames => (BundleDebMaintainerArgumentValue::TriggerNames, None),
        NativeArgumentValue::TriggerCount => (BundleDebMaintainerArgumentValue::TriggerCount, None),
        NativeArgumentValue::FilePath => (BundleDebMaintainerArgumentValue::FilePath, None),
        NativeArgumentValue::InstalledVersion => {
            (BundleDebMaintainerArgumentValue::InstalledVersion, None)
        }
        NativeArgumentValue::MostRecentlyConfiguredVersion => (
            BundleDebMaintainerArgumentValue::MostRecentlyConfiguredVersion,
            None,
        ),
        NativeArgumentValue::Raw(literal) => (
            BundleDebMaintainerArgumentValue::Literal,
            Some(literal.clone()),
        ),
    }
}

fn project_arch_hook_metadata(metadata: &ArchAlpmHookMetadata) -> ArchHookMetadata {
    ArchHookMetadata {
        hook_path: metadata.hook_path.clone(),
        triggers: metadata
            .triggers
            .iter()
            .map(|trigger| ArchHookTriggerMetadata {
                operations: trigger
                    .operations
                    .iter()
                    .map(|operation| match operation {
                        ArchAlpmHookOperation::Install => BundleArchHookOperation::Install,
                        ArchAlpmHookOperation::Upgrade => BundleArchHookOperation::Upgrade,
                        ArchAlpmHookOperation::Remove => BundleArchHookOperation::Remove,
                    })
                    .collect(),
                trigger_type: match trigger.trigger_type {
                    ArchAlpmHookTriggerType::Package => BundleArchHookTriggerType::Package,
                    ArchAlpmHookTriggerType::Path => BundleArchHookTriggerType::Path,
                },
                targets: trigger.targets.clone(),
            })
            .collect(),
        action: ArchHookActionMetadata {
            description: metadata.action.description.clone(),
            when: match metadata.action.when {
                crate::packages::native_abi::NativeTransactionPosition::BeforeTransaction => {
                    ArchHookWhen::PreTransaction
                }
                crate::packages::native_abi::NativeTransactionPosition::AfterTransaction => {
                    ArchHookWhen::PostTransaction
                }
                other => {
                    unreachable!("strict ALPM parser produced invalid hook position {other:?}")
                }
            },
            exec: metadata.action.exec.clone(),
            argv: metadata.action.argv.clone(),
            depends: metadata.action.depends.clone(),
            abort_on_fail: metadata.action.abort_on_fail,
            needs_targets: metadata.action.needs_targets,
        },
    }
}
