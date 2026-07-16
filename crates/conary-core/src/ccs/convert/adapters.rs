// conary-core/src/ccs/convert/adapters.rs

use crate::ccs::convert::apparmor_adapters::ApparmorPolicyAdapter;
use crate::ccs::convert::blocked_classes::{BlockedClassOutcome, BlockedClassRegistry};
use crate::ccs::convert::command_evidence::{CommandEvidenceSource, CommandInvocation};
use crate::ccs::convert::debian_adapters::DebSystemdHelperAdapter;
use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletCommandEvidence, ScriptletEffectEvidence,
};
use crate::ccs::convert::payload_hints::PayloadHints;
use crate::ccs::convert::selinux_adapters::SelinuxPolicyAdapter;
use crate::ccs::hooks::{validate_sysctl_key, validate_sysctl_value};
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use crate::ccs::manifest::is_supported_linux_file_capability;
use std::collections::{BTreeMap, BTreeSet};

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
            command: "deb-systemd-helper",
            forms: &[
                "deb-systemd-helper enable",
                "deb-systemd-helper disable",
                "deb-systemd-helper purge|mask|unmask|is-enabled|was-enabled|debian-installed|update-state|reenable",
            ],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["deb-systemd-helper/v1"],
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
            command: "sysctl",
            forms: &["sysctl -w <key>=<value>"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["sysctl/v1"],
        },
        BootstrapAdapterEvidence {
            command: "chmod",
            forms: &[
                "chmod u+s <payload-executable>",
                "chmod 4xxx <payload-executable>",
            ],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["setuid-mode/v1"],
        },
        BootstrapAdapterEvidence {
            command: "setcap",
            forms: &["setcap cap_*=+ep <payload-executable>"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["file-capability/v1"],
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
            command: "restorecon",
            forms: &[
                "restorecon -R <payload-path>",
                "semanage fcontext -a|-m|-d",
                "setsebool -P <boolean> on|off",
                "semodule -i <payload-module>",
            ],
            package_count: 1,
            invocation_count: 4,
            coverage_ids: &["selinux-policy/v1"],
        },
        BootstrapAdapterEvidence {
            command: "apparmor_parser",
            forms: &["apparmor_parser -r <payload-profile>"],
            package_count: 1,
            invocation_count: 1,
            coverage_ids: &["apparmor-policy/v1"],
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
            Box::new(DebSystemdHelperAdapter),
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
            .map(|adapter| adapter.classify(input));

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
                command: input.invocation.command.clone(),
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

struct ClassFallback {
    class_id: &'static str,
    outcome: BlockedClassOutcome,
    classification: ScriptletClassification,
}

struct NativeFreeAdapter;
struct LdconfigAdapter;
struct SystemdDaemonReloadAdapter;
struct SystemdUnitStateAdapter;
struct SystemdTmpfilesCreateAdapter;
struct SystemdSysusersAdapter;
struct SysctlAdapter;
struct SetuidModeAdapter;
struct FileCapabilityAdapter;
struct AlternativesRegistrationAdapter;
struct CacheRefreshAdapter;

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

impl ScriptletEffectAdapter for SystemdTmpfilesCreateAdapter {
    fn id(&self) -> &'static str {
        "systemd-tmpfiles-create/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"systemd-tmpfiles-create/v1:tmpfiles:payload-gated")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["systemd-tmpfiles"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        input.invocation.command == "systemd-tmpfiles"
            && tmpfiles_create_configs(&input.invocation.argv, input.payload).is_some()
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let configs = tmpfiles_create_configs(&input.invocation.argv, input.payload)
            .expect("matches() must ensure tmpfiles configs");
        known_effect_classification(
            self,
            input.invocation,
            "tmpfiles",
            EffectReplacement::Complete,
            configs.first().cloned(),
            TMPFILES_CREATE_COMPLETE_REASON,
            configs_extra(configs),
        )
    }
}

impl ScriptletEffectAdapter for SystemdSysusersAdapter {
    fn id(&self) -> &'static str {
        "systemd-sysusers/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"systemd-sysusers/v1:sysusers:payload-gated")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["systemd-sysusers"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        input.invocation.command == "systemd-sysusers"
            && sysusers_configs(&input.invocation.argv, input.payload).is_some()
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let configs = sysusers_configs(&input.invocation.argv, input.payload)
            .expect("matches() must ensure sysusers configs");
        known_effect_classification(
            self,
            input.invocation,
            "sysusers",
            EffectReplacement::Complete,
            configs.first().cloned(),
            SYSUSERS_COMPLETE_REASON,
            configs_extra(configs),
        )
    }
}

impl ScriptletEffectAdapter for SysctlAdapter {
    fn id(&self) -> &'static str {
        "sysctl/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"sysctl/v1:sysctl-setting:write")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["sysctl"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        input.invocation.command == "sysctl" && parse_sysctl_write(&input.invocation.argv).is_some()
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let setting =
            parse_sysctl_write(&input.invocation.argv).expect("matches() must ensure sysctl write");
        known_effect_classification(
            self,
            input.invocation,
            "sysctl-setting",
            EffectReplacement::Complete,
            Some(setting.key.clone()),
            SYSCTL_COMPLETE_REASON,
            BTreeMap::from([
                ("key".to_string(), toml::Value::String(setting.key)),
                ("value".to_string(), toml::Value::String(setting.value)),
                ("only_if_lower".to_string(), toml::Value::Boolean(false)),
            ]),
        )
    }
}

impl ScriptletEffectAdapter for SetuidModeAdapter {
    fn id(&self) -> &'static str {
        "setuid-mode/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"setuid-mode/v1:payload-executable:setuid-only")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["chmod"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        input.invocation.command == "chmod"
            && parse_setuid_mode_change(&input.invocation.argv, input.payload).is_some()
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let change = parse_setuid_mode_change(&input.invocation.argv, input.payload)
            .expect("matches() must ensure setuid mode change");
        known_effect_classification(
            self,
            input.invocation,
            "setuid-mode",
            EffectReplacement::Complete,
            Some(change.path.clone()),
            SETUID_COMPLETE_REASON,
            BTreeMap::from([
                (
                    "target_mode".to_string(),
                    toml::Value::Integer(i64::from(change.target_mode)),
                ),
                (
                    "target_mode_octal".to_string(),
                    toml::Value::String(format!("{:04o}", change.target_mode)),
                ),
            ]),
        )
    }
}

impl ScriptletEffectAdapter for FileCapabilityAdapter {
    fn id(&self) -> &'static str {
        "file-capability/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"file-capability/v1:payload-executable:+ep")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["setcap"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        input.invocation.command == "setcap"
            && parse_file_capability_change(&input.invocation.argv, input.payload).is_some()
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let change = parse_file_capability_change(&input.invocation.argv, input.payload)
            .expect("matches() must ensure file capability change");
        known_effect_classification(
            self,
            input.invocation,
            "file-capability",
            EffectReplacement::Complete,
            Some(change.path),
            FILE_CAPABILITY_COMPLETE_REASON,
            BTreeMap::from([
                (
                    "capabilities".to_string(),
                    toml::Value::Array(
                        change
                            .capabilities
                            .into_iter()
                            .map(toml::Value::String)
                            .collect(),
                    ),
                ),
                (
                    "permitted".to_string(),
                    toml::Value::Boolean(change.permitted),
                ),
                (
                    "effective".to_string(),
                    toml::Value::Boolean(change.effective),
                ),
                (
                    "inheritable".to_string(),
                    toml::Value::Boolean(change.inheritable),
                ),
            ]),
        )
    }
}

impl ScriptletEffectAdapter for AlternativesRegistrationAdapter {
    fn id(&self) -> &'static str {
        "alternatives-registration/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(
            b"alternatives-registration/v1:alternatives:registration-remove",
        )
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["update-alternatives", "alternatives"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        is_alternatives_command(&input.invocation.command)
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        match parse_alternatives_registration(&input.invocation.argv) {
            Some(registration) => {
                let path = registration.effect_path();
                known_effect_classification(
                    self,
                    input.invocation,
                    "alternatives",
                    EffectReplacement::Complete,
                    Some(path),
                    ALTERNATIVES_COMPLETE_REASON,
                    alternatives_extra(registration),
                )
            }
            None => review_classification(
                ALTERNATIVES_REVIEW_REASON,
                "alternatives-interactive-or-broad",
            ),
        }
    }
}

impl ScriptletEffectAdapter for CacheRefreshAdapter {
    fn id(&self) -> &'static str {
        "cache-refresh/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"cache-refresh/v1:cache-refresh:payload-gated")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &[
            "update-mime-database",
            "update-desktop-database",
            "gtk-update-icon-cache",
            "glib-compile-schemas",
            "fc-cache",
        ]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        is_cache_refresh_command(&input.invocation.command)
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let Some(refresh) = parse_cache_refresh(input.invocation, input.payload) else {
            return review_classification(CACHE_REFRESH_REVIEW_REASON, "cache-refresh-nonstandard");
        };

        let replacement = cache_refresh_replacement(&refresh, input.payload);
        let reason_code = if replacement == EffectReplacement::Complete {
            CACHE_REFRESH_COMPLETE_REASON
        } else {
            PARTIAL_COVERAGE_REASON
        };

        known_effect_classification(
            self,
            input.invocation,
            "cache-refresh",
            replacement,
            Some(refresh.root),
            reason_code,
            BTreeMap::from([(
                "cache_kind".to_string(),
                toml::Value::String(refresh.kind.to_string()),
            )]),
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

fn tmpfiles_create_configs(argv: &[String], payload: &PayloadHints) -> Option<Vec<String>> {
    let mut saw_create = false;
    let mut configs = Vec::new();

    for arg in argv {
        match arg.as_str() {
            "--create" => {
                if saw_create {
                    return None;
                }
                saw_create = true;
            }
            path if path.ends_with(".conf") && !path.starts_with('-') => {
                configs.push(path.to_string());
            }
            _ => return None,
        }
    }

    if !saw_create {
        return None;
    }
    payload_gated_configs(configs, &payload.tmpfiles_configs)
}

fn sysusers_configs(argv: &[String], payload: &PayloadHints) -> Option<Vec<String>> {
    let mut configs = Vec::new();

    for arg in argv {
        match arg.as_str() {
            "-" => return None,
            path if path.ends_with(".conf") && !path.starts_with('-') => {
                configs.push(path.to_string());
            }
            _ if arg.starts_with('-') => return None,
            _ => return None,
        }
    }

    payload_gated_configs(configs, &payload.sysusers_configs)
}

#[derive(Debug, Clone)]
struct SysctlSetting {
    key: String,
    value: String,
}

#[derive(Debug, Clone)]
struct SetuidModeChange {
    path: String,
    target_mode: u32,
}

#[derive(Debug, Clone)]
struct FileCapabilityChange {
    path: String,
    capabilities: Vec<String>,
    permitted: bool,
    effective: bool,
    inheritable: bool,
}

fn parse_sysctl_write(argv: &[String]) -> Option<SysctlSetting> {
    match argv {
        [flag, assignment] if matches!(flag.as_str(), "-w" | "--write") => {
            parse_sysctl_assignment(assignment)
        }
        _ => None,
    }
}

fn parse_sysctl_assignment(assignment: &str) -> Option<SysctlSetting> {
    let (key, value) = assignment.split_once('=')?;
    validate_sysctl_key(key).ok()?;
    validate_sysctl_value(value).ok()?;
    Some(SysctlSetting {
        key: key.to_string(),
        value: value.to_string(),
    })
}

fn parse_setuid_mode_change(argv: &[String], payload: &PayloadHints) -> Option<SetuidModeChange> {
    let [mode_arg, path] = argv else {
        return None;
    };
    if !path.starts_with('/') || !payload.executable_paths.contains(path) {
        return None;
    }

    let current_mode = payload.file_modes.get(path).copied()? & 0o7777;
    let target_mode = match mode_arg.as_str() {
        "u+s" => current_mode | 0o4000,
        mode => parse_setuid_numeric_mode(mode)?,
    };
    if target_mode & 0o4000 == 0 || target_mode & 0o2000 != 0 {
        return None;
    }

    Some(SetuidModeChange {
        path: path.to_string(),
        target_mode,
    })
}

fn parse_setuid_numeric_mode(mode: &str) -> Option<u32> {
    if mode.len() != 4 || !mode.starts_with('4') || !mode.chars().all(|ch| matches!(ch, '0'..='7'))
    {
        return None;
    }
    let parsed = u32::from_str_radix(mode, 8).ok()?;
    (parsed & 0o7000 == 0o4000 && parsed & 0o111 != 0).then_some(parsed)
}

fn parse_file_capability_change(
    argv: &[String],
    payload: &PayloadHints,
) -> Option<FileCapabilityChange> {
    let [spec, path] = argv else {
        return None;
    };
    if !path.starts_with('/') || !payload.executable_paths.contains(path) {
        return None;
    }
    let capabilities = parse_setcap_ep_spec(spec)?;
    Some(FileCapabilityChange {
        path: path.to_string(),
        capabilities,
        permitted: true,
        effective: true,
        inheritable: false,
    })
}

fn parse_setcap_ep_spec(spec: &str) -> Option<Vec<String>> {
    let (capabilities, flags) = spec.split_once('=')?;
    if flags != "+ep" {
        return None;
    }

    let mut parsed = capabilities
        .split(',')
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parsed.is_empty()
        || parsed
            .iter()
            .any(|capability| !is_supported_linux_file_capability(capability))
    {
        return None;
    }
    parsed.sort();
    parsed.dedup();
    Some(parsed)
}

fn payload_gated_configs(
    explicit_configs: Vec<String>,
    packaged_configs: &BTreeSet<String>,
) -> Option<Vec<String>> {
    if explicit_configs.is_empty() {
        return (!packaged_configs.is_empty()).then(|| packaged_configs.iter().cloned().collect());
    }

    explicit_configs
        .iter()
        .all(|config| packaged_configs.contains(config))
        .then_some(explicit_configs)
}

fn configs_extra(configs: Vec<String>) -> BTreeMap<String, toml::Value> {
    BTreeMap::from([(
        "configs".to_string(),
        toml::Value::Array(configs.into_iter().map(toml::Value::String).collect()),
    )])
}

#[derive(Debug, Clone)]
struct AlternativesRegistration {
    action: &'static str,
    link: Option<String>,
    name: String,
    target: String,
    priority: Option<i32>,
    slaves: Vec<String>,
}

impl AlternativesRegistration {
    fn effect_path(&self) -> String {
        self.link.clone().unwrap_or_else(|| self.target.clone())
    }
}

#[derive(Debug, Clone)]
struct CacheRefresh {
    kind: &'static str,
    root: String,
    roots: Vec<String>,
}

fn is_alternatives_command(command: &str) -> bool {
    matches!(command, "update-alternatives" | "alternatives")
}

fn parse_alternatives_registration(argv: &[String]) -> Option<AlternativesRegistration> {
    match argv.first().map(String::as_str) {
        Some("--install") => parse_alternatives_install(argv),
        Some("--remove") => parse_alternatives_remove(argv),
        _ => None,
    }
}

fn parse_alternatives_install(argv: &[String]) -> Option<AlternativesRegistration> {
    if argv.len() < 5 {
        return None;
    }
    let priority = argv.get(4)?.parse::<i32>().ok()?;
    let mut index = 5;
    let mut slaves = Vec::new();
    while index < argv.len() {
        if argv.get(index).map(String::as_str) != Some("--slave") || index + 3 >= argv.len() {
            return None;
        }
        let slave_link = argv[index + 1].clone();
        let slave_name = argv[index + 2].clone();
        let slave_path = argv[index + 3].clone();
        slaves.push(format!("{slave_link} {slave_name} {slave_path}"));
        index += 4;
    }

    Some(AlternativesRegistration {
        action: "install",
        link: Some(argv[1].clone()),
        name: argv[2].clone(),
        target: argv[3].clone(),
        priority: Some(priority),
        slaves,
    })
}

fn parse_alternatives_remove(argv: &[String]) -> Option<AlternativesRegistration> {
    if argv.len() != 3 {
        return None;
    }
    Some(AlternativesRegistration {
        action: "remove",
        link: None,
        name: argv[1].clone(),
        target: argv[2].clone(),
        priority: None,
        slaves: Vec::new(),
    })
}

fn alternatives_extra(registration: AlternativesRegistration) -> BTreeMap<String, toml::Value> {
    let mut extra = BTreeMap::from([
        (
            "action".to_string(),
            toml::Value::String(registration.action.to_string()),
        ),
        ("name".to_string(), toml::Value::String(registration.name)),
        (
            "target".to_string(),
            toml::Value::String(registration.target),
        ),
        (
            "slaves".to_string(),
            toml::Value::Array(
                registration
                    .slaves
                    .into_iter()
                    .map(toml::Value::String)
                    .collect(),
            ),
        ),
    ]);
    if let Some(priority) = registration.priority {
        extra.insert(
            "priority".to_string(),
            toml::Value::Integer(i64::from(priority)),
        );
    }
    extra
}

fn is_cache_refresh_command(command: &str) -> bool {
    matches!(
        command,
        "update-mime-database"
            | "update-desktop-database"
            | "gtk-update-icon-cache"
            | "glib-compile-schemas"
            | "fc-cache"
    )
}

fn parse_cache_refresh(
    invocation: &CommandInvocation,
    _payload: &PayloadHints,
) -> Option<CacheRefresh> {
    match invocation.command.as_str() {
        "update-mime-database" => {
            parse_exact_cache_root(&invocation.argv, "mime-db", "/usr/share/mime", &[])
        }
        "update-desktop-database" => parse_exact_cache_root(
            &invocation.argv,
            "desktop-db",
            "/usr/share/applications",
            &["-q", "--quiet"],
        ),
        "gtk-update-icon-cache" => parse_icon_cache_refresh(&invocation.argv),
        "glib-compile-schemas" => parse_glib_schema_refresh(&invocation.argv),
        "fc-cache" => parse_font_cache_refresh(&invocation.argv),
        _ => None,
    }
}

fn parse_exact_cache_root(
    argv: &[String],
    kind: &'static str,
    root: &str,
    allowed_flags: &[&str],
) -> Option<CacheRefresh> {
    let paths: Vec<&str> = argv
        .iter()
        .map(String::as_str)
        .filter(|arg| !allowed_flags.contains(arg))
        .collect();
    if paths.len() == 1 && paths[0] == root {
        return Some(cache_refresh(kind, root, vec![root.to_string()]));
    }
    None
}

fn parse_icon_cache_refresh(argv: &[String]) -> Option<CacheRefresh> {
    let mut roots = Vec::new();
    for arg in argv {
        if is_icon_cache_flag(arg) {
            continue;
        }
        if arg.starts_with("/usr/share/icons/") && arg.len() > "/usr/share/icons/".len() {
            roots.push(arg.clone());
        } else {
            return None;
        }
    }
    if roots.len() == 1 {
        let root = roots[0].clone();
        Some(cache_refresh("icon-cache", &root, roots))
    } else {
        None
    }
}

fn is_icon_cache_flag(arg: &str) -> bool {
    matches!(
        arg,
        "-f" | "--force" | "-q" | "--quiet" | "--ignore-theme-index"
    ) || short_flag_chars_are(arg, &['f', 'q'])
}

fn parse_glib_schema_refresh(argv: &[String]) -> Option<CacheRefresh> {
    let paths: Vec<&str> = argv
        .iter()
        .map(String::as_str)
        .filter(|arg| *arg != "--allow-any-name")
        .collect();
    match paths.as_slice() {
        [] => Some(cache_refresh(
            "gsettings",
            "/usr/share/glib-2.0/schemas",
            vec!["/usr/share/glib-2.0/schemas".to_string()],
        )),
        [path] if *path == "/usr/share/glib-2.0/schemas" => Some(cache_refresh(
            "gsettings",
            "/usr/share/glib-2.0/schemas",
            vec!["/usr/share/glib-2.0/schemas".to_string()],
        )),
        _ => None,
    }
}

fn parse_font_cache_refresh(argv: &[String]) -> Option<CacheRefresh> {
    let mut roots = Vec::new();
    for arg in argv {
        if is_font_cache_flag(arg) {
            continue;
        }
        if is_standard_font_dir(arg) {
            roots.push(arg.clone());
        } else {
            return None;
        }
    }
    if roots.is_empty() {
        roots.push("/usr/share/fonts".to_string());
    }
    let root = roots[0].clone();
    Some(cache_refresh("font-cache", &root, roots))
}

fn is_font_cache_flag(arg: &str) -> bool {
    matches!(
        arg,
        "-s" | "--system-only" | "-f" | "--force" | "-r" | "--really-force" | "-v" | "--verbose"
    ) || short_flag_chars_are(arg, &['s', 'f', 'r', 'v'])
}

fn short_flag_chars_are(arg: &str, allowed: &[char]) -> bool {
    arg.starts_with('-')
        && !arg.starts_with("--")
        && arg.len() > 2
        && arg[1..].chars().all(|flag| allowed.contains(&flag))
}

fn is_standard_font_dir(path: &str) -> bool {
    path_is_under(path, "/usr/share/fonts") || path_is_under(path, "/usr/share/texmf/fonts")
}

fn cache_refresh(kind: &'static str, root: &str, roots: Vec<String>) -> CacheRefresh {
    CacheRefresh {
        kind,
        root: root.to_string(),
        roots,
    }
}

fn cache_refresh_replacement(refresh: &CacheRefresh, payload: &PayloadHints) -> EffectReplacement {
    let complete = refresh
        .roots
        .iter()
        .all(|root| payload_has_cache_input_under(payload, refresh.kind, root));
    if complete {
        EffectReplacement::Complete
    } else {
        EffectReplacement::Partial
    }
}

fn payload_has_cache_input_under(payload: &PayloadHints, kind: &str, root: &str) -> bool {
    payload
        .cache_inputs
        .get(kind)
        .is_some_and(|paths| paths.iter().any(|path| path_is_under(path, root)))
}

fn path_is_under(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    path == root || path.starts_with(&format!("{root}/"))
}

fn review_classification(reason_code: &str, class_id: &str) -> ScriptletClassification {
    ScriptletClassification::Review {
        reason_code: reason_code.to_string(),
        class_id: Some(class_id.to_string()),
        command: None,
    }
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
    use crate::packages::traits::ExtractedFile;

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

    fn extra_str<'a>(effect: &'a ScriptletEffectEvidence, key: &str) -> Option<&'a str> {
        effect.extra.get(key).and_then(toml::Value::as_str)
    }

    fn extra_bool(effect: &ScriptletEffectEvidence, key: &str) -> Option<bool> {
        effect.extra.get(key).and_then(toml::Value::as_bool)
    }

    fn extra_string_array(effect: &ScriptletEffectEvidence, key: &str) -> Vec<String> {
        effect
            .extra
            .get(key)
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(toml::Value::as_str)
            .map(str::to_string)
            .collect()
    }

    fn file(path: &str) -> ExtractedFile {
        ExtractedFile {
            path: path.to_string(),
            content: Vec::new(),
            size: 0,
            mode: 0o644,
            sha256: None,
            symlink_target: None,
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
    fn blocked_boot_security_classes_carry_command_evidence() {
        let registry = AdapterRegistry::default();

        for (command, args, class_id, expected_argv) in [
            ("depmod", vec!["6.10.0"], "kernel-module", vec!["<kver>"]),
            (
                "kernel-install",
                vec!["add", "6.10.0", "/lib/modules/6.10.0/vmlinuz"],
                "kernel-module",
                vec!["add", "<kver>", "/lib/modules/<kver>/vmlinuz"],
            ),
            (
                "dracut",
                vec!["--force", "/boot/6.10.0/initramfs.img"],
                "initramfs",
                vec!["--force", "<boot>/<kver>/initramfs.img"],
            ),
            (
                "restorecon",
                vec!["-R", "/usr/lib/modules"],
                "selinux",
                vec!["-R", "/usr/lib/modules"],
            ),
        ] {
            let classification = registry.classify_invocation(&invocation(command, &args));
            match classification {
                ScriptletClassification::Blocked {
                    class_id: actual_class,
                    command: Some(evidence),
                    ..
                } => {
                    assert_eq!(actual_class, class_id);
                    assert_eq!(evidence.command, command);
                    assert_eq!(evidence.argv, expected_argv);
                    assert_eq!(evidence.source, "static-signal");
                    assert!(evidence.environment.is_empty());
                }
                other => panic!("expected blocked evidence for {command}, got {other:?}"),
            }
        }
    }

    #[test]
    fn adapter_registry_lets_blocked_class_win_before_adapter_matching() {
        let registry = AdapterRegistry::default();

        let classification =
            registry.classify_invocation(&invocation("curl", &["https://example.invalid"]));

        assert!(matches!(
            classification,
            ScriptletClassification::Blocked {
                reason_code,
                class_id,
                ..
            }
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
                "ldconfig/v2",
                "systemd-daemon-reload/v2",
                "systemd-unit-state/v1",
                "deb-systemd-helper/v1",
                "systemd-tmpfiles-create/v1",
                "systemd-sysusers/v1",
                "sysctl/v1",
                "setuid-mode/v1",
                "file-capability/v1",
                "selinux-policy/v1",
                "apparmor-policy/v1",
                "alternatives-registration/v1",
                "cache-refresh/v1",
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
            "sysctl",
            "chmod",
            "setcap",
            "update-alternatives",
            "deb-systemd-helper",
            "update-mime-database",
            "restorecon",
            "apparmor_parser",
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
    fn adapter_registry_golden_helpers_are_fully_replaced_with_adapter_evidence() {
        let registry = AdapterRegistry::default();
        let payload = golden_adapter_payload();
        let cases = [
            GoldenAdapterCase {
                fixture_id: "adapter-sysusers",
                command: "systemd-sysusers",
                argv: &["/usr/lib/sysusers.d/demo.conf"],
                adapter_id: "systemd-sysusers/v1",
                reason_code: "helper-complete-sysusers",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-sysctl",
                command: "sysctl",
                argv: &["-w", "kernel.example=1"],
                adapter_id: "sysctl/v1",
                reason_code: "helper-complete-sysctl",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-sysctl-target-profile-private-review",
                command: "sysctl",
                argv: &["-w", "net.ipv4.ip_forward=1"],
                adapter_id: "sysctl/v1",
                reason_code: "helper-complete-sysctl",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-setuid-mode",
                command: "chmod",
                argv: &["u+s", "/usr/bin/demo"],
                adapter_id: "setuid-mode/v1",
                reason_code: "helper-complete-setuid-mode",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-file-capability",
                command: "setcap",
                argv: &["cap_net_bind_service=+ep", "/usr/bin/demo"],
                adapter_id: "file-capability/v1",
                reason_code: "helper-complete-file-capability",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-file-capability-high-risk",
                command: "setcap",
                argv: &["cap_sys_admin=+ep", "/usr/bin/demo"],
                adapter_id: "file-capability/v1",
                reason_code: "helper-complete-file-capability",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-registry-systemd-daemon-reload",
                command: "systemctl",
                argv: &["daemon-reload"],
                adapter_id: "systemd-daemon-reload/v2",
                reason_code: "helper-complete-systemd-daemon-reload",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-registry-systemd-unit-state",
                command: "systemctl",
                argv: &["enable", "demo.service"],
                adapter_id: "systemd-unit-state/v1",
                reason_code: "helper-complete-systemd-unit-state",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-deb-systemd-helper-unit-state",
                command: "deb-systemd-helper",
                argv: &["enable", "demo.service"],
                adapter_id: "deb-systemd-helper/v1",
                reason_code: "helper-complete-deb-systemd-helper-unit-state",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-tmpfiles-create",
                command: "systemd-tmpfiles",
                argv: &["--create", "/usr/lib/tmpfiles.d/demo.conf"],
                adapter_id: "systemd-tmpfiles-create/v1",
                reason_code: "helper-complete-tmpfiles-create",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-cache-refresh",
                command: "update-mime-database",
                argv: &["/usr/share/mime"],
                adapter_id: "cache-refresh/v1",
                reason_code: "helper-complete-cache-refresh",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-selinux-policy",
                command: "restorecon",
                argv: &["-R", "/usr/bin/demo"],
                adapter_id: "selinux-policy/v1",
                reason_code: "helper-complete-selinux-policy",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-apparmor-policy",
                command: "apparmor_parser",
                argv: &["-r", "/etc/apparmor.d/usr.bin.demo"],
                adapter_id: "apparmor-policy/v1",
                reason_code: "helper-complete-apparmor-policy",
            },
            GoldenAdapterCase {
                fixture_id: "adapter-alternatives-registration",
                command: "update-alternatives",
                argv: &[
                    "--install",
                    "/usr/bin/editor",
                    "editor",
                    "/usr/bin/demo-editor",
                    "50",
                ],
                adapter_id: "alternatives-registration/v1",
                reason_code: "helper-complete-alternatives-registration",
            },
        ];

        for case in cases {
            let invocation = invocation(case.command, case.argv);
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation,
                payload: &payload,
            });

            assert_complete_adapter_evidence(
                case.fixture_id,
                classification,
                case.adapter_id,
                case.reason_code,
            );
        }
    }

    #[test]
    fn selinux_adapter_models_payload_backed_policy_and_label_intent_as_portable_effects() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::from_files(&[
            file("/usr/bin/demo"),
            file("/usr/share/selinux/packages/demo.pp"),
        ]);

        for (command, argv, kind, path, operation) in [
            (
                "restorecon",
                vec!["-R", "/usr/bin/demo"],
                "selinux-label-refresh",
                "/usr/bin/demo",
                "label-refresh",
            ),
            (
                "semanage",
                vec!["fcontext", "-a", "-t", "demo_exec_t", "/usr/bin/demo"],
                "selinux-file-context",
                "/usr/bin/demo",
                "file-context-add",
            ),
            (
                "setsebool",
                vec!["-P", "demo_can_network", "on"],
                "selinux-boolean",
                "demo_can_network",
                "boolean-set",
            ),
            (
                "semodule",
                vec!["-i", "/usr/share/selinux/packages/demo.pp"],
                "selinux-policy-module",
                "/usr/share/selinux/packages/demo.pp",
                "module-install",
            ),
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation(command, &argv),
                payload: &payload,
            });

            let ScriptletClassification::Known {
                reason_code,
                effects,
            } = classification
            else {
                panic!("{command} should be modeled as SELinux policy intent");
            };

            assert_eq!(reason_code, "helper-complete-selinux-policy");
            assert_eq!(effects.len(), 1);
            let effect = &effects[0];
            assert_eq!(effect.kind, kind);
            assert_eq!(effect.adapter_id.as_deref(), Some("selinux-policy/v1"));
            assert_eq!(effect.replacement, EffectReplacement::Complete);
            assert_eq!(effect.path.as_deref(), Some(path));
            assert_eq!(extra_str(effect, "selinux_operation"), Some(operation));
            assert_eq!(
                extra_str(effect, "target_security_policy"),
                Some("selinux-optional")
            );
            assert_eq!(
                extra_str(effect, "host_policy_behavior"),
                Some("apply-when-selinux-present-dormant-when-absent")
            );
        }
    }

    #[test]
    fn selinux_adapter_leaves_broad_or_unbacked_mutation_blocked() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::from_files(&[file("/usr/bin/demo")]);

        for (command, argv) in [
            ("restorecon", vec!["-R", "/"]),
            ("restorecon", vec!["-Rv", "/usr"]),
            ("semodule", vec!["-i", "/tmp/demo.pp"]),
            ("semodule", vec!["-r", "demo"]),
            ("semanage", vec!["permissive", "-a", "demo_t"]),
            ("setsebool", vec!["demo_can_network", "on"]),
            ("fixfiles", vec!["restore"]),
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation(command, &argv),
                payload: &payload,
            });

            assert!(matches!(
                classification,
                ScriptletClassification::Blocked {
                    reason_code,
                    class_id,
                    command: Some(_),
                } if reason_code == "blocked-class-selinux" && class_id == "selinux"
            ));
        }
    }

    #[test]
    fn apparmor_adapter_models_payload_backed_profile_reload_as_portable_effect() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::from_files(&[file("/etc/apparmor.d/usr.bin.demo")]);

        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("apparmor_parser", &["-r", "/etc/apparmor.d/usr.bin.demo"]),
            payload: &payload,
        });

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("payload-backed AppArmor profile reload should be modeled as policy intent");
        };

        assert_eq!(reason_code, "helper-complete-apparmor-policy");
        assert_eq!(effects.len(), 1);
        let effect = &effects[0];
        assert_eq!(effect.kind, "apparmor-profile-reload");
        assert_eq!(effect.adapter_id.as_deref(), Some("apparmor-policy/v1"));
        assert_eq!(effect.replacement, EffectReplacement::Complete);
        assert_eq!(effect.path.as_deref(), Some("/etc/apparmor.d/usr.bin.demo"));
        assert_eq!(
            extra_str(effect, "apparmor_operation"),
            Some("profile-reload")
        );
        assert_eq!(
            extra_str(effect, "profile_path"),
            Some("/etc/apparmor.d/usr.bin.demo")
        );
        assert_eq!(extra_str(effect, "profile_name"), Some("usr.bin.demo"));
        assert_eq!(extra_bool(effect, "payload_backed"), Some(true));
        assert_eq!(
            extra_string_array(effect, "paths"),
            vec!["/etc/apparmor.d/usr.bin.demo"]
        );
    }

    #[test]
    fn apparmor_adapter_leaves_broad_or_unbacked_profile_mutation_blocked() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::from_files(&[file("/etc/apparmor.d/usr.bin.demo")]);

        for (command, argv) in [
            ("apparmor_parser", vec!["-r", "/etc/apparmor.d"]),
            ("apparmor_parser", vec!["-r", "/tmp/usr.bin.demo"]),
            (
                "apparmor_parser",
                vec!["-R", "/etc/apparmor.d/usr.bin.demo"],
            ),
            (
                "apparmor_parser",
                vec![
                    "--replace",
                    "/etc/apparmor.d/usr.bin.demo",
                    "/etc/apparmor.d/usr.bin.other",
                ],
            ),
            (
                "apparmor_parser",
                vec!["--replace", "/etc/apparmor.d/subdir/usr.bin.demo"],
            ),
            ("aa-enforce", vec!["/etc/apparmor.d/usr.bin.demo"]),
            ("aa-complain", vec!["/etc/apparmor.d/usr.bin.demo"]),
            ("aa-disable", vec!["/etc/apparmor.d/usr.bin.demo"]),
            ("aa-status", vec![]),
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation(command, &argv),
                payload: &payload,
            });

            assert!(matches!(
                classification,
                ScriptletClassification::Blocked {
                    reason_code,
                    class_id,
                    command: Some(_),
                } if reason_code == "blocked-class-apparmor" && class_id == "apparmor"
            ));
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
            ScriptletClassification::Review {
                reason_code,
                class_id,
                ..
            }
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
            ScriptletClassification::Review {
                reason_code,
                class_id,
                ..
            }
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

    #[test]
    fn deb_systemd_helper_enable_disable_are_complete_with_state_model_for_packaged_units() {
        let registry = AdapterRegistry::default();
        let mut payload = PayloadHints::default();
        payload.systemd_units.insert("demo.service".to_string());

        for (action, state_model) in [
            ("enable", "first-enable-state-file"),
            ("disable", "enablement-state-update"),
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation("deb-systemd-helper", &[action, "demo.service"]),
                payload: &payload,
            });

            let ScriptletClassification::Known {
                reason_code,
                effects,
            } = classification
            else {
                panic!("deb-systemd-helper {action} should be modeled as known evidence");
            };
            assert_eq!(reason_code, "helper-complete-deb-systemd-helper-unit-state");
            assert_eq!(effects.len(), 1);
            let effect = &effects[0];
            assert_eq!(effect.adapter_id.as_deref(), Some("deb-systemd-helper/v1"));
            assert_eq!(effect.kind, "debian-systemd-helper-state");
            assert_eq!(effect.replacement, EffectReplacement::Complete);
            assert_eq!(effect.path.as_deref(), Some("demo.service"));
            assert_eq!(extra_str(effect, "debian_helper_action"), Some(action));
            assert_eq!(extra_str(effect, "state_model"), Some(state_model));
            assert_eq!(extra_bool(effect, "payload_backed"), Some(true));
            assert_eq!(extra_bool(effect, "documented_action"), Some(true));
            assert_eq!(extra_bool(effect, "maintscript_only"), Some(true));
            assert_eq!(extra_bool(effect, "dpkg_root_aware"), Some(true));
            assert_eq!(
                extra_string_array(effect, "units"),
                vec!["demo.service".to_string()]
            );
        }
    }

    #[test]
    fn deb_systemd_helper_documented_review_actions_are_typed_partial_evidence() {
        let registry = AdapterRegistry::default();
        let mut payload = PayloadHints::default();
        payload.systemd_units.insert("demo.service".to_string());

        for (action, state_model) in [
            ("purge", "state-file-purge"),
            ("mask", "mask-state-save"),
            ("unmask", "mask-state-restore"),
            ("is-enabled", "enablement-query"),
            ("was-enabled", "previous-enablement-query"),
            ("debian-installed", "state-file-presence-query"),
            ("update-state", "state-file-reconcile"),
            ("reenable", "reenable-from-recorded-state"),
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation("deb-systemd-helper", &[action, "demo.service"]),
                payload: &payload,
            });

            let ScriptletClassification::Known {
                reason_code,
                effects,
            } = classification
            else {
                panic!("deb-systemd-helper {action} should be modeled as known partial evidence");
            };
            assert_eq!(reason_code, "helper-review-deb-systemd-helper-state");
            assert_eq!(effects.len(), 1);
            let effect = &effects[0];
            assert_eq!(effect.adapter_id.as_deref(), Some("deb-systemd-helper/v1"));
            assert_eq!(effect.kind, "debian-systemd-helper-state");
            assert_eq!(effect.replacement, EffectReplacement::Partial);
            assert_eq!(effect.path.as_deref(), Some("demo.service"));
            assert_eq!(extra_str(effect, "debian_helper_action"), Some(action));
            assert_eq!(extra_str(effect, "state_model"), Some(state_model));
            assert_eq!(extra_bool(effect, "payload_backed"), Some(true));
            assert_eq!(extra_bool(effect, "documented_action"), Some(true));
            assert_eq!(extra_bool(effect, "review_required"), Some(true));
        }
    }

    #[test]
    fn deb_systemd_helper_unbacked_documented_action_is_typed_partial_evidence() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::default();

        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("deb-systemd-helper", &["enable", "demo.service"]),
            payload: &payload,
        });

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("unbacked documented deb-systemd-helper action should be typed evidence");
        };
        assert_eq!(reason_code, "helper-review-deb-systemd-helper-state");
        assert_eq!(effects[0].replacement, EffectReplacement::Partial);
        assert_eq!(extra_bool(&effects[0], "payload_backed"), Some(false));
        assert_eq!(extra_bool(&effects[0], "review_required"), Some(true));
    }

    #[test]
    fn deb_systemd_invoke_and_undocumented_helper_forms_stay_review() {
        let registry = AdapterRegistry::default();
        let empty_payload = PayloadHints::default();
        let invoke = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("deb-systemd-invoke", &["restart", "demo.service"]),
            payload: &empty_payload,
        });
        assert!(matches!(
            invoke,
            ScriptletClassification::Review {
                reason_code,
                class_id,
                command: Some(_),
            }
                if reason_code == "review-class-deb-systemd-helper"
                    && class_id.as_deref() == Some("deb-systemd-helper")
        ));

        let mut payload = PayloadHints::default();
        payload.systemd_units.insert("demo.service".to_string());
        let flagged = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("deb-systemd-helper", &["enable", "--quiet", "demo.service"]),
            payload: &payload,
        });
        assert!(matches!(
            flagged,
            ScriptletClassification::Review {
                reason_code,
                class_id,
                command: Some(_),
            }
                if reason_code == "review-class-deb-systemd-helper"
                    && class_id.as_deref() == Some("deb-systemd-helper")
        ));
    }

    #[test]
    fn tmpfiles_create_is_complete_with_packaged_config() {
        let registry = AdapterRegistry::default();
        let mut payload = PayloadHints::default();
        payload
            .tmpfiles_configs
            .insert("/usr/lib/tmpfiles.d/demo.conf".to_string());

        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(
                "systemd-tmpfiles",
                &["--create", "/usr/lib/tmpfiles.d/demo.conf"],
            ),
            payload: &payload,
        });

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("tmpfiles create should be known");
        };
        assert_eq!(reason_code, "helper-complete-tmpfiles-create");
        assert_eq!(effects[0].replacement, EffectReplacement::Complete);
        assert_eq!(effects[0].kind, "tmpfiles");
    }

    #[test]
    fn tmpfiles_remove_and_boot_are_review() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::default();

        for argv in [
            vec!["--remove"],
            vec!["--boot", "--create"],
            vec!["--create", "--boot"],
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation("systemd-tmpfiles", &argv),
                payload: &payload,
            });
            assert!(matches!(
                classification,
                ScriptletClassification::Review {
                    reason_code,
                    class_id,
                    ..
                }
                    if reason_code == "review-class-tmpfiles-noncreate"
                        && class_id.as_deref() == Some("tmpfiles-noncreate")
            ));
        }
    }

    #[test]
    fn sysusers_is_complete_with_packaged_config() {
        let registry = AdapterRegistry::default();
        let mut payload = PayloadHints::default();
        payload
            .sysusers_configs
            .insert("/usr/lib/sysusers.d/demo.conf".to_string());

        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("systemd-sysusers", &["/usr/lib/sysusers.d/demo.conf"]),
            payload: &payload,
        });

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("sysusers should be known");
        };
        assert_eq!(reason_code, "helper-complete-sysusers");
        assert_eq!(effects[0].replacement, EffectReplacement::Complete);
        assert_eq!(effects[0].kind, "sysusers");
    }

    #[test]
    fn sysusers_replace_and_root_are_review() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::default();

        for argv in [
            vec!["--replace=/usr/lib/sysusers.d/demo.conf"],
            vec!["--root=/tmp/root"],
            vec!["/usr/lib/sysusers.d/demo.conf", "--root=/tmp/root"],
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation("systemd-sysusers", &argv),
                payload: &payload,
            });
            assert!(matches!(
                classification,
                ScriptletClassification::Review {
                    reason_code,
                    class_id,
                    ..
                }
                    if reason_code == "review-class-sysusers-nonstandard"
                        && class_id.as_deref() == Some("sysusers-nonstandard")
            ));
        }
    }

    #[test]
    fn sysctl_adapter_models_safe_write_as_complete_native_intent() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::default();

        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("sysctl", &["-w", "net.ipv4.ip_forward=1"]),
            payload: &payload,
        });

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("safe sysctl write should be modeled as native intent");
        };
        assert_eq!(reason_code, "helper-complete-sysctl");
        assert_eq!(effects.len(), 1);
        let effect = &effects[0];
        assert_eq!(effect.kind, "sysctl-setting");
        assert_eq!(effect.adapter_id.as_deref(), Some("sysctl/v1"));
        assert_eq!(effect.replacement, EffectReplacement::Complete);
        assert_eq!(effect.path.as_deref(), Some("net.ipv4.ip_forward"));
        assert_eq!(extra_str(effect, "key"), Some("net.ipv4.ip_forward"));
        assert_eq!(extra_str(effect, "value"), Some("1"));
    }

    #[test]
    fn sysctl_adapter_leaves_broad_and_denied_forms_blocked() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::default();

        for argv in [
            vec!["-p"],
            vec!["-w", "kernel.modules_disabled=1"],
            vec!["-w", "net.ipv4.ip_forward=1", "vm.swappiness=10"],
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation("sysctl", &argv),
                payload: &payload,
            });
            assert!(matches!(
                classification,
                ScriptletClassification::Blocked {
                    reason_code,
                    class_id,
                    command: Some(_),
                } if reason_code == "blocked-class-sysctl" && class_id == "sysctl"
            ));
        }
    }

    #[test]
    fn setuid_adapter_requires_payload_executable_and_leaves_other_privilege_forms_blocked() {
        let registry = AdapterRegistry::default();
        let mut payload = PayloadHints::default();
        payload.payload_paths.insert("/usr/bin/demo".to_string());
        payload
            .file_modes
            .insert("/usr/bin/demo".to_string(), 0o755);
        payload.executable_paths.insert("/usr/bin/demo".to_string());

        for (command, argv) in [
            ("chmod", vec!["u+s", "/usr/bin/missing"]),
            ("chmod", vec!["g+s", "/usr/bin/demo"]),
            ("chmod", vec!["+s", "/usr/bin/demo"]),
            ("chmod", vec!["6755", "/usr/bin/demo"]),
            ("setpriv", vec!["--no-new-privs", "/usr/bin/demo"]),
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation(command, &argv),
                payload: &payload,
            });
            assert!(matches!(
                classification,
                ScriptletClassification::Blocked {
                    reason_code,
                    class_id,
                    command: Some(_),
                } if reason_code == "blocked-class-setuid-setcap"
                    && class_id == "setuid-setcap"
            ));
        }
    }

    #[test]
    fn file_capability_adapter_models_supported_payload_executable_setcap() {
        let registry = AdapterRegistry::default();
        let mut payload = PayloadHints::default();
        payload.payload_paths.insert("/usr/bin/demo".to_string());
        payload
            .file_modes
            .insert("/usr/bin/demo".to_string(), 0o755);
        payload.executable_paths.insert("/usr/bin/demo".to_string());

        let classification = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("setcap", &["cap_net_bind_service=+ep", "/usr/bin/demo"]),
            payload: &payload,
        });

        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = classification
        else {
            panic!("supported setcap should be modeled as file capability authority");
        };
        assert_eq!(reason_code, "helper-complete-file-capability");
        assert_eq!(effects.len(), 1);
        let effect = &effects[0];
        assert_eq!(effect.kind, "file-capability");
        assert_eq!(effect.adapter_id.as_deref(), Some("file-capability/v1"));
        assert_eq!(effect.replacement, EffectReplacement::Complete);
        assert_eq!(effect.path.as_deref(), Some("/usr/bin/demo"));
        assert_eq!(
            extra_string_array(effect, "capabilities"),
            vec!["cap_net_bind_service"]
        );
        assert_eq!(extra_bool(effect, "permitted"), Some(true));
        assert_eq!(extra_bool(effect, "effective"), Some(true));
        assert_eq!(extra_bool(effect, "inheritable"), Some(false));
    }

    #[test]
    fn file_capability_adapter_keeps_broad_unknown_and_non_payload_setcap_blocked() {
        let registry = AdapterRegistry::default();
        let mut payload = PayloadHints::default();
        payload.payload_paths.insert("/usr/bin/demo".to_string());
        payload
            .file_modes
            .insert("/usr/bin/demo".to_string(), 0o755);
        payload.executable_paths.insert("/usr/bin/demo".to_string());

        for argv in [
            vec!["-r", "/usr/bin/demo"],
            vec!["cap_net_bind_service=+eip", "/usr/bin/demo"],
            vec!["cap_not_real=+ep", "/usr/bin/demo"],
            vec!["cap_net_bind_service=+ep", "/usr/bin/missing"],
            vec!["cap_net_bind_service=+ep", "/etc/demo.conf"],
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation("setcap", &argv),
                payload: &payload,
            });
            assert!(matches!(
                classification,
                ScriptletClassification::Blocked {
                    reason_code,
                    class_id,
                    command: Some(_),
                } if reason_code == "blocked-class-setuid-setcap"
                    && class_id == "setuid-setcap"
            ));
        }
    }

    #[test]
    fn alternatives_install_and_remove_are_complete_when_parseable() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::default();

        let install = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(
                "update-alternatives",
                &[
                    "--install",
                    "/usr/bin/editor",
                    "editor",
                    "/usr/bin/demo-editor",
                    "50",
                    "--slave",
                    "/usr/share/man/man1/editor.1.gz",
                    "editor.1.gz",
                    "/usr/share/man/man1/demo-editor.1.gz",
                    "--slave",
                    "/usr/share/man/man1/view.1.gz",
                    "view.1.gz",
                    "/usr/share/man/man1/demo-view.1.gz",
                ],
            ),
            payload: &payload,
        });
        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = install
        else {
            panic!("alternatives install should be known");
        };
        assert_eq!(reason_code, "helper-complete-alternatives-registration");
        assert_eq!(effects[0].replacement, EffectReplacement::Complete);
        assert_eq!(effects[0].kind, "alternatives");
        assert_eq!(effects[0].path.as_deref(), Some("/usr/bin/editor"));

        let remove = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(
                "alternatives",
                &["--remove", "editor", "/usr/bin/demo-editor"],
            ),
            payload: &payload,
        });
        assert!(matches!(
            remove,
            ScriptletClassification::Known { reason_code, .. }
                if reason_code == "helper-complete-alternatives-registration"
        ));
    }

    #[test]
    fn alternatives_interactive_and_broad_actions_are_review() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::default();

        for argv in [
            vec!["--config", "editor"],
            vec!["--remove-all", "editor"],
            vec!["--remove", "editor"],
        ] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation("update-alternatives", &argv),
                payload: &payload,
            });
            assert!(matches!(
                classification,
                ScriptletClassification::Review {
                    reason_code,
                    class_id,
                    ..
                }
                    if reason_code == "review-class-alternatives-interactive-or-broad"
                        && class_id.as_deref() == Some("alternatives-interactive-or-broad")
            ));
        }
    }

    #[test]
    fn cache_refresh_known_forms_are_complete_with_payload_inputs() {
        let registry = AdapterRegistry::default();
        let mut payload = PayloadHints::default();
        payload
            .cache_inputs
            .entry("mime-db".to_string())
            .or_default()
            .insert("/usr/share/mime/packages/demo.xml".to_string());
        payload
            .cache_inputs
            .entry("desktop-db".to_string())
            .or_default()
            .insert("/usr/share/applications/demo.desktop".to_string());
        payload
            .cache_inputs
            .entry("icon-cache".to_string())
            .or_default()
            .insert("/usr/share/icons/hicolor/16x16/apps/demo.png".to_string());
        payload
            .cache_inputs
            .entry("gsettings".to_string())
            .or_default()
            .insert("/usr/share/glib-2.0/schemas/org.example.demo.gschema.xml".to_string());
        payload
            .cache_inputs
            .entry("font-cache".to_string())
            .or_default()
            .insert("/usr/share/fonts/demo/demo.ttf".to_string());

        let mime = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("update-mime-database", &["/usr/share/mime"]),
            payload: &payload,
        });
        let ScriptletClassification::Known {
            reason_code,
            effects,
        } = mime
        else {
            panic!("mime cache refresh should be known");
        };
        assert_eq!(reason_code, "helper-complete-cache-refresh");
        assert_eq!(effects[0].replacement, EffectReplacement::Complete);
        assert_eq!(effects[0].kind, "cache-refresh");
        assert_eq!(
            effects[0].extra["cache_kind"],
            toml::Value::String("mime-db".to_string())
        );

        let desktop = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(
                "update-desktop-database",
                &["-q", "/usr/share/applications"],
            ),
            payload: &payload,
        });
        assert!(matches!(
            desktop,
            ScriptletClassification::Known { reason_code, .. }
                if reason_code == "helper-complete-cache-refresh"
        ));

        let icons = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(
                "gtk-update-icon-cache",
                &["--force", "--quiet", "/usr/share/icons/hicolor"],
            ),
            payload: &payload,
        });
        assert!(matches!(
            icons,
            ScriptletClassification::Known { reason_code, .. }
                if reason_code == "helper-complete-cache-refresh"
        ));

        let icons_combined_flags = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(
                "gtk-update-icon-cache",
                &["-qf", "/usr/share/icons/hicolor"],
            ),
            payload: &payload,
        });
        assert!(matches!(
            icons_combined_flags,
            ScriptletClassification::Known { reason_code, .. }
                if reason_code == "helper-complete-cache-refresh"
        ));

        let schemas = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation(
                "glib-compile-schemas",
                &["--allow-any-name", "/usr/share/glib-2.0/schemas"],
            ),
            payload: &payload,
        });
        assert!(matches!(
            schemas,
            ScriptletClassification::Known { reason_code, .. }
                if reason_code == "helper-complete-cache-refresh"
        ));

        let schemas_default_path = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("glib-compile-schemas", &[]),
            payload: &payload,
        });
        assert!(matches!(
            schemas_default_path,
            ScriptletClassification::Known { reason_code, .. }
                if reason_code == "helper-complete-cache-refresh"
        ));

        let fonts = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("fc-cache", &["-fs"]),
            payload: &payload,
        });
        assert!(matches!(
            fonts,
            ScriptletClassification::Known { reason_code, .. }
                if reason_code == "helper-complete-cache-refresh"
        ));

        let fonts_with_dir = registry.classify_invocation_with_context(AdapterInput {
            invocation: &invocation("fc-cache", &["-f", "/usr/share/fonts/demo"]),
            payload: &payload,
        });
        assert!(matches!(
            fonts_with_dir,
            ScriptletClassification::Known { reason_code, .. }
                if reason_code == "helper-complete-cache-refresh"
        ));
    }

    #[test]
    fn cache_refresh_nonstandard_paths_are_review() {
        let registry = AdapterRegistry::default();
        let payload = PayloadHints::default();

        for path in ["/opt/vendor/mime", "/usr/local/share/mime"] {
            let classification = registry.classify_invocation_with_context(AdapterInput {
                invocation: &invocation("update-mime-database", &[path]),
                payload: &payload,
            });
            assert!(matches!(
                classification,
                ScriptletClassification::Review {
                    reason_code,
                    class_id,
                    ..
                }
                    if reason_code == "review-class-cache-refresh-nonstandard"
                        && class_id.as_deref() == Some("cache-refresh-nonstandard")
            ));
        }
    }

    struct GoldenAdapterCase {
        fixture_id: &'static str,
        command: &'static str,
        argv: &'static [&'static str],
        adapter_id: &'static str,
        reason_code: &'static str,
    }

    fn golden_adapter_payload() -> PayloadHints {
        let mut payload = PayloadHints::default();
        payload.payload_paths.insert("/usr/bin/demo".to_string());
        payload
            .file_modes
            .insert("/usr/bin/demo".to_string(), 0o755);
        payload.executable_paths.insert("/usr/bin/demo".to_string());
        payload
            .payload_paths
            .insert("/usr/share/selinux/packages/demo.pp".to_string());
        payload
            .payload_paths
            .insert("/etc/apparmor.d/usr.bin.demo".to_string());
        payload.systemd_units.insert("demo.service".to_string());
        payload
            .tmpfiles_configs
            .insert("/usr/lib/tmpfiles.d/demo.conf".to_string());
        payload
            .sysusers_configs
            .insert("/usr/lib/sysusers.d/demo.conf".to_string());
        payload
            .cache_inputs
            .entry("mime-db".to_string())
            .or_default()
            .insert("/usr/share/mime/packages/demo.xml".to_string());
        payload
    }

    fn assert_complete_adapter_evidence(
        fixture_id: &str,
        classification: ScriptletClassification,
        adapter_id: &str,
        reason_code: &str,
    ) {
        let ScriptletClassification::Known {
            reason_code: actual_reason,
            effects,
        } = classification
        else {
            panic!("{fixture_id} should classify as known adapter evidence");
        };

        assert_eq!(actual_reason, reason_code, "{fixture_id} reason code");
        assert_eq!(
            effects[0].adapter_id.as_deref(),
            Some(adapter_id),
            "{fixture_id} adapter id"
        );
        assert_eq!(
            effects[0].replacement,
            EffectReplacement::Complete,
            "{fixture_id} replacement"
        );
    }
}
