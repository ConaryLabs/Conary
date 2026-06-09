// conary-core/src/ccs/convert/scriptlet_bundle/format_metadata.rs

use super::native_contracts::{native_stdin, native_transaction_position};
use crate::ccs::legacy_scriptlets::{
    ArchInstallMetadata, DebMaintainerMetadata, RpmTriggerMetadata as BundleRpmTriggerMetadata,
    RpmTriggerTargetConstraint,
};
use crate::packages::native_abi::{
    ArchAlpmHookAction, ArchAlpmHookMetadata, ArchAlpmHookOperation, ArchAlpmHookTrigger,
    ArchAlpmHookTriggerType, ArchFunctionExtractionStatus, ArchInstallScriptletMetadata,
    ArchNativeScriptletMetadata, DebControlMember, DebMaintainerMode, DebNativeScriptletMetadata,
    DebTriggerAwaitMode, DebTriggerDeclaration, DebTriggerDirective, NativeInvocationContract,
    NativeScriptletBody, NativeScriptletEntry, NativeScriptletMetadata, NativeStdinContract,
    NativeTransactionOrder, RpmNativeScriptletMetadata, RpmTriggerAction, RpmTriggerFamily,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn project_format_metadata(
    native: &NativeScriptletEntry,
    extra: &mut BTreeMap<String, toml::Value>,
) -> (
    Option<BundleRpmTriggerMetadata>,
    Option<DebMaintainerMetadata>,
    Option<ArchInstallMetadata>,
) {
    match &native.metadata {
        NativeScriptletMetadata::Rpm(metadata) => (
            project_rpm_metadata(metadata, &native.invocation, &native.order, extra),
            None,
            None,
        ),
        NativeScriptletMetadata::Deb(metadata) => (
            None,
            Some(project_deb_metadata(
                metadata,
                &native.body,
                &native.invocation,
                extra,
            )),
            None,
        ),
        NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(metadata)) => (
            None,
            None,
            Some(project_arch_install_metadata(metadata, extra)),
        ),
        NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(metadata)) => {
            extra.insert("arch_alpm_hook".to_string(), arch_alpm_hook_value(metadata));
            (None, None, None)
        }
    }
}

fn project_rpm_metadata(
    metadata: &RpmNativeScriptletMetadata,
    invocation: &NativeInvocationContract,
    order: &NativeTransactionOrder,
    extra: &mut BTreeMap<String, toml::Value>,
) -> Option<BundleRpmTriggerMetadata> {
    if let Some(flags) = &metadata.scriptlet_flags {
        let mut table = toml::Table::new();
        table.insert("names".to_string(), toml_string_array(&flags.names));
        table.insert(
            "raw_bits".to_string(),
            toml::Value::Integer(flags.raw_bits as i64),
        );
        extra.insert("rpm_scriptlet_flags".to_string(), toml::Value::Table(table));
    }

    let trigger = metadata.trigger.as_ref()?;
    Some(BundleRpmTriggerMetadata {
        kind: rpm_trigger_family(trigger.family).to_string(),
        condition: trigger
            .conditions
            .first()
            .map(|condition| condition.name.clone()),
        target_constraints: trigger
            .conditions
            .iter()
            .map(|condition| RpmTriggerTargetConstraint {
                package: condition.name.clone(),
                operator: condition.comparison.clone(),
                version: condition.version.clone(),
                extra: BTreeMap::from([
                    (
                        "action".to_string(),
                        toml::Value::String(rpm_trigger_action(condition.action).to_string()),
                    ),
                    (
                        "raw_flags".to_string(),
                        toml::Value::Integer(condition.raw_flags as i64),
                    ),
                ]),
            })
            .collect(),
        priority: None,
        file_globs: trigger.file_globs.clone(),
        stdin_contract: native_stdin(invocation.stdin).map(str::to_string),
        transaction_order: Some(native_transaction_position(order.position).to_string()),
        extra: BTreeMap::new(),
    })
}

fn project_deb_metadata(
    metadata: &DebNativeScriptletMetadata,
    body: &NativeScriptletBody,
    invocation: &NativeInvocationContract,
    extra: &mut BTreeMap<String, toml::Value>,
) -> DebMaintainerMetadata {
    if !metadata.trigger_declarations.is_empty() {
        extra.insert(
            "deb_trigger_raw_lines".to_string(),
            toml::Value::Array(
                metadata
                    .trigger_declarations
                    .iter()
                    .map(|declaration| toml::Value::String(declaration.raw_line.clone()))
                    .collect(),
            ),
        );
        extra.insert(
            "deb_trigger_declarations".to_string(),
            toml::Value::Array(
                metadata
                    .trigger_declarations
                    .iter()
                    .map(deb_trigger_declaration_value)
                    .collect(),
            ),
        );
    }

    DebMaintainerMetadata {
        invocation_mode: metadata
            .maintainer_modes
            .first()
            .map(|mode| deb_maintainer_mode(mode.mode).to_string()),
        old_version: None,
        new_version: None,
        triggers_content: matches!(metadata.control_member, DebControlMember::Triggers).then(
            || {
                body.text
                    .clone()
                    .unwrap_or_else(|| String::from_utf8_lossy(&body.bytes).into_owned())
            },
        ),
        trigger_names: metadata
            .trigger_declarations
            .iter()
            .map(|declaration| declaration.trigger_name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        purge: metadata
            .maintainer_modes
            .iter()
            .any(|mode| mode.mode == DebMaintainerMode::Purge),
        abort: metadata.maintainer_modes.iter().any(|mode| {
            matches!(
                mode.mode,
                DebMaintainerMode::AbortInstall
                    | DebMaintainerMode::AbortUpgrade
                    | DebMaintainerMode::AbortRemove
                    | DebMaintainerMode::AbortDeconfigure
            )
        }),
        noninteractive: invocation.stdin != NativeStdinContract::Debconf,
        extra: BTreeMap::from([(
            "control_member".to_string(),
            toml::Value::String(deb_control_member(metadata.control_member).to_string()),
        )]),
    }
}

fn project_arch_install_metadata(
    metadata: &ArchInstallScriptletMetadata,
    extra: &mut BTreeMap<String, toml::Value>,
) -> ArchInstallMetadata {
    if let Some(function_body) = &metadata.function_body {
        extra.insert(
            "arch_function_body".to_string(),
            toml::Value::String(function_body.clone()),
        );
    }
    extra.insert(
        "arch_function_extraction_status".to_string(),
        toml::Value::String(
            arch_function_extraction_status(&metadata.extraction_status).to_string(),
        ),
    );
    if let ArchFunctionExtractionStatus::DeferredReview { reason_code } =
        &metadata.extraction_status
    {
        extra.insert(
            "arch_function_extraction_reason".to_string(),
            toml::Value::String(reason_code.clone()),
        );
    }

    ArchInstallMetadata {
        install_digest: Some(metadata.install_source_sha256.clone()),
        called_function: Some(metadata.function_name.clone()),
        old_version: None,
        new_version: None,
        wrapper_source_digest: metadata.function_body_sha256.clone(),
        extra: BTreeMap::new(),
    }
}

fn rpm_trigger_family(family: RpmTriggerFamily) -> &'static str {
    match family {
        RpmTriggerFamily::Package => "package",
        RpmTriggerFamily::File => "file",
        RpmTriggerFamily::TransactionFile => "transaction-file",
    }
}

fn rpm_trigger_action(action: RpmTriggerAction) -> &'static str {
    match action {
        RpmTriggerAction::PreInstall => "pre-install",
        RpmTriggerAction::Install => "install",
        RpmTriggerAction::Uninstall => "uninstall",
        RpmTriggerAction::PostUninstall => "post-uninstall",
        RpmTriggerAction::Unknown { .. } => "unknown",
    }
}

fn deb_control_member(member: DebControlMember) -> &'static str {
    match member {
        DebControlMember::Config => "config",
        DebControlMember::Preinst => "preinst",
        DebControlMember::Postinst => "postinst",
        DebControlMember::Prerm => "prerm",
        DebControlMember::Postrm => "postrm",
        DebControlMember::Triggers => "triggers",
    }
}

fn deb_maintainer_mode(mode: DebMaintainerMode) -> &'static str {
    match mode {
        DebMaintainerMode::Install => "install",
        DebMaintainerMode::Configure => "configure",
        DebMaintainerMode::Reconfigure => "reconfigure",
        DebMaintainerMode::Upgrade => "upgrade",
        DebMaintainerMode::Remove => "remove",
        DebMaintainerMode::Purge => "purge",
        DebMaintainerMode::Triggered => "triggered",
        DebMaintainerMode::Disappear => "disappear",
        DebMaintainerMode::Deconfigure => "deconfigure",
        DebMaintainerMode::FailedUpgrade => "failed-upgrade",
        DebMaintainerMode::AbortInstall => "abort-install",
        DebMaintainerMode::AbortUpgrade => "abort-upgrade",
        DebMaintainerMode::AbortRemove => "abort-remove",
        DebMaintainerMode::AbortDeconfigure => "abort-deconfigure",
    }
}

fn deb_trigger_declaration_value(declaration: &DebTriggerDeclaration) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert(
        "directive".to_string(),
        toml::Value::String(deb_trigger_directive(declaration.directive).to_string()),
    );
    table.insert(
        "trigger_name".to_string(),
        toml::Value::String(declaration.trigger_name.clone()),
    );
    table.insert(
        "await_mode".to_string(),
        toml::Value::String(deb_trigger_await_mode(declaration.await_mode).to_string()),
    );
    table.insert(
        "raw_line".to_string(),
        toml::Value::String(declaration.raw_line.clone()),
    );
    toml::Value::Table(table)
}

fn deb_trigger_directive(directive: DebTriggerDirective) -> &'static str {
    match directive {
        DebTriggerDirective::Interest => "interest",
        DebTriggerDirective::Activate => "activate",
    }
}

fn deb_trigger_await_mode(await_mode: DebTriggerAwaitMode) -> &'static str {
    match await_mode {
        DebTriggerAwaitMode::Default => "default",
        DebTriggerAwaitMode::Await => "await",
        DebTriggerAwaitMode::NoAwait => "noawait",
    }
}

fn arch_function_extraction_status(status: &ArchFunctionExtractionStatus) -> &'static str {
    match status {
        ArchFunctionExtractionStatus::Parsed => "parsed",
        ArchFunctionExtractionStatus::DeferredReview { .. } => "deferred-review",
    }
}

fn arch_alpm_hook_value(metadata: &ArchAlpmHookMetadata) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert(
        "hook_path".to_string(),
        toml::Value::String(metadata.hook_path.clone()),
    );
    table.insert(
        "triggers".to_string(),
        toml::Value::Array(
            metadata
                .triggers
                .iter()
                .map(arch_alpm_hook_trigger_value)
                .collect(),
        ),
    );
    if let Some(action) = &metadata.action {
        table.insert("action".to_string(), arch_alpm_hook_action_value(action));
    }
    toml::Value::Table(table)
}

fn arch_alpm_hook_trigger_value(trigger: &ArchAlpmHookTrigger) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert(
        "operations".to_string(),
        toml::Value::Array(
            trigger
                .operations
                .iter()
                .map(|operation| {
                    toml::Value::String(arch_alpm_hook_operation(*operation).to_string())
                })
                .collect(),
        ),
    );
    table.insert(
        "type".to_string(),
        toml::Value::String(arch_alpm_hook_trigger_type(trigger.trigger_type).to_string()),
    );
    table.insert("targets".to_string(), toml_string_array(&trigger.targets));
    toml::Value::Table(table)
}

fn arch_alpm_hook_action_value(action: &ArchAlpmHookAction) -> toml::Value {
    let mut table = toml::Table::new();
    if let Some(description) = &action.description {
        table.insert(
            "description".to_string(),
            toml::Value::String(description.clone()),
        );
    }
    table.insert(
        "when".to_string(),
        toml::Value::String(native_transaction_position(action.when).to_string()),
    );
    table.insert("exec".to_string(), toml::Value::String(action.exec.clone()));
    table.insert("depends".to_string(), toml_string_array(&action.depends));
    table.insert(
        "abort_on_fail".to_string(),
        toml::Value::Boolean(action.abort_on_fail),
    );
    table.insert(
        "needs_targets".to_string(),
        toml::Value::Boolean(action.needs_targets),
    );
    toml::Value::Table(table)
}

fn arch_alpm_hook_operation(operation: ArchAlpmHookOperation) -> &'static str {
    match operation {
        ArchAlpmHookOperation::Install => "install",
        ArchAlpmHookOperation::Upgrade => "upgrade",
        ArchAlpmHookOperation::Remove => "remove",
    }
}

fn arch_alpm_hook_trigger_type(trigger_type: ArchAlpmHookTriggerType) -> &'static str {
    match trigger_type {
        ArchAlpmHookTriggerType::Package => "package",
        ArchAlpmHookTriggerType::Path => "path",
    }
}

fn toml_string_array(values: &[String]) -> toml::Value {
    toml::Value::Array(values.iter().cloned().map(toml::Value::String).collect())
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        arch_alpm_hook_entry, arch_install_entry, bundle_for_metadata, deb_triggers_entry,
        package_metadata, rpm_trigger_entry,
    };
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletClassificationReport};
    use crate::ccs::legacy_scriptlets::{
        PublicationStatus, ScriptletDecision, ScriptletFidelity, TargetCompatibility,
    };

    #[test]
    fn format_metadata_boundaries_become_review_required_with_registry_reasons() {
        let mut metadata = package_metadata("metadata-boundaries", "1.0");
        metadata.native_scriptlet_abi = vec![
            rpm_trigger_entry(),
            deb_triggers_entry(),
            arch_install_entry(),
        ];
        let mut classification = ScriptletClassificationReport::default();
        for (entry_id, class_id, reason_code) in [
            ("rpm:trigger", "rpm-trigger", "review-class-rpm-trigger"),
            ("deb:triggers", "deb-trigger", "review-class-deb-trigger"),
            (
                "arch:post_install",
                "arch-install-function",
                "review-class-arch-install-function",
            ),
        ] {
            classification.push(
                entry_id,
                ScriptletClassification::Review {
                    reason_code: reason_code.to_string(),
                    class_id: Some(class_id.to_string()),
                },
            );
        }

        let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();

        for (entry_id, reason_code) in [
            ("rpm:trigger", "review-class-rpm-trigger"),
            ("deb:triggers", "review-class-deb-trigger"),
            ("arch:post_install", "review-class-arch-install-function"),
        ] {
            let entry = build
                .bundle
                .entries
                .iter()
                .find(|entry| entry.id == entry_id)
                .unwrap_or_else(|| panic!("missing entry {entry_id}"));
            assert_eq!(entry.decision, ScriptletDecision::Review, "{entry_id}");
            assert_eq!(entry.reason_code, reason_code, "{entry_id}");
        }
        assert_eq!(
            build.bundle.scriptlet_fidelity,
            ScriptletFidelity::ReviewRequired
        );
        assert_eq!(
            build.bundle.target_compatibility,
            TargetCompatibility::ReviewRequired
        );
        assert_eq!(
            build.bundle.publication_status,
            PublicationStatus::PrivateReview
        );
        for reason_code in [
            "review-class-rpm-trigger",
            "review-class-deb-trigger",
            "review-class-arch-install-function",
        ] {
            assert!(
                build
                    .summary
                    .review_reason_codes
                    .iter()
                    .any(|code| code == reason_code),
                "missing review reason {reason_code}"
            );
        }
    }

    #[test]
    fn format_specific_metadata_projects_into_bundle() {
        let mut metadata = package_metadata("format-specific", "1.0");
        metadata.native_scriptlet_abi = vec![
            rpm_trigger_entry(),
            deb_triggers_entry(),
            arch_install_entry(),
            arch_alpm_hook_entry(),
        ];

        let build =
            bundle_for_metadata(&metadata, &[], &ScriptletClassificationReport::default()).unwrap();

        let rpm = build
            .bundle
            .entries
            .iter()
            .find(|entry| entry.id == "rpm:trigger")
            .unwrap();
        assert_eq!(rpm.rpm_trigger.as_ref().unwrap().kind, "file");
        assert_eq!(
            rpm.rpm_trigger.as_ref().unwrap().file_globs,
            vec!["/usr/share/icons/*"]
        );
        assert!(rpm.extra.contains_key("rpm_scriptlet_flags"));

        let deb = build
            .bundle
            .entries
            .iter()
            .find(|entry| entry.id == "deb:triggers")
            .unwrap();
        assert_eq!(
            deb.deb_maintainer
                .as_ref()
                .unwrap()
                .triggers_content
                .as_deref(),
            Some("interest-noawait icon-cache\n")
        );
        assert_eq!(
            deb.deb_maintainer.as_ref().unwrap().trigger_names,
            vec!["icon-cache"]
        );
        assert!(deb.extra.contains_key("deb_trigger_raw_lines"));

        let arch_install = build
            .bundle
            .entries
            .iter()
            .find(|entry| entry.id == "arch:post_install")
            .unwrap();
        assert_eq!(
            arch_install
                .arch_install
                .as_ref()
                .unwrap()
                .called_function
                .as_deref(),
            Some("post_install")
        );

        let hook = build
            .bundle
            .entries
            .iter()
            .find(|entry| entry.id == "arch:hook")
            .unwrap();
        assert!(hook.extra.contains_key("arch_alpm_hook"));
        assert_eq!(
            hook.extra.get("native_scriptlet_kind"),
            Some(&toml::Value::String("control-artifact".to_string()))
        );
    }

    #[test]
    fn arch_alpm_hook_control_artifact_validates_with_placeholder_interpreter() {
        let mut metadata = package_metadata("arch-hook", "1.0");
        metadata.native_scriptlet_abi.push(arch_alpm_hook_entry());

        let build =
            bundle_for_metadata(&metadata, &[], &ScriptletClassificationReport::default()).unwrap();
        let entry = &build.bundle.entries[0];

        assert_eq!(entry.interpreter, "package-manager-control-artifact");
        assert!(entry.extra.contains_key("arch_alpm_hook"));
        build.bundle.validate().unwrap();
    }
}
