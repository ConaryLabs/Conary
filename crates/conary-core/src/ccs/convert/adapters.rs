// conary-core/src/ccs/convert/adapters.rs

use crate::ccs::convert::apparmor_adapters::ApparmorPolicyAdapter;
use crate::ccs::convert::blocked_classes::{BlockedClassOutcome, BlockedClassRegistry};
use crate::ccs::convert::command_evidence::{CommandEvidenceSource, CommandInvocation};
use crate::ccs::convert::debian_adapters::{DebSystemdHelperAdapter, DpkgMaintscriptHelperAdapter};
use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletCommandEvidence, ScriptletEffectEvidence,
};
use crate::ccs::convert::payload_hints::PayloadHints;
use crate::ccs::convert::selinux_adapters::SelinuxPolicyAdapter;
use crate::ccs::hooks::{validate_sysctl_key, validate_sysctl_value};
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use crate::ccs::manifest::is_supported_linux_file_capability;
use std::collections::{BTreeMap, BTreeSet};

mod builtin;
use builtin::{
    AlternativesRegistrationAdapter, CacheRefreshAdapter, FileCapabilityAdapter, LdconfigAdapter,
    NativeFreeAdapter, SetuidModeAdapter, SysctlAdapter, SystemdDaemonReloadAdapter,
    SystemdSysusersAdapter, SystemdTmpfilesCreateAdapter, SystemdUnitStateAdapter,
};

const PARTIAL_COVERAGE_REASON: &str = "known-helper-partial-coverage";
const LDCONFIG_COMPLETE_REASON: &str = "helper-complete-ldconfig";
const SYSTEMD_DAEMON_RELOAD_COMPLETE_REASON: &str = "helper-complete-systemd-daemon-reload";
const SYSTEMD_UNIT_STATE_COMPLETE_REASON: &str = "helper-complete-systemd-unit-state";
const TMPFILES_CREATE_COMPLETE_REASON: &str = "helper-complete-tmpfiles-create";
const SYSUSERS_COMPLETE_REASON: &str = "helper-complete-sysusers";
const SYSCTL_COMPLETE_REASON: &str = "helper-complete-sysctl";
const SETUID_COMPLETE_REASON: &str = "helper-complete-setuid-mode";
const FILE_CAPABILITY_COMPLETE_REASON: &str = "helper-complete-file-capability";
const ALTERNATIVES_COMPLETE_REASON: &str = "helper-complete-alternatives-registration";
const CACHE_REFRESH_COMPLETE_REASON: &str = "helper-complete-cache-refresh";
const ALTERNATIVES_REVIEW_REASON: &str = "review-class-alternatives-interactive-or-broad";
const CACHE_REFRESH_REVIEW_REASON: &str = "review-class-cache-refresh-nonstandard";

#[derive(Debug, Clone, Copy)]
pub struct AdapterInput<'a> {
    pub invocation: &'a CommandInvocation,
    pub payload: &'a PayloadHints,
}

pub trait ScriptletEffectAdapter {
    fn id(&self) -> &'static str;
    fn digest(&self) -> String;
    fn command_names(&self) -> &'static [&'static str];
    fn matches(&self, input: AdapterInput<'_>) -> bool;
    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification;
    fn is_authoritative(&self, input: AdapterInput<'_>) -> bool {
        invocation_has_authoritative_semantics(input.invocation)
    }
}

pub struct AdapterRegistry {
    adapters: Vec<Box<dyn ScriptletEffectAdapter + Send + Sync>>,
    blocked_classes: BlockedClassRegistry,
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        let adapters: Vec<Box<dyn ScriptletEffectAdapter + Send + Sync>> = vec![
            Box::new(NativeFreeAdapter),
            Box::new(LdconfigAdapter),
            Box::new(SystemdDaemonReloadAdapter),
            Box::new(SystemdUnitStateAdapter),
            Box::new(DebSystemdHelperAdapter),
            Box::new(DpkgMaintscriptHelperAdapter),
            Box::new(SystemdTmpfilesCreateAdapter),
            Box::new(SystemdSysusersAdapter),
            Box::new(SysctlAdapter),
            Box::new(SetuidModeAdapter),
            Box::new(FileCapabilityAdapter),
            Box::new(SelinuxPolicyAdapter),
            Box::new(ApparmorPolicyAdapter),
            Box::new(AlternativesRegistrationAdapter),
            Box::new(CacheRefreshAdapter),
        ];
        assert_unique_adapter_ids(&adapters);

        Self {
            adapters,
            blocked_classes: BlockedClassRegistry::default(),
        }
    }
}

impl AdapterRegistry {
    pub fn adapter_ids(&self) -> Vec<&'static str> {
        self.adapters.iter().map(|adapter| adapter.id()).collect()
    }

    #[cfg(test)]
    fn adapters_for_testing(&self) -> Vec<&(dyn ScriptletEffectAdapter + Send + Sync)> {
        self.adapters
            .iter()
            .map(|adapter| adapter.as_ref())
            .collect()
    }

    pub fn classify_invocation_with_context(
        &self,
        input: AdapterInput<'_>,
    ) -> ScriptletClassification {
        let class_fallback =
            if let Some(class) = self.blocked_classes.match_invocation(input.invocation) {
                let command = Some(ScriptletCommandEvidence::from_invocation(input.invocation));
                match class.default_outcome {
                    BlockedClassOutcome::Blocked => Some(ClassFallback {
                        class_id: class.id,
                        outcome: class.default_outcome,
                        classification: ScriptletClassification::Blocked {
                            reason_code: class.reason_code.to_string(),
                            class_id: class.id.to_string(),
                            command,
                        },
                    }),
                    BlockedClassOutcome::Review => Some(ClassFallback {
                        class_id: class.id,
                        outcome: class.default_outcome,
                        classification: ScriptletClassification::Review {
                            reason_code: class.reason_code.to_string(),
                            class_id: Some(class.id.to_string()),
                            command,
                        },
                    }),
                }
            } else {
                None
            };

        let adapter_classification = self
            .adapters
            .iter()
            .find(|adapter| adapter.matches(input))
            .map(|adapter| {
                enforce_adapter_authority(
                    input,
                    adapter.is_authoritative(input),
                    adapter.classify(input),
                )
            });

        match (class_fallback, adapter_classification) {
            (Some(fallback), Some(classification))
                if fallback.outcome == BlockedClassOutcome::Review
                    && classification_has_adapter_effects(&classification) =>
            {
                classification
            }
            (Some(fallback), Some(classification))
                if fallback.outcome == BlockedClassOutcome::Blocked
                    && blocked_class_can_be_adapter_modeled(fallback.class_id)
                    && classification_has_complete_adapter_replacement(&classification) =>
            {
                classification
            }
            (Some(fallback), _) => fallback.classification,
            (None, Some(classification)) => classification,
            (None, None) => ScriptletClassification::Unknown {
                reason_code: "unknown-command".to_string(),
                command: ScriptletCommandEvidence::from_invocation(input.invocation),
            },
        }
    }

    pub fn classify_invocation(&self, invocation: &CommandInvocation) -> ScriptletClassification {
        let payload = PayloadHints::default();
        self.classify_invocation_with_context(AdapterInput {
            invocation,
            payload: &payload,
        })
    }

    /// Native-free classification is package-level evidence, not per-command
    /// dispatch. `NativeFreeAdapter` remains in the registry so support-matrix
    /// coverage and adapter digests include the no-scriptlet case.
    pub fn classify_native_free_package(&self) -> ScriptletClassification {
        let adapter = self
            .adapters
            .iter()
            .find(|adapter| adapter.id() == "native-free/v1")
            .expect("default registry must include native-free/v1");

        ScriptletClassification::Known {
            reason_code: "native-free-no-scriptlets".to_string(),
            effects: vec![ScriptletEffectEvidence {
                kind: "no-scriptlet".to_string(),
                source: EffectSource::NativeShellAst,
                confidence: EffectConfidence::Declared,
                replacement: EffectReplacement::Complete,
                adapter_id: Some(adapter.id().to_string()),
                adapter_digest: Some(adapter.digest()),
                command: None,
                args: vec![],
                path: None,
                reason_code: Some("native-free-no-scriptlets".to_string()),
                extra: BTreeMap::new(),
            }],
        }
    }
}

fn enforce_adapter_authority(
    input: AdapterInput<'_>,
    authoritative: bool,
    mut classification: ScriptletClassification,
) -> ScriptletClassification {
    if authoritative {
        return classification;
    }
    let ScriptletClassification::Known {
        reason_code,
        effects,
    } = &mut classification
    else {
        return classification;
    };
    if !effects
        .iter()
        .any(|effect| effect.replacement == EffectReplacement::Complete)
    {
        return classification;
    }

    *reason_code = "review-class-helper-form-not-authoritative".to_string();
    for effect in effects {
        if effect.replacement == EffectReplacement::Complete {
            effect.replacement = EffectReplacement::Partial;
        }
        effect.reason_code = Some(reason_code.clone());
        effect.extra.insert(
            "authority".to_string(),
            toml::Value::String("discovery-only".to_string()),
        );
        effect.extra.insert(
            "execution_context".to_string(),
            toml::Value::String(
                format!("{:?}", input.invocation.execution_context).to_ascii_lowercase(),
            ),
        );
    }
    classification
}

fn invocation_has_authoritative_semantics(invocation: &CommandInvocation) -> bool {
    if invocation.source == CommandEvidenceSource::HelperGrammar {
        return true;
    }
    matches!(
        invocation.source,
        CommandEvidenceSource::ShellAst | CommandEvidenceSource::NativeShellAst
    ) && invocation.command_provenance
        == crate::ccs::legacy_scriptlets::CommandArgumentProvenance::Literal
        && invocation.argument_provenance.iter().all(|provenance| {
            *provenance == crate::ccs::legacy_scriptlets::CommandArgumentProvenance::Literal
        })
        && invocation.execution_context
            == crate::ccs::legacy_scriptlets::CommandExecutionContext::Unconditional
}

struct ClassFallback {
    class_id: &'static str,
    outcome: BlockedClassOutcome,
    classification: ScriptletClassification,
}

fn classification_has_adapter_effects(classification: &ScriptletClassification) -> bool {
    let ScriptletClassification::Known { effects, .. } = classification else {
        return false;
    };

    !effects.is_empty()
        && effects.iter().all(|effect| {
            effect
                .adapter_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
        })
}

fn classification_has_complete_adapter_replacement(
    classification: &ScriptletClassification,
) -> bool {
    let ScriptletClassification::Known { effects, .. } = classification else {
        return false;
    };

    !effects.is_empty()
        && effects.iter().all(|effect| {
            effect
                .adapter_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
                && effect.replacement == EffectReplacement::Complete
        })
}

fn blocked_class_can_be_adapter_modeled(class_id: &str) -> bool {
    matches!(
        class_id,
        "selinux" | "apparmor" | "sysctl" | "setuid-setcap"
    )
}

fn effect_source(source: CommandEvidenceSource) -> EffectSource {
    match source {
        CommandEvidenceSource::ShellAst => EffectSource::ShellAst,
        CommandEvidenceSource::NativeShellAst => EffectSource::NativeShellAst,
        CommandEvidenceSource::PackageMetadata => EffectSource::PackageMetadata,
        CommandEvidenceSource::HelperGrammar => EffectSource::HelperGrammar,
        CommandEvidenceSource::Unresolved => EffectSource::Unknown("unresolved".to_string()),
    }
}

fn assert_unique_adapter_ids(adapters: &[Box<dyn ScriptletEffectAdapter + Send + Sync>]) {
    let mut seen = BTreeSet::new();
    for adapter in adapters {
        assert!(
            seen.insert(adapter.id()),
            "duplicate scriptlet adapter id: {}",
            adapter.id()
        );
    }
}

#[cfg(test)]
mod tests;
