// conary-core/src/ccs/convert/adapters.rs

use crate::ccs::convert::blocked_classes::{BlockedClassOutcome, BlockedClassRegistry};
use crate::ccs::convert::command_evidence::{CommandEvidenceSource, CommandInvocation};
use crate::ccs::convert::effects::{ScriptletClassification, ScriptletEffectEvidence};
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use std::collections::BTreeSet;

const KNOWN_HELPER_REASON: &str = "known-helper-requires-adapter-coverage";

pub trait ScriptletEffectAdapter {
    fn id(&self) -> &'static str;
    fn digest(&self) -> String;
    fn command_names(&self) -> &'static [&'static str];
    fn matches(&self, invocation: &CommandInvocation) -> bool;
    fn classify(&self, invocation: &CommandInvocation) -> ScriptletClassification;
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
            Box::new(SystemdEnableDisableAdapter),
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

    pub fn classify_invocation(&self, invocation: &CommandInvocation) -> ScriptletClassification {
        if let Some(class) = self.blocked_classes.match_invocation(invocation) {
            return match class.default_outcome {
                BlockedClassOutcome::Blocked => ScriptletClassification::Blocked {
                    reason_code: class.reason_code.to_string(),
                    class_id: class.id.to_string(),
                },
                BlockedClassOutcome::Review => ScriptletClassification::Review {
                    reason_code: class.reason_code.to_string(),
                    class_id: Some(class.id.to_string()),
                },
            };
        }

        self.adapters
            .iter()
            .find(|adapter| adapter.matches(invocation))
            .map_or_else(
                || ScriptletClassification::Unknown {
                    reason_code: "unknown-command".to_string(),
                    command: invocation.command.clone(),
                },
                |adapter| adapter.classify(invocation),
            )
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
                source: EffectSource::NativeMetadata,
                confidence: EffectConfidence::Declared,
                replacement: EffectReplacement::Complete,
                adapter_id: Some(adapter.id().to_string()),
                adapter_digest: Some(adapter.digest()),
                command: None,
                args: vec![],
                path: None,
                reason_code: Some("native-free-no-scriptlets".to_string()),
            }],
        }
    }
}

struct NativeFreeAdapter;
struct LdconfigAdapter;
struct SystemdDaemonReloadAdapter;
struct SystemdEnableDisableAdapter;

impl ScriptletEffectAdapter for NativeFreeAdapter {
    fn id(&self) -> &'static str {
        "native-free/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"native-free/v1:no-scriptlet:complete")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &[]
    }

    fn matches(&self, _invocation: &CommandInvocation) -> bool {
        false
    }

    fn classify(&self, _invocation: &CommandInvocation) -> ScriptletClassification {
        unreachable!("native-free is package-level evidence")
    }
}

impl ScriptletEffectAdapter for LdconfigAdapter {
    fn id(&self) -> &'static str {
        "ldconfig/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"ldconfig/v1:dynamic-linker-cache:none")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["ldconfig"]
    }

    fn matches(&self, invocation: &CommandInvocation) -> bool {
        invocation.command == "ldconfig"
    }

    fn classify(&self, invocation: &CommandInvocation) -> ScriptletClassification {
        known_effect_classification(
            self,
            invocation,
            "dynamic-linker-cache",
            EffectReplacement::None,
            None,
        )
    }
}

impl ScriptletEffectAdapter for SystemdDaemonReloadAdapter {
    fn id(&self) -> &'static str {
        "systemd-daemon-reload/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"systemd-daemon-reload/v1:systemd-daemon-reload:none")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["systemctl"]
    }

    fn matches(&self, invocation: &CommandInvocation) -> bool {
        invocation.command == "systemctl"
            && invocation
                .argv
                .first()
                .is_some_and(|action| action == "daemon-reload")
    }

    fn classify(&self, invocation: &CommandInvocation) -> ScriptletClassification {
        known_effect_classification(
            self,
            invocation,
            "systemd-daemon-reload",
            EffectReplacement::None,
            None,
        )
    }
}

impl ScriptletEffectAdapter for SystemdEnableDisableAdapter {
    fn id(&self) -> &'static str {
        "systemd-enable-disable/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"systemd-enable-disable/v1:systemd-unit-enable-disable:none")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["systemctl"]
    }

    fn matches(&self, invocation: &CommandInvocation) -> bool {
        invocation.command == "systemctl"
            && invocation
                .argv
                .first()
                .is_some_and(|action| matches!(action.as_str(), "enable" | "disable"))
            && invocation.argv.len() > 1
    }

    fn classify(&self, invocation: &CommandInvocation) -> ScriptletClassification {
        let action = invocation
            .argv
            .first()
            .map(String::as_str)
            .unwrap_or("enable");
        let kind = format!("systemd-unit-{action}");
        let unit = invocation.argv.get(1).cloned();
        known_effect_classification(self, invocation, &kind, EffectReplacement::None, unit)
    }
}

fn known_effect_classification(
    adapter: &dyn ScriptletEffectAdapter,
    invocation: &CommandInvocation,
    kind: &str,
    replacement: EffectReplacement,
    path: Option<String>,
) -> ScriptletClassification {
    ScriptletClassification::Known {
        reason_code: KNOWN_HELPER_REASON.to_string(),
        effects: vec![ScriptletEffectEvidence {
            kind: kind.to_string(),
            source: effect_source(invocation.source),
            confidence: EffectConfidence::Inferred,
            replacement,
            adapter_id: Some(adapter.id().to_string()),
            adapter_digest: Some(adapter.digest()),
            command: Some(invocation.command.clone()),
            args: invocation.argv.clone(),
            path,
            reason_code: Some(KNOWN_HELPER_REASON.to_string()),
        }],
    }
}

fn effect_source(source: CommandEvidenceSource) -> EffectSource {
    match source {
        CommandEvidenceSource::StaticSignal => EffectSource::StaticSignal,
        CommandEvidenceSource::CaptureLog => EffectSource::CaptureLog,
        CommandEvidenceSource::NativeMetadata => EffectSource::NativeMetadata,
        CommandEvidenceSource::PayloadHeuristic => EffectSource::PayloadHeuristic,
        CommandEvidenceSource::CuratedRule => EffectSource::CuratedRule,
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
mod tests {
    use super::*;
    use crate::ccs::convert::command_evidence::{CommandEvidenceSource, CommandInvocation};
    use crate::ccs::convert::effects::ScriptletClassification;

    fn invocation(command: &str, argv: &[&str]) -> CommandInvocation {
        CommandInvocation {
            id: format!("entry:line0:cmd0:{command}"),
            entry_id: "entry".to_string(),
            source: CommandEvidenceSource::StaticSignal,
            phase: Some("post-install".to_string()),
            lifecycle_paths: vec!["post-install".to_string()],
            interpreter: Some("/bin/sh".to_string()),
            command: command.to_string(),
            argv: argv.iter().map(|arg| arg.to_string()).collect(),
            raw_line: Some(format!("{} {}", command, argv.join(" ")).trim().to_string()),
            cwd: None,
            environment: vec![],
        }
    }

    #[test]
    fn adapter_registry_classifies_known_helpers_without_complete_replacement() {
        let registry = AdapterRegistry::default();

        let classification = registry.classify_invocation(&invocation("ldconfig", &[]));

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("ldconfig should be known");
        };
        assert_eq!(reason_code, "known-helper-requires-adapter-coverage");
        assert_eq!(effects[0].adapter_id.as_deref(), Some("ldconfig/v1"));
        assert_ne!(
            effects[0].replacement,
            crate::ccs::legacy_scriptlets::EffectReplacement::Complete
        );
    }

    #[test]
    fn adapter_registry_lets_blocked_class_win_before_adapter_matching() {
        let registry = AdapterRegistry::default();

        let classification =
            registry.classify_invocation(&invocation("curl", &["https://example.invalid"]));

        assert!(matches!(
            classification,
            ScriptletClassification::Blocked { reason_code, class_id }
                if reason_code == "blocked-class-network" && class_id == "network"
        ));
    }

    #[test]
    fn adapter_registry_reports_unknown_commands() {
        let registry = AdapterRegistry::default();

        let classification =
            registry.classify_invocation(&invocation("custom-helper", &["--do-it"]));

        assert!(matches!(
            classification,
            ScriptletClassification::Unknown { reason_code, command }
                if reason_code == "unknown-command" && command == "custom-helper"
        ));
    }

    #[test]
    fn adapter_registry_has_stable_builtin_order_and_unique_ids() {
        let registry = AdapterRegistry::default();
        let ids = registry.adapter_ids();

        assert_eq!(
            ids,
            vec![
                "native-free/v1",
                "ldconfig/v1",
                "systemd-daemon-reload/v1",
                "systemd-enable-disable/v1",
            ]
        );

        let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len());

        let native_free = registry
            .adapters_for_testing()
            .into_iter()
            .find(|adapter| adapter.id() == "native-free/v1")
            .expect("native-free adapter present");
        assert!(!native_free.matches(&invocation("true", &[])));
    }
}
