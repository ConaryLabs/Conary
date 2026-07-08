// conary-core/src/ccs/convert/scriptlet_bundle/classification.rs

use crate::ccs::convert::effects::{
    EntryClassification, ScriptletClassification, ScriptletClassificationReport,
    ScriptletCommandEvidence, ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::{
    BootSecurityIntentEvidence, EffectReplacement, ScriptletDecision, ScriptletEffect,
};
use crate::ccs::security_policy::{
    SECURITY_POLICY_INTENT_SCHEMA_V1, SecurityPolicyFallback, SecurityPolicyIntent,
    SecurityPolicyPayloadEvidence, SecurityPolicyProvider, SecurityPolicyReconciliation,
    SecurityPolicyReconciliationState, SecurityPolicyRequirements, SecurityPolicyScope,
    SecurityPolicySource,
};
use crate::packages::native_abi::NativeScriptletSupport;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct EntryOutcome {
    pub(super) decision: ScriptletDecision,
    pub(super) reason_code: String,
    pub(super) effects: Vec<ScriptletEffect>,
    pub(super) unknown_commands: Vec<String>,
    pub(super) blocked_classes: Vec<String>,
    pub(super) boot_security_intents: Vec<BootSecurityIntentEvidence>,
    pub(super) security_policy_intents: Vec<SecurityPolicyIntent>,
}

pub(super) fn classify_entry(
    classifications: &[&EntryClassification],
    support: &NativeScriptletSupport,
) -> EntryOutcome {
    let effects = classifications
        .iter()
        .flat_map(|entry| match &entry.classification {
            ScriptletClassification::Known { effects, .. } => effects
                .iter()
                .map(scriptlet_effect_from_evidence)
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    let unknown_commands = classifications
        .iter()
        .filter_map(|entry| match &entry.classification {
            ScriptletClassification::Unknown { command, .. } => Some(command.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let blocked_classes = classifications
        .iter()
        .filter_map(|entry| match &entry.classification {
            ScriptletClassification::Blocked { class_id, .. } => Some(class_id.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let boot_security_intents = classifications
        .iter()
        .filter_map(|entry| boot_security_intent_from_classification(&entry.classification))
        .collect::<Vec<_>>();
    let security_policy_intents = classifications
        .iter()
        .filter_map(|entry| {
            security_policy_intent_from_classification(&entry.entry_id, &entry.classification)
        })
        .collect::<Vec<_>>();

    if let Some(reason_code) =
        classifications
            .iter()
            .find_map(|entry| match &entry.classification {
                ScriptletClassification::Blocked { reason_code, .. } => Some(reason_code.clone()),
                _ => None,
            })
    {
        return EntryOutcome {
            decision: ScriptletDecision::Blocked,
            reason_code,
            effects,
            unknown_commands,
            blocked_classes,
            boot_security_intents,
            security_policy_intents,
        };
    }

    if let NativeScriptletSupport::Unpreservable { reason_code } = support {
        return EntryOutcome {
            decision: ScriptletDecision::Blocked,
            reason_code: reason_code.clone(),
            effects,
            unknown_commands,
            blocked_classes,
            boot_security_intents,
            security_policy_intents,
        };
    }

    if let Some(reason_code) =
        classifications
            .iter()
            .find_map(|entry| match &entry.classification {
                ScriptletClassification::Review { reason_code, .. } => Some(reason_code.clone()),
                _ => None,
            })
    {
        return EntryOutcome {
            decision: ScriptletDecision::Review,
            reason_code,
            effects,
            unknown_commands,
            blocked_classes,
            boot_security_intents,
            security_policy_intents,
        };
    }

    if let NativeScriptletSupport::DeferredReview { reason_code } = support {
        return EntryOutcome {
            decision: ScriptletDecision::Review,
            reason_code: reason_code.clone(),
            effects,
            unknown_commands,
            blocked_classes,
            boot_security_intents,
            security_policy_intents,
        };
    }

    if let Some(reason_code) =
        classifications
            .iter()
            .find_map(|entry| match &entry.classification {
                ScriptletClassification::Unknown { reason_code, .. } => Some(reason_code.clone()),
                _ => None,
            })
    {
        return EntryOutcome {
            decision: ScriptletDecision::Legacy,
            reason_code,
            effects,
            unknown_commands,
            blocked_classes,
            boot_security_intents,
            security_policy_intents,
        };
    }

    let known_reason = classifications
        .iter()
        .find_map(|entry| match &entry.classification {
            ScriptletClassification::Known { reason_code, .. } => Some(reason_code.clone()),
            _ => None,
        });
    if let Some(reason_code) = known_reason {
        let all_complete = !effects.is_empty()
            && effects
                .iter()
                .all(|effect| effect.replacement == EffectReplacement::Complete);
        return EntryOutcome {
            decision: if all_complete {
                ScriptletDecision::Replaced
            } else {
                ScriptletDecision::Review
            },
            reason_code,
            effects,
            unknown_commands,
            blocked_classes,
            boot_security_intents,
            security_policy_intents,
        };
    }

    EntryOutcome {
        decision: ScriptletDecision::Review,
        reason_code: support
            .reason_code()
            .unwrap_or("scriptlet-preserved-for-review")
            .to_string(),
        effects,
        unknown_commands,
        blocked_classes,
        boot_security_intents,
        security_policy_intents,
    }
}

fn boot_security_intent_from_classification(
    classification: &ScriptletClassification,
) -> Option<BootSecurityIntentEvidence> {
    match classification {
        ScriptletClassification::Blocked {
            reason_code,
            class_id,
            command: Some(command),
        }
        | ScriptletClassification::Review {
            reason_code,
            class_id: Some(class_id),
            command: Some(command),
        } if is_boot_security_class(class_id) => {
            Some(intent_from_command(class_id, reason_code, command))
        }
        _ => None,
    }
}

fn intent_from_command(
    class_id: &str,
    reason_code: &str,
    command: &ScriptletCommandEvidence,
) -> BootSecurityIntentEvidence {
    BootSecurityIntentEvidence {
        class_id: class_id.to_string(),
        reason_code: reason_code.to_string(),
        command: command.command.clone(),
        argv: command.argv.clone(),
        phase: command.phase.clone(),
        lifecycle_paths: command.lifecycle_paths.clone(),
    }
}

pub(super) fn security_policy_intent_from_classification(
    entry_id: &str,
    classification: &ScriptletClassification,
) -> Option<SecurityPolicyIntent> {
    let (reason_code, class_id, command) = match classification {
        ScriptletClassification::Blocked {
            reason_code,
            class_id,
            command: Some(command),
        }
        | ScriptletClassification::Review {
            reason_code,
            class_id: Some(class_id),
            command: Some(command),
        } => (reason_code, class_id, command),
        _ => return None,
    };

    if class_id != "apparmor" {
        return None;
    }

    Some(SecurityPolicyIntent {
        schema: SECURITY_POLICY_INTENT_SCHEMA_V1.to_string(),
        id: format!("{entry_id}:apparmor:{}", command.command),
        source: SecurityPolicySource {
            source_format: None,
            source_distro: None,
            entry_id: Some(entry_id.to_string()),
            command: Some(command.command.clone()),
            argv: command.argv.clone(),
            adapter_id: None,
        },
        provider: SecurityPolicyProvider::Apparmor,
        operation: apparmor_operation(&command.command).to_string(),
        scope: SecurityPolicyScope {
            kind: "profile".to_string(),
            name: command
                .argv
                .iter()
                .find(|arg| !arg.starts_with('-'))
                .cloned(),
            paths: command
                .argv
                .iter()
                .filter(|arg| arg.starts_with('/'))
                .cloned()
                .collect(),
            service: None,
            port: None,
            extra: BTreeMap::new(),
        },
        desired_state: BTreeMap::new(),
        requirements: SecurityPolicyRequirements {
            required_on_active_provider: false,
            provider_mode: None,
            tools: vec![command.command.clone()],
            modules: Vec::new(),
        },
        fallback: SecurityPolicyFallback::BlockOnEnforcingTarget,
        payload_evidence: SecurityPolicyPayloadEvidence::default(),
        reconciliation: SecurityPolicyReconciliation {
            state: SecurityPolicyReconciliationState::Review,
            reason: Some(reason_code.clone()),
            target_provider: None,
        },
        extra: BTreeMap::new(),
    })
}

fn apparmor_operation(command: &str) -> &'static str {
    match command {
        "apparmor_parser" => "profile-reload",
        "aa-enforce" => "mode-enforce",
        "aa-complain" => "mode-complain",
        "aa-disable" => "profile-disable",
        "aa-status" => "status-query",
        _ => "profile-operation",
    }
}

fn is_boot_security_class(class_id: &str) -> bool {
    matches!(
        class_id,
        "kernel-module" | "initramfs" | "bootloader" | "selinux"
    )
}

pub(super) fn classification_entries_for<'a>(
    report: &'a ScriptletClassificationReport,
    id: &str,
) -> Vec<&'a EntryClassification> {
    report
        .entries
        .iter()
        .filter(|entry| entry.entry_id == id)
        .collect()
}

fn scriptlet_effect_from_evidence(evidence: &ScriptletEffectEvidence) -> ScriptletEffect {
    ScriptletEffect {
        kind: evidence.kind.clone(),
        source: evidence.source.clone(),
        confidence: evidence.confidence.clone(),
        replacement: evidence.replacement.clone(),
        adapter_id: evidence.adapter_id.clone(),
        adapter_digest: evidence.adapter_digest.clone(),
        command: evidence.command.clone(),
        args: evidence.args.clone(),
        path: evidence.path.clone(),
        reason_code: evidence.reason_code.clone(),
        extra: evidence.extra.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::security_policy_intent_from_classification;
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletCommandEvidence};

    #[test]
    fn blocked_apparmor_command_projects_review_policy_intent() {
        let classification = ScriptletClassification::Blocked {
            reason_code: "blocked-class-apparmor".to_string(),
            class_id: "apparmor".to_string(),
            command: Some(ScriptletCommandEvidence {
                command: "apparmor_parser".to_string(),
                argv: vec!["-r".to_string(), "/etc/apparmor.d/usr.bin.demo".to_string()],
                phase: Some("post-install".to_string()),
                lifecycle_paths: vec!["post-install".to_string()],
                raw_line: None,
                source: "static-signal".to_string(),
                environment: Vec::new(),
            }),
        };

        let intent =
            security_policy_intent_from_classification("scriptlet:0:post-install", &classification)
                .expect("apparmor intent");

        assert_eq!(intent.provider.as_str(), "apparmor");
        assert_eq!(intent.operation, "profile-reload");
        assert_eq!(intent.scope.kind, "profile");
        assert_eq!(intent.fallback.as_str(), "block-on-enforcing-target");
        assert_eq!(intent.reconciliation.state.as_str(), "review");
        assert_eq!(intent.source.command.as_deref(), Some("apparmor_parser"));
    }
}
