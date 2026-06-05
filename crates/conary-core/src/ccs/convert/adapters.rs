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
const TMPFILES_CREATE_COMPLETE_REASON: &str = "helper-complete-tmpfiles-create";
const SYSUSERS_COMPLETE_REASON: &str = "helper-complete-sysusers";
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
            Box::new(SystemdTmpfilesCreateAdapter),
            Box::new(SystemdSysusersAdapter),
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
struct SystemdTmpfilesCreateAdapter;
struct SystemdSysusersAdapter;
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
            ids,
            vec![
                "native-free/v1",
                "ldconfig/v2",
                "systemd-daemon-reload/v2",
                "systemd-unit-state/v1",
                "systemd-tmpfiles-create/v1",
                "systemd-sysusers/v1",
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
                ScriptletClassification::Review { reason_code, class_id }
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
                ScriptletClassification::Review { reason_code, class_id }
                    if reason_code == "review-class-sysusers-nonstandard"
                        && class_id.as_deref() == Some("sysusers-nonstandard")
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
                ScriptletClassification::Review { reason_code, class_id }
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
                ScriptletClassification::Review { reason_code, class_id }
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
