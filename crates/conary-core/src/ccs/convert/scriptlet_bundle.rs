// conary-core/src/ccs/convert/scriptlet_bundle.rs
//! Passive legacy scriptlet bundle construction for legacy package conversion.

mod classification;
mod native_contracts;
mod summary;
#[cfg(test)]
mod test_support;
mod types;

pub use types::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    ScriptletDecisionCountsSummary,
};

use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletClassificationReport, ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::{
    ArchInstallMetadata, DebMaintainerMetadata, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1,
    LegacyScriptletBundle, LegacyScriptletEntry, NativeInvocation,
    RpmTriggerMetadata as BundleRpmTriggerMetadata, RpmTriggerTargetConstraint, SourceFormat,
    VersionScheme,
};
use crate::packages::common::PackageMetadata;
use crate::packages::native_abi::{
    ArchAlpmHookAction, ArchAlpmHookMetadata, ArchAlpmHookOperation, ArchAlpmHookTrigger,
    ArchAlpmHookTriggerType, ArchFunctionExtractionStatus, ArchNativeScriptletMetadata,
    DebControlMember, DebMaintainerMode, DebNativeScriptletMetadata, DebTriggerAwaitMode,
    DebTriggerDeclaration, DebTriggerDirective, NativeInvocationContract, NativeScriptletBody,
    NativeScriptletEntry, NativeScriptletMetadata, NativeScriptletSupport, NativeStdinContract,
    NativeTransactionOrder, RpmNativeScriptletMetadata, RpmTriggerAction, RpmTriggerFamily,
};
use crate::packages::traits::Scriptlet;
use std::collections::{BTreeMap, BTreeSet};

use classification::{classification_entries_for, classify_entry};
use native_contracts::{
    encoded_native_body, flat_transaction_order, native_invocation, native_lifecycle_paths,
    native_scriptlet_kind, native_stdin, native_transaction_order, native_transaction_position,
    non_empty_or_default, phase_from_native_lifecycle, phase_from_scriptlet_phase,
};
use summary::{aggregate_status, decision_counts, summary_from_bundle};

pub fn build_legacy_scriptlet_bundle(
    input: ScriptletBundleInput<'_>,
) -> anyhow::Result<ScriptletBundleBuild> {
    let format = source_format(input.source_format)?;
    let source_distro = input.source_distro.unwrap_or("unknown").to_string();
    let source_release = input.source_release.unwrap_or("unknown").to_string();
    let source_arch = input
        .source_arch
        .or(input.source_metadata.architecture.as_deref())
        .unwrap_or("unknown")
        .to_string();
    let source_checksum = input
        .source_checksum
        .filter(|checksum| valid_prefixed_sha256(checksum))
        .map(str::to_string);

    let entries = build_entries(&input)?;
    let decision_counts = decision_counts(&entries);
    let (scriptlet_fidelity, target_compatibility, publication_policy, publication_status) =
        aggregate_status(&entries, &decision_counts);

    let mut bundle = LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 1,
        source_format: format.clone(),
        source_family: source_family(&format).to_string(),
        source_distro: Some(source_distro),
        source_release: Some(source_release),
        source_arch: Some(source_arch),
        source_package: input.source_metadata.name.clone(),
        source_version: input.source_metadata.version.clone(),
        source_checksum,
        version_scheme: version_scheme(&format),
        conversion_tool: input.conversion_tool.to_string(),
        conversion_tool_version: input.conversion_tool_version.to_string(),
        conversion_policy: "passive-scriptlet-bundle-goal4".to_string(),
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: None,
        target_compatibility,
        allowed_targets: Vec::new(),
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy,
        publication_status,
        scriptlet_fidelity,
        decision_counts,
        unsupported_class_counts: input.classification.unsupported_class_counts.clone(),
        entries,
        extra: BTreeMap::new(),
    };

    let digest = evidence_digest(&bundle, &input)?;
    bundle.evidence_digest = Some(digest.clone());
    for entry in &mut bundle.entries {
        entry.evidence_digest = Some(digest.clone());
    }
    bundle.validate()?;

    Ok(ScriptletBundleBuild {
        summary: summary_from_bundle(&bundle, Some(digest)),
        bundle,
    })
}

fn source_format(value: &str) -> anyhow::Result<SourceFormat> {
    match value {
        "rpm" => Ok(SourceFormat::Rpm),
        "deb" => Ok(SourceFormat::Deb),
        "arch" => Ok(SourceFormat::Arch),
        other => anyhow::bail!("unsupported scriptlet source format '{other}'"),
    }
}

fn source_family(format: &SourceFormat) -> &'static str {
    match format {
        SourceFormat::Rpm => "rpm",
        SourceFormat::Deb => "deb",
        SourceFormat::Arch => "arch",
        SourceFormat::Unknown(_) => "unknown",
    }
}

fn version_scheme(format: &SourceFormat) -> VersionScheme {
    match format {
        SourceFormat::Rpm => VersionScheme::Rpm,
        SourceFormat::Deb => VersionScheme::Deb,
        SourceFormat::Arch => VersionScheme::Arch,
        SourceFormat::Unknown(_) => VersionScheme::Semver,
    }
}

fn valid_prefixed_sha256(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn build_entries(input: &ScriptletBundleInput<'_>) -> anyhow::Result<Vec<LegacyScriptletEntry>> {
    if !input.source_metadata.native_scriptlet_abi.is_empty() {
        input
            .source_metadata
            .native_scriptlet_abi
            .iter()
            .map(|entry| build_native_entry(entry, input.classification))
            .collect()
    } else {
        input
            .source_metadata
            .scriptlets
            .iter()
            .enumerate()
            .map(|(index, scriptlet)| build_flat_entry(index, scriptlet, input.classification))
            .collect()
    }
}

fn build_flat_entry(
    index: usize,
    scriptlet: &Scriptlet,
    report: &ScriptletClassificationReport,
) -> anyhow::Result<LegacyScriptletEntry> {
    let id = format!("scriptlet:{index}:{}", scriptlet.phase);
    let phase = phase_from_scriptlet_phase(scriptlet.phase);
    let lifecycle_paths = vec![phase.as_str().to_string()];
    let classifications = classification_entries_for(report, &id);
    let outcome = classify_entry(&classifications, &NativeScriptletSupport::Parsed);
    let body_bytes = scriptlet.content.as_bytes();

    Ok(LegacyScriptletEntry {
        id,
        native_slot: scriptlet.phase.to_string(),
        phase,
        lifecycle_paths,
        interpreter: non_empty_or_default(&scriptlet.interpreter, "/bin/sh"),
        interpreter_args: scriptlet
            .flags
            .as_deref()
            .map(|flags| flags.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default(),
        body_sha256: crate::hash::sha256_prefixed(body_bytes),
        body: scriptlet.content.clone(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: flat_transaction_order(scriptlet.phase),
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        decision: outcome.decision,
        reason_code: outcome.reason_code,
        human_reason: None,
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        effects: outcome.effects,
        unknown_commands: outcome.unknown_commands,
        blocked_classes: outcome.blocked_classes,
        rpm_trigger: None,
        deb_maintainer: None,
        arch_install: None,
        residual_replay: None,
        extra: BTreeMap::new(),
    })
}

fn build_native_entry(
    native: &NativeScriptletEntry,
    report: &ScriptletClassificationReport,
) -> anyhow::Result<LegacyScriptletEntry> {
    let classifications = classification_entries_for(report, &native.id);
    let outcome = classify_entry(&classifications, &native.support);
    let phase = phase_from_native_lifecycle(native.primary_lifecycle);
    let lifecycle_paths = native_lifecycle_paths(native);
    let (body, body_encoding) = encoded_native_body(&native.body);
    let mut extra = BTreeMap::from([(
        "native_scriptlet_kind".to_string(),
        toml::Value::String(native_scriptlet_kind(native.kind).to_string()),
    )]);
    let (rpm_trigger, deb_maintainer, arch_install) = project_format_metadata(native, &mut extra);

    Ok(LegacyScriptletEntry {
        id: native.id.clone(),
        native_slot: native.native_slot.clone(),
        phase,
        lifecycle_paths,
        interpreter: native
            .interpreter
            .clone()
            .unwrap_or_else(|| "package-manager-control-artifact".to_string()),
        interpreter_args: native.interpreter_args.clone(),
        body_sha256: native.body.sha256.clone(),
        body,
        body_encoding,
        native_invocation: native_invocation(&native.invocation),
        transaction_order: native_transaction_order(&native.order),
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        decision: outcome.decision,
        reason_code: outcome.reason_code,
        human_reason: None,
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        effects: outcome.effects,
        unknown_commands: outcome.unknown_commands,
        blocked_classes: outcome.blocked_classes,
        rpm_trigger,
        deb_maintainer,
        arch_install,
        residual_replay: None,
        extra,
    })
}

fn project_format_metadata(
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
    metadata: &crate::packages::native_abi::ArchInstallScriptletMetadata,
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

fn evidence_digest(
    bundle: &LegacyScriptletBundle,
    input: &ScriptletBundleInput<'_>,
) -> anyhow::Result<String> {
    let digest_doc = serde_json::json!({
        "schema": "conary-scriptlet-evidence-v1",
        "source_format": bundle.source_format.as_str(),
        "source_distro": bundle.source_distro.as_deref(),
        "source_release": bundle.source_release.as_deref(),
        "source_arch": bundle.source_arch.as_deref(),
        "source_package": &bundle.source_package,
        "source_version": &bundle.source_version,
        "source_checksum": bundle.source_checksum.as_deref(),
        "native_entries": sorted_native_digest_entries(input.source_metadata),
        "flat_entries": sorted_flat_digest_entries(input.source_metadata),
        "classification_counts": {
            "known": input.classification.known_count,
            "unknown": input.classification.unknown_count,
            "review": input.classification.review_count,
            "blocked": input.classification.blocked_count,
        },
        "classification_reasons": sorted_classification_reasons(input.classification),
        "classification_evidence": sorted_classification_evidence(input.classification),
        "entry_decisions": sorted_entry_decision_digest(bundle),
        "decision_counts": {
            "replaced": bundle.decision_counts.replaced,
            "legacy": bundle.decision_counts.legacy,
            "blocked": bundle.decision_counts.blocked,
            "review": bundle.decision_counts.review,
        },
        "scriptlet_fidelity": bundle.scriptlet_fidelity.as_str(),
        "target_compatibility": bundle.target_compatibility.as_str(),
        "publication_status": bundle.publication_status.as_str(),
    });
    let canonical = crate::json::canonical_json(&digest_doc)
        .map_err(|error| anyhow::anyhow!("failed to canonicalize scriptlet evidence: {error}"))?;
    let mut bytes = b"conary-scriptlet-evidence-v1\n".to_vec();
    bytes.extend_from_slice(&canonical);
    Ok(crate::hash::sha256_prefixed(&bytes))
}

fn sorted_native_digest_entries(metadata: &PackageMetadata) -> Vec<serde_json::Value> {
    let mut entries = metadata
        .native_scriptlet_abi
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": &entry.id,
                "slot": &entry.native_slot,
                "body_sha256": &entry.body.sha256,
                "support": native_support_digest(&entry.support),
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left["id"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["id"].as_str().unwrap_or_default())
    });
    entries
}

fn sorted_flat_digest_entries(metadata: &PackageMetadata) -> Vec<serde_json::Value> {
    if !metadata.native_scriptlet_abi.is_empty() {
        return Vec::new();
    }
    metadata
        .scriptlets
        .iter()
        .enumerate()
        .map(|(index, scriptlet)| {
            serde_json::json!({
                "id": format!("scriptlet:{index}:{}", scriptlet.phase),
                "phase": scriptlet.phase.to_string(),
                "body_sha256": crate::hash::sha256_prefixed(scriptlet.content.as_bytes()),
            })
        })
        .collect()
}

fn sorted_classification_reasons(report: &ScriptletClassificationReport) -> Vec<String> {
    report
        .entries
        .iter()
        .map(|entry| match &entry.classification {
            ScriptletClassification::Known { reason_code, .. }
            | ScriptletClassification::Unknown { reason_code, .. }
            | ScriptletClassification::Review { reason_code, .. }
            | ScriptletClassification::Blocked { reason_code, .. } => reason_code.clone(),
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn sorted_classification_evidence(
    report: &ScriptletClassificationReport,
) -> Vec<serde_json::Value> {
    let mut values = report
        .entries
        .iter()
        .map(|entry| match &entry.classification {
            ScriptletClassification::Known {
                reason_code,
                effects,
            } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "known",
                "reason_code": reason_code,
                "effects": sorted_effect_digest(effects),
            }),
            ScriptletClassification::Unknown {
                command,
                reason_code,
            } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "unknown",
                "command": command,
                "reason_code": reason_code,
            }),
            ScriptletClassification::Review {
                class_id,
                reason_code,
            } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "review",
                "class_id": class_id,
                "reason_code": reason_code,
            }),
            ScriptletClassification::Blocked {
                class_id,
                reason_code,
            } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "blocked",
                "class_id": class_id,
                "reason_code": reason_code,
            }),
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left["entry_id"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["entry_id"].as_str().unwrap_or_default())
            .then_with(|| {
                left["outcome"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["outcome"].as_str().unwrap_or_default())
            })
            .then_with(|| {
                left["reason_code"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["reason_code"].as_str().unwrap_or_default())
            })
    });
    values
}

fn sorted_effect_digest(effects: &[ScriptletEffectEvidence]) -> Vec<serde_json::Value> {
    let mut values = effects
        .iter()
        .map(|effect| {
            serde_json::json!({
                "kind": &effect.kind,
                "replacement": effect.replacement.as_str(),
                "adapter_id": effect.adapter_id.as_deref(),
                "adapter_digest": effect.adapter_digest.as_deref(),
                "reason_code": effect.reason_code.as_deref(),
                "command": effect.command.as_deref(),
            })
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left["kind"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["kind"].as_str().unwrap_or_default())
            .then_with(|| {
                left["adapter_id"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["adapter_id"].as_str().unwrap_or_default())
            })
    });
    values
}

fn sorted_entry_decision_digest(bundle: &LegacyScriptletBundle) -> Vec<serde_json::Value> {
    let mut values = bundle
        .entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": &entry.id,
                "decision": entry.decision.as_str(),
                "reason_code": &entry.reason_code,
                "body_sha256": &entry.body_sha256,
                "unknown_commands": &entry.unknown_commands,
                "blocked_classes": &entry.blocked_classes,
            })
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left["id"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["id"].as_str().unwrap_or_default())
    });
    values
}

fn native_support_digest(support: &NativeScriptletSupport) -> serde_json::Value {
    match support {
        NativeScriptletSupport::Parsed => serde_json::json!({"status": "parsed"}),
        NativeScriptletSupport::DeferredReview { reason_code } => {
            serde_json::json!({"status": "deferred-review", "reason_code": reason_code})
        }
        NativeScriptletSupport::Unpreservable { reason_code } => {
            serde_json::json!({"status": "unpreservable", "reason_code": reason_code})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{
        arch_alpm_hook_entry, arch_install_entry, bundle_for_metadata, complete_effect,
        deb_triggers_entry, known_report_with_effect, native_entry_with_body, package_metadata,
        rpm_trigger_entry,
    };
    use super::*;
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletClassificationReport};
    use crate::ccs::legacy_scriptlets::{
        EffectReplacement, ForeignReplayPolicy, PublicationPolicy, PublicationStatus,
        ScriptletDecision, ScriptletFidelity, TargetCompatibility,
    };
    use crate::packages::native_abi::NativeScriptletSupport;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};

    #[test]
    fn native_free_input_builds_zero_entry_bundle() {
        let metadata = package_metadata("native-free", "1.0");
        let files = Vec::new();
        let classification = ScriptletClassificationReport::default();

        let build = build_legacy_scriptlet_bundle(ScriptletBundleInput {
            source_metadata: &metadata,
            final_metadata: &metadata,
            source_files: &files,
            final_files: &files,
            source_format: "rpm",
            source_distro: Some("fedora-44"),
            source_release: Some("44"),
            source_arch: Some("x86_64"),
            source_checksum: Some(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            classification: &classification,
            conversion_tool: "remi",
            conversion_tool_version: "0.1.0",
        })
        .unwrap();

        assert!(build.bundle.entries.is_empty());
        assert_eq!(build.bundle.scriptlet_fidelity.as_str(), "native-free");
        assert_eq!(
            build.bundle.target_compatibility.as_str(),
            "conary-portable"
        );
        assert_eq!(
            build.bundle.publication_policy.as_str(),
            "public-if-no-blocked"
        );
        assert_eq!(build.bundle.publication_status.as_str(), "public");
        assert_eq!(build.bundle.decision_counts.total(), 0);
        assert_eq!(build.summary.scriptlet_fidelity, "native-free");
        assert_eq!(build.summary.target_compatibility, "conary-portable");
        assert_eq!(build.summary.publication_status, "public");
        assert!(
            build
                .summary
                .evidence_digest
                .as_deref()
                .unwrap()
                .starts_with("sha256:")
        );
        build.bundle.validate().unwrap();
    }

    #[test]
    fn flattened_scriptlet_with_complete_effect_builds_replaced_entry() {
        let mut metadata = package_metadata("flat", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "/sbin/ldconfig\n".to_string(),
            flags: None,
        });
        let files = Vec::new();
        let mut classification = ScriptletClassificationReport::default();
        classification.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Known {
                reason_code: "dynamic-linker-cache-complete".to_string(),
                effects: vec![complete_effect("dynamic-linker-cache", "ldconfig")],
            },
        );

        let build = bundle_for_metadata(&metadata, &files, &classification).unwrap();

        assert_eq!(build.bundle.entries.len(), 1);
        let entry = &build.bundle.entries[0];
        assert_eq!(entry.decision.as_str(), "replaced");
        assert_eq!(entry.reason_code, "dynamic-linker-cache-complete");
        assert_eq!(entry.effects.len(), 1);
        assert_eq!(entry.body, "/sbin/ldconfig\n");
        build.bundle.validate().unwrap();
    }

    #[test]
    fn native_abi_binary_body_is_base64_encoded_and_validates() {
        let mut metadata = package_metadata("native-bin", "1.0");
        metadata
            .native_scriptlet_abi
            .push(native_entry_with_body(vec![0xff, 0x00, 0x01]));
        let files = Vec::new();
        let classification = ScriptletClassificationReport::default();

        let build = bundle_for_metadata(&metadata, &files, &classification).unwrap();
        let entry = &build.bundle.entries[0];

        assert_eq!(entry.body_encoding.as_deref(), Some("base64"));
        assert_eq!(
            entry.body_sha256,
            crate::hash::sha256_prefixed(&[0xff, 0x00, 0x01])
        );
        build.bundle.validate().unwrap();
    }

    #[test]
    fn tampered_body_after_build_fails_strict_bundle_validation() {
        let mut metadata = package_metadata("tamper", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PreInstall,
            interpreter: "/bin/sh".to_string(),
            content: "echo ok\n".to_string(),
            flags: None,
        });
        let files = Vec::new();
        let classification = ScriptletClassificationReport::default();
        let mut build = bundle_for_metadata(&metadata, &files, &classification).unwrap();

        build.bundle.entries[0].body.push_str("tampered\n");

        assert!(build.bundle.validate().is_err());
    }

    #[test]
    fn unknown_classification_becomes_source_native_legacy_replay_entry() {
        let mut metadata = package_metadata("unknown", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "custom-helper --do-thing\n".to_string(),
            flags: None,
        });
        let mut classification = ScriptletClassificationReport::default();
        classification.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Unknown {
                reason_code: "unknown-command".to_string(),
                command: "custom-helper".to_string(),
            },
        );

        let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();
        let entry = &build.bundle.entries[0];

        assert_eq!(entry.decision, ScriptletDecision::Legacy);
        assert_eq!(entry.reason_code, "unknown-command");
        assert_eq!(entry.unknown_commands, vec!["custom-helper"]);
        assert_eq!(build.bundle.decision_counts.legacy, 1);
        assert_eq!(
            build.bundle.scriptlet_fidelity,
            ScriptletFidelity::LegacyReplay
        );
        assert_eq!(
            build.bundle.target_compatibility,
            TargetCompatibility::SourceNative
        );
        assert_eq!(
            build.bundle.foreign_replay_policy,
            ForeignReplayPolicy::Deny
        );
        assert_eq!(
            build.bundle.publication_policy,
            PublicationPolicy::LocalOnly
        );
        assert_eq!(
            build.bundle.publication_status,
            PublicationStatus::LocalOnly
        );
        assert_ne!(build.bundle.publication_status, PublicationStatus::Public);
        assert_eq!(build.summary.scriptlet_fidelity, "legacy-replay");
        assert_eq!(build.summary.target_compatibility, "source-native");
        assert_eq!(build.summary.publication_status, "local-only");
        assert_eq!(build.summary.decision_counts.legacy, 1);
        assert_eq!(build.summary.unknown_commands, vec!["custom-helper"]);
    }

    #[test]
    fn review_classification_becomes_private_review_entry() {
        let mut metadata = package_metadata("review", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "systemctl restart demo.service\n".to_string(),
            flags: None,
        });
        let mut classification = ScriptletClassificationReport::default();
        classification.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Review {
                reason_code: "review-class-systemd-runtime-action".to_string(),
                class_id: Some("systemd-runtime-action".to_string()),
            },
        );

        let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();
        let entry = &build.bundle.entries[0];

        assert_eq!(entry.decision, ScriptletDecision::Review);
        assert_eq!(entry.reason_code, "review-class-systemd-runtime-action");
        assert_eq!(build.bundle.decision_counts.review, 1);
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
        assert_eq!(
            build.summary.review_reason_codes,
            vec!["review-class-systemd-runtime-action"]
        );
    }

    #[test]
    fn blocked_classification_becomes_blocked_entry() {
        let mut metadata = package_metadata("blocked", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "curl https://example.invalid\n".to_string(),
            flags: None,
        });
        let mut classification = ScriptletClassificationReport::default();
        classification.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Blocked {
                reason_code: "blocked-class-network".to_string(),
                class_id: "network".to_string(),
            },
        );

        let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();
        let entry = &build.bundle.entries[0];

        assert_eq!(entry.decision, ScriptletDecision::Blocked);
        assert_eq!(entry.reason_code, "blocked-class-network");
        assert_eq!(entry.blocked_classes, vec!["network"]);
        assert_eq!(
            build.summary.blocked_reason_codes,
            vec!["blocked-class-network"]
        );
        assert_eq!(build.summary.blocked_classes, vec!["network"]);
        assert_eq!(build.summary.publication_status, "blocked");
    }

    #[test]
    fn native_deferred_and_unpreservable_support_drive_decisions() {
        let mut metadata = package_metadata("native-support", "1.0");
        let mut deferred = native_entry_with_body(b"echo deferred\n".to_vec());
        deferred.id = "rpm:%verify".to_string();
        deferred.native_slot = "%verify".to_string();
        deferred.support = NativeScriptletSupport::DeferredReview {
            reason_code: "rpm-verify-scriptlet-deferred".to_string(),
        };
        let mut unpreservable = native_entry_with_body(b"echo nope\n".to_vec());
        unpreservable.id = "rpm:%postun".to_string();
        unpreservable.native_slot = "%postun".to_string();
        unpreservable.support = NativeScriptletSupport::Unpreservable {
            reason_code: "native-abi-parser-limitation".to_string(),
        };
        metadata.native_scriptlet_abi = vec![deferred, unpreservable];

        let build =
            bundle_for_metadata(&metadata, &[], &ScriptletClassificationReport::default()).unwrap();

        let deferred = build
            .bundle
            .entries
            .iter()
            .find(|entry| entry.id == "rpm:%verify")
            .unwrap();
        let unpreservable = build
            .bundle
            .entries
            .iter()
            .find(|entry| entry.id == "rpm:%postun")
            .unwrap();
        assert_eq!(deferred.decision, ScriptletDecision::Review);
        assert_eq!(deferred.reason_code, "rpm-verify-scriptlet-deferred");
        assert_eq!(unpreservable.decision, ScriptletDecision::Blocked);
        assert_eq!(unpreservable.reason_code, "native-abi-parser-limitation");
    }

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

    #[test]
    fn digest_changes_when_classification_evidence_changes() {
        let mut metadata = package_metadata("digest", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "ldconfig\n".to_string(),
            flags: None,
        });
        let files = Vec::new();

        let base = bundle_for_metadata(
            &metadata,
            &files,
            &known_report_with_effect(complete_effect("dynamic-linker-cache", "ldconfig")),
        )
        .unwrap()
        .bundle
        .evidence_digest;
        let mut different_adapter = complete_effect("dynamic-linker-cache", "ldconfig");
        different_adapter.adapter_digest = Some(crate::hash::sha256_prefixed(b"different"));
        let adapter_digest = bundle_for_metadata(
            &metadata,
            &files,
            &known_report_with_effect(different_adapter),
        )
        .unwrap()
        .bundle
        .evidence_digest;
        let mut partial = complete_effect("dynamic-linker-cache", "ldconfig");
        partial.replacement = EffectReplacement::Partial;
        let replacement_digest =
            bundle_for_metadata(&metadata, &files, &known_report_with_effect(partial))
                .unwrap()
                .bundle
                .evidence_digest;
        let mut unknown = ScriptletClassificationReport::default();
        unknown.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Unknown {
                reason_code: "unknown-command".to_string(),
                command: "custom-helper".to_string(),
            },
        );
        let unknown_digest = bundle_for_metadata(&metadata, &files, &unknown)
            .unwrap()
            .bundle
            .evidence_digest;
        let mut blocked = ScriptletClassificationReport::default();
        blocked.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Blocked {
                reason_code: "blocked-class-network".to_string(),
                class_id: "network".to_string(),
            },
        );
        let blocked_digest = bundle_for_metadata(&metadata, &files, &blocked)
            .unwrap()
            .bundle
            .evidence_digest;

        assert_ne!(base, adapter_digest);
        assert_ne!(base, replacement_digest);
        assert_ne!(base, unknown_digest);
        assert_ne!(base, blocked_digest);
    }
}
