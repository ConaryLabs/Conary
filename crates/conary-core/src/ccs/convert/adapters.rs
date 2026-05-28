// conary-core/src/ccs/convert/adapters.rs

use crate::ccs::convert::blocked_classes::{BlockedClassOutcome, BlockedClassRegistry};
use crate::ccs::convert::command_evidence::{CommandEvidenceSource, CommandInvocation};
use crate::ccs::convert::effects::{ScriptletClassification, ScriptletEffectEvidence};
use crate::ccs::convert::payload_hints::PayloadHints;
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use std::collections::{BTreeMap, BTreeSet};

const PARTIAL_COVERAGE_REASON: &str = "known-helper-partial-coverage";
const LDCONFIG_COMPLETE_REASON: &str = "helper-complete-ldconfig";
const SYSTEMD_DAEMON_RELOAD_COMPLETE_REASON: &str = "helper-complete-systemd-daemon-reload";
const SYSTEMD_UNIT_STATE_COMPLETE_REASON: &str = "helper-complete-systemd-unit-state";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapAdapterEvidence {
    pub command: &'static str,
    pub forms: &'static [&'static str],
    pub package_count: u32,
    pub invocation_count: u32,
    pub coverage_ids: &'static [&'static str],
}

pub fn bootstrap_adapter_evidence() -> &'static [BootstrapAdapterEvidence] {
    &[
        BootstrapAdapterEvidence {
            command: "ldconfig",
            forms: &["ldconfig", "/sbin/ldconfig"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["ldconfig/v2"],
        },
        BootstrapAdapterEvidence {
            command: "systemctl",
            forms: &[
                "systemctl daemon-reload",
                "systemctl enable",
                "systemctl disable",
                "systemctl preset",
            ],
            package_count: 1,
            invocation_count: 3,
            coverage_ids: &["systemd-daemon-reload/v2", "systemd-unit-state/v1"],
        },
        BootstrapAdapterEvidence {
            command: "systemd-tmpfiles",
            forms: &["systemd-tmpfiles --create"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["systemd-tmpfiles-create/v1"],
        },
        BootstrapAdapterEvidence {
            command: "systemd-sysusers",
            forms: &["systemd-sysusers"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["systemd-sysusers/v1"],
        },
        BootstrapAdapterEvidence {
            command: "update-alternatives",
            forms: &[
                "update-alternatives --install",
                "update-alternatives --remove",
            ],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["alternatives-registration/v1"],
        },
        BootstrapAdapterEvidence {
            command: "update-mime-database",
            forms: &["update-mime-database /usr/share/mime"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["cache-refresh/v1"],
        },
        BootstrapAdapterEvidence {
            command: "install-info",
            forms: &["install-info"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["review-class-install-info"],
        },
        BootstrapAdapterEvidence {
            command: "gconftool-2",
            forms: &["gconftool-2 --makefile-install-rule"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["review-class-gconf-schema"],
        },
    ]
}

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
        if let Some(class) = self.blocked_classes.match_invocation(input.invocation) {
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
            .find(|adapter| adapter.matches(input))
            .map_or_else(
                || ScriptletClassification::Unknown {
                    reason_code: "unknown-command".to_string(),
                    command: input.invocation.command.clone(),
                },
                |adapter| adapter.classify(input),
            )
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
                source: EffectSource::NativeMetadata,
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

struct NativeFreeAdapter;
struct LdconfigAdapter;
struct SystemdDaemonReloadAdapter;
struct SystemdUnitStateAdapter;

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

    fn matches(&self, _input: AdapterInput<'_>) -> bool {
        false
    }

    fn classify(&self, _input: AdapterInput<'_>) -> ScriptletClassification {
        unreachable!("native-free is package-level evidence")
    }
}

impl ScriptletEffectAdapter for LdconfigAdapter {
    fn id(&self) -> &'static str {
        "ldconfig/v2"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"ldconfig/v2:dynamic-linker-cache:complete")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["ldconfig"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        input.invocation.command == "ldconfig" && is_simple_ldconfig_form(&input.invocation.argv)
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        known_effect_classification(
            self,
            input.invocation,
            "dynamic-linker-cache",
            EffectReplacement::Complete,
            None,
            LDCONFIG_COMPLETE_REASON,
            BTreeMap::from([(
                "cache".to_string(),
                toml::Value::String("ld.so.cache".to_string()),
            )]),
        )
    }
}

impl ScriptletEffectAdapter for SystemdDaemonReloadAdapter {
    fn id(&self) -> &'static str {
        "systemd-daemon-reload/v2"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"systemd-daemon-reload/v2:systemd-daemon-reload:complete")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["systemctl"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        input.invocation.command == "systemctl"
            && is_systemd_daemon_reload_form(&input.invocation.argv)
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        known_effect_classification(
            self,
            input.invocation,
            "systemd-daemon-reload",
            EffectReplacement::Complete,
            None,
            SYSTEMD_DAEMON_RELOAD_COMPLETE_REASON,
            BTreeMap::new(),
        )
    }
}

impl ScriptletEffectAdapter for SystemdUnitStateAdapter {
    fn id(&self) -> &'static str {
        "systemd-unit-state/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"systemd-unit-state/v1:systemd-unit-state:payload-gated")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["systemctl"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        input.invocation.command == "systemctl"
            && systemd_unit_state_parts(&input.invocation.argv).is_some()
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let invocation = input.invocation;
        let (action, units) = systemd_unit_state_parts(&invocation.argv)
            .expect("matches() must ensure systemd unit state args");
        let kind = format!("systemd-unit-{action}");
        let all_units_are_packaged = units
            .iter()
            .all(|unit| input.payload.systemd_units.contains(*unit));
        let replacement = if all_units_are_packaged {
            EffectReplacement::Complete
        } else {
            EffectReplacement::Partial
        };
        let reason_code = if all_units_are_packaged {
            SYSTEMD_UNIT_STATE_COMPLETE_REASON
        } else {
            PARTIAL_COVERAGE_REASON
        };
        let extra = BTreeMap::from([(
            "units".to_string(),
            toml::Value::Array(
                units
                    .iter()
                    .map(|unit| toml::Value::String((*unit).to_string()))
                    .collect(),
            ),
        )]);

        known_effect_classification(
            self,
            invocation,
            &kind,
            replacement,
            units.first().map(|unit| (*unit).to_string()),
            reason_code,
            extra,
        )
    }
}

fn is_simple_ldconfig_form(argv: &[String]) -> bool {
    argv.is_empty()
        || matches!(
            argv,
            [arg] if matches!(arg.as_str(), "-v" | "--verbose")
        )
}

fn is_systemd_daemon_reload_form(argv: &[String]) -> bool {
    matches!(
        argv,
        [action] if action == "daemon-reload"
    ) || matches!(
        argv,
        [scope, action] if scope == "--system" && action == "daemon-reload"
    )
}

fn systemd_unit_state_parts(argv: &[String]) -> Option<(&str, Vec<&str>)> {
    let action = argv.first()?.as_str();
    if !matches!(action, "enable" | "disable" | "preset") {
        return None;
    }
    if argv.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--now" | "--user" | "--global" | "--runtime" | "preset-all"
        )
    }) {
        return None;
    }

    let units: Vec<&str> = argv
        .iter()
        .skip(1)
        .map(String::as_str)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    if units.is_empty() {
        return None;
    }

    Some((action, units))
}

fn known_effect_classification(
    adapter: &dyn ScriptletEffectAdapter,
    invocation: &CommandInvocation,
    kind: &str,
    replacement: EffectReplacement,
    path: Option<String>,
    reason_code: &str,
    extra: BTreeMap<String, toml::Value>,
) -> ScriptletClassification {
    ScriptletClassification::Known {
        reason_code: reason_code.to_string(),
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
            reason_code: Some(reason_code.to_string()),
            extra,
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
    use crate::ccs::convert::payload_hints::PayloadHints;
    use crate::ccs::legacy_scriptlets::EffectReplacement;

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
    fn adapter_registry_classifies_safe_helpers_with_complete_replacement() {
        let registry = AdapterRegistry::default();

        let classification = registry.classify_invocation(&invocation("ldconfig", &[]));

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("ldconfig should be known");
        };
        assert_eq!(reason_code, "helper-complete-ldconfig");
        assert_eq!(effects[0].adapter_id.as_deref(), Some("ldconfig/v2"));
        assert_eq!(effects[0].replacement, EffectReplacement::Complete);
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
            &ids[..4],
            &[
                "native-free/v1",
                "ldconfig/v2",
                "systemd-daemon-reload/v2",
                "systemd-unit-state/v1",
            ]
        );

        let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len());

        let native_free = registry
            .adapters_for_testing()
            .into_iter()
            .find(|adapter| adapter.id() == "native-free/v1")
            .expect("native-free adapter present");
        let payload = PayloadHints::default();
        let command = invocation("true", &[]);
        assert!(!native_free.matches(AdapterInput {
            invocation: &command,
            payload: &payload,
        }));
    }

    #[test]
    fn bootstrap_adapter_candidates_are_backed_by_corpus_evidence() {
        let evidence = bootstrap_adapter_evidence();

        for command in [
            "ldconfig",
            "systemctl",
            "systemd-tmpfiles",
            "systemd-sysusers",
            "update-alternatives",
            "update-mime-database",
            "install-info",
            "gconftool-2",
        ] {
            assert!(
                evidence.iter().any(|entry| entry.command == command),
                "missing bootstrap corpus evidence for {command}"
            );
        }

        for entry in evidence {
            assert!(entry.package_count > 0);
            assert!(entry.invocation_count >= entry.package_count);
            assert!(!entry.forms.is_empty());
            assert!(!entry.coverage_ids.is_empty());
        }
    }

    #[test]
    fn adapter_registry_uses_payload_context_for_systemd_units() {
        let registry = AdapterRegistry::default();
        let mut payload = PayloadHints::default();
        payload.systemd_units.insert("demo.service".to_string());

        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemctl", &["enable", "demo.service"]),
            payload: &payload,
        });

        let ScriptletClassification::Known { effects, .. } = classification else {
            panic!("systemctl enable should be known through context dispatch");
        };
        assert_eq!(effects[0].command.as_deref(), Some("systemctl"));
        assert_eq!(effects[0].args, vec!["enable", "demo.service"]);
    }

    #[test]
    fn ldconfig_complete_only_for_simple_cache_refresh_forms() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::default();

        let complete = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("ldconfig", &[]),
            payload: &payload,
        });
        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = complete
        else {
            panic!("simple ldconfig should be known");
        };
        assert_eq!(reason_code, "helper-complete-ldconfig");
        assert_eq!(effects[0].replacement, EffectReplacement::Complete);
        assert_eq!(effects[0].kind, "dynamic-linker-cache");

        let review = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("ldconfig", &["-p"]),
            payload: &payload,
        });
        assert!(matches!(
            review,
            ScriptletClassification::Review { reason_code, class_id }
                if reason_code == "review-class-ldconfig-nonstandard"
                    && class_id.as_deref() == Some("ldconfig-nonstandard")
        ));
    }

    #[test]
    fn systemd_daemon_reload_is_complete_but_runtime_actions_are_review() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::default();

        let reload = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemctl", &["daemon-reload"]),
            payload: &payload,
        });
        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = reload
        else {
            panic!("daemon-reload should be known");
        };
        assert_eq!(reason_code, "helper-complete-systemd-daemon-reload");
        assert_eq!(effects[0].replacement, EffectReplacement::Complete);

        let system_scope = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemctl", &["--system", "daemon-reload"]),
            payload: &payload,
        });
        assert!(matches!(
            system_scope,
            ScriptletClassification::Known { reason_code, .. }
                if reason_code == "helper-complete-systemd-daemon-reload"
        ));

        let restart = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemctl", &["restart", "demo.service"]),
            payload: &payload,
        });
        assert!(matches!(
            restart,
            ScriptletClassification::Review { reason_code, class_id }
                if reason_code == "review-class-systemd-runtime-action"
                    && class_id.as_deref() == Some("systemd-runtime-action")
        ));
    }

    #[test]
    fn systemd_unit_state_requires_payload_evidence_for_complete() {
        let registry = AdapterRegistry::default();
        let empty_payload = PayloadHints::default();

        let partial = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemctl", &["enable", "demo.service"]),
            payload: &empty_payload,
        });
        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = partial
        else {
            panic!("systemctl enable should be known");
        };
        assert_eq!(reason_code, "known-helper-partial-coverage");
        assert_eq!(effects[0].replacement, EffectReplacement::Partial);

        let mut payload = PayloadHints::default();
        payload.systemd_units.insert("demo.service".to_string());
        let complete = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemctl", &["preset", "demo.service"]),
            payload: &payload,
        });
        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = complete
        else {
            panic!("systemctl preset should be known");
        };
        assert_eq!(reason_code, "helper-complete-systemd-unit-state");
        assert_eq!(effects[0].replacement, EffectReplacement::Complete);
        assert_eq!(effects[0].path.as_deref(), Some("demo.service"));
    }
}
