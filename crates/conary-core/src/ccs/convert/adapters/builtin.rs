// conary-core/src/ccs/convert/adapters/builtin.rs

use super::*;

pub(super) struct NativeFreeAdapter;
pub(super) struct LdconfigAdapter;
pub(super) struct SystemdDaemonReloadAdapter;
pub(super) struct SystemdUnitStateAdapter;
pub(super) struct SystemdTmpfilesCreateAdapter;
pub(super) struct SystemdSysusersAdapter;
pub(super) struct SysctlAdapter;
pub(super) struct SetuidModeAdapter;
pub(super) struct FileCapabilityAdapter;
pub(super) struct AlternativesRegistrationAdapter;
pub(super) struct CacheRefreshAdapter;

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
