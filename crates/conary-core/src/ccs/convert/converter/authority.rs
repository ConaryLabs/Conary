// conary-core/src/ccs/convert/converter/authority.rs

use super::*;

pub(super) fn classify_scriptlets(
    metadata: &PackageMetadata,
    files: &[ExtractedFile],
) -> ScriptletClassificationReport {
    let registry = AdapterRegistry::default();
    let payload = PayloadHints::from_package(metadata, files);
    let mut report = ScriptletClassificationReport::default();

    if metadata.scriptlets.is_empty() && metadata.native_scriptlet_abi.is_empty() {
        report.push("package", registry.classify_native_free_package());
        return report;
    }

    for entry in &metadata.native_scriptlet_abi {
        let classifications = match extract_native_entry_invocations(entry) {
            Ok(invocations) => invocations
                .into_iter()
                .map(|invocation| {
                    registry.classify_invocation_with_context(AdapterInput {
                        invocation: &invocation,
                        payload: &payload,
                    })
                })
                .collect::<Vec<_>>(),
            Err(_) => {
                report.push(
                    entry.id.clone(),
                    ScriptletClassification::Review {
                        reason_code: "review-class-shell-parse-error".to_string(),
                        class_id: Some("shell-parse-error".to_string()),
                        command: None,
                    },
                );
                Vec::new()
            }
        };

        let support_fully_covered =
            deferred_native_support_is_fully_adapter_covered(entry, &classifications);

        for classification in classifications {
            report.push(entry.id.clone(), classification);
        }
        if !support_fully_covered && let Some(classification) = classify_native_support(entry) {
            report.push(entry.id.clone(), classification);
        }
    }

    for (index, scriptlet) in metadata.scriptlets.iter().enumerate() {
        let entry_id = format!("scriptlet:{index}:{}", scriptlet.phase);
        match extract_scriptlet_invocations(&entry_id, scriptlet) {
            Ok(invocations) => {
                for invocation in invocations {
                    report.push(
                        entry_id.clone(),
                        registry.classify_invocation_with_context(AdapterInput {
                            invocation: &invocation,
                            payload: &payload,
                        }),
                    );
                }
            }
            Err(_) => report.push(
                entry_id,
                ScriptletClassification::Review {
                    reason_code: "review-class-shell-parse-error".to_string(),
                    class_id: Some("shell-parse-error".to_string()),
                    command: None,
                },
            ),
        }
    }

    report
}

pub(super) fn manifest_hooks_from_complete_adapter_effects(
    classification: &ScriptletClassificationReport,
) -> Hooks {
    let mut hooks = Hooks::default();

    for entry in &classification.entries {
        let ScriptletClassification::Known { effects, .. } = &entry.classification else {
            continue;
        };

        for effect in effects {
            if effect.adapter_id.as_deref() != Some("sysctl/v1")
                || effect.kind != "sysctl-setting"
                || effect.replacement != crate::ccs::legacy_scriptlets::EffectReplacement::Complete
            {
                continue;
            }

            let Some(key) = effect.extra.get("key").and_then(toml::Value::as_str) else {
                continue;
            };
            let Some(value) = effect.extra.get("value").and_then(toml::Value::as_str) else {
                continue;
            };
            let only_if_lower = effect
                .extra
                .get("only_if_lower")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);

            hooks.sysctl.push(SysctlHook {
                key: key.to_string(),
                value: value.to_string(),
                only_if_lower,
                reversible: Some(true),
            });
        }
    }

    hooks
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SetuidModeUpdate {
    path: String,
    target_mode: u32,
}

pub(super) fn setuid_mode_updates_from_complete_adapter_effects(
    classification: &ScriptletClassificationReport,
) -> Vec<SetuidModeUpdate> {
    let mut updates = std::collections::BTreeMap::new();

    for entry in &classification.entries {
        let ScriptletClassification::Known { effects, .. } = &entry.classification else {
            continue;
        };

        for effect in effects {
            if effect.adapter_id.as_deref() != Some("setuid-mode/v1")
                || effect.kind != "setuid-mode"
                || effect.replacement != crate::ccs::legacy_scriptlets::EffectReplacement::Complete
            {
                continue;
            }

            let Some(path) = effect.path.as_deref() else {
                continue;
            };
            let Some(target_mode) = effect
                .extra
                .get("target_mode")
                .and_then(toml::Value::as_integer)
                .and_then(|value| u32::try_from(value).ok())
            else {
                continue;
            };
            if target_mode & 0o4000 == 0 || target_mode & 0o2000 != 0 {
                continue;
            }

            updates.insert(path.to_string(), target_mode);
        }
    }

    updates
        .into_iter()
        .map(|(path, target_mode)| SetuidModeUpdate { path, target_mode })
        .collect()
}

pub(super) fn apply_setuid_mode_updates(
    files: &mut [ExtractedFile],
    updates: &[SetuidModeUpdate],
) -> Result<(), ConversionError> {
    for update in updates {
        let Some(file) = files.iter_mut().find(|file| file.path == update.path) else {
            return Err(ConversionError::ManifestError(format!(
                "setuid adapter referenced missing payload path {}",
                update.path
            )));
        };
        let existing_mode = file.mode as u32;
        file.mode = ((existing_mode & !0o7777) | update.target_mode) as i32;
    }
    Ok(())
}

pub(super) fn apply_setuid_policy_allowlist(
    manifest: &mut CcsManifest,
    updates: &[SetuidModeUpdate],
) {
    manifest
        .policy
        .allow_setuid_paths
        .extend(updates.iter().map(|update| update.path.clone()));
    manifest.policy.allow_setuid_paths.sort();
    manifest.policy.allow_setuid_paths.dedup();
}

pub(super) fn file_capability_updates_from_complete_adapter_effects(
    classification: &ScriptletClassificationReport,
) -> Result<Vec<FileCapability>, ConversionError> {
    let mut updates = std::collections::BTreeMap::<String, FileCapability>::new();

    for entry in &classification.entries {
        let ScriptletClassification::Known { effects, .. } = &entry.classification else {
            continue;
        };

        for effect in effects {
            if effect.adapter_id.as_deref() != Some("file-capability/v1")
                || effect.kind != "file-capability"
                || effect.replacement != crate::ccs::legacy_scriptlets::EffectReplacement::Complete
            {
                continue;
            }

            let Some(path) = effect.path.as_deref() else {
                continue;
            };
            let capabilities = effect
                .extra
                .get("capabilities")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();
            let permitted = effect
                .extra
                .get("permitted")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true);
            let effective = effect
                .extra
                .get("effective")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true);
            let inheritable = effect
                .extra
                .get("inheritable")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);

            let capability = FileCapability {
                path: path.to_string(),
                capabilities,
                permitted,
                effective,
                inheritable,
            };
            capability.validate().map_err(|error| {
                ConversionError::ManifestError(format!(
                    "file capability adapter produced invalid manifest authority: {error}"
                ))
            })?;

            updates
                .entry(path.to_string())
                .and_modify(|existing| {
                    if existing.permitted == capability.permitted
                        && existing.effective == capability.effective
                        && existing.inheritable == capability.inheritable
                    {
                        existing
                            .capabilities
                            .extend(capability.capabilities.iter().cloned());
                        existing.capabilities.sort();
                        existing.capabilities.dedup();
                    }
                })
                .or_insert(capability);
        }
    }

    Ok(updates.into_values().collect())
}

pub(super) fn apply_file_capability_authority(
    manifest: &mut CcsManifest,
    updates: &[FileCapability],
) {
    manifest.file_capabilities.extend(updates.iter().cloned());
    manifest.file_capabilities.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.capabilities.cmp(&right.capabilities))
    });
    manifest.file_capabilities.dedup();
}

pub(super) fn deferred_native_support_is_fully_adapter_covered(
    entry: &NativeScriptletEntry,
    classifications: &[ScriptletClassification],
) -> bool {
    if !native_deferred_support_can_be_adapter_covered(entry) || classifications.is_empty() {
        return false;
    }

    classifications
        .iter()
        .all(classification_is_complete_adapter_coverage)
}

pub(super) fn classify_native_support(
    entry: &NativeScriptletEntry,
) -> Option<ScriptletClassification> {
    match &entry.support {
        NativeScriptletSupport::Parsed => None,
        NativeScriptletSupport::DeferredReview { reason_code } => {
            Some(ScriptletClassification::Review {
                reason_code: reason_code.clone(),
                class_id: native_review_class_id(entry),
                command: None,
            })
        }
        NativeScriptletSupport::Unpreservable { reason_code } => {
            Some(ScriptletClassification::Blocked {
                reason_code: reason_code.clone(),
                class_id: "native-abi-unpreservable".to_string(),
                command: None,
            })
        }
    }
}

pub(super) fn native_review_class_id(entry: &NativeScriptletEntry) -> Option<String> {
    match &entry.metadata {
        NativeScriptletMetadata::Rpm(metadata) => {
            if metadata.slot == RpmScriptletSlot::Verify
                || entry.lifecycle_paths.contains(&NativeLifecyclePath::Verify)
            {
                return Some("rpm-verify".to_string());
            }
            if metadata.slot == RpmScriptletSlot::Trigger
                || metadata.trigger.is_some()
                || entry.lifecycle_paths.iter().any(|path| {
                    matches!(
                        path,
                        NativeLifecyclePath::Trigger
                            | NativeLifecyclePath::FileTrigger
                            | NativeLifecyclePath::TransactionFileTrigger
                    )
                })
            {
                return Some("rpm-trigger".to_string());
            }
        }
        NativeScriptletMetadata::Deb(metadata) => {
            if metadata.control_member == DebControlMember::Triggers
                || !metadata.trigger_declarations.is_empty()
                || entry
                    .lifecycle_paths
                    .contains(&NativeLifecyclePath::Trigger)
            {
                return Some("deb-trigger".to_string());
            }
            if metadata.control_member == DebControlMember::Config
                || entry.lifecycle_paths.contains(&NativeLifecyclePath::Config)
            {
                return Some("debconf".to_string());
            }
        }
        NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(_)) => {
            return Some("arch-alpm-hook".to_string());
        }
        NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(_)) => {
            return Some("arch-install-function".to_string());
        }
    }

    None
}
