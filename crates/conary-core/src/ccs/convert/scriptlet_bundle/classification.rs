// conary-core/src/ccs/convert/scriptlet_bundle/classification.rs

use crate::ccs::convert::effects::{
    EntryClassification, ScriptletClassification, ScriptletClassificationReport,
    ScriptletCommandEvidence, ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::{
    BootSecurityIntentEvidence, EffectReplacement, ScriptletDecision, ScriptletEffect,
};
use crate::packages::native_abi::NativeScriptletSupport;
use std::collections::BTreeSet;

pub(super) struct EntryOutcome {
    pub(super) decision: ScriptletDecision,
    pub(super) reason_code: String,
    pub(super) effects: Vec<ScriptletEffect>,
    pub(super) unknown_commands: Vec<String>,
    pub(super) blocked_classes: Vec<String>,
    pub(super) boot_security_intents: Vec<BootSecurityIntentEvidence>,
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
