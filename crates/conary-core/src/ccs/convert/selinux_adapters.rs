// conary-core/src/ccs/convert/selinux_adapters.rs

use crate::ccs::convert::adapters::{AdapterInput, ScriptletEffectAdapter};
use crate::ccs::convert::command_evidence::CommandEvidenceSource;
use crate::ccs::convert::effects::{ScriptletClassification, ScriptletEffectEvidence};
use crate::ccs::convert::payload_hints::PayloadHints;
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use std::collections::BTreeMap;

const COMPLETE_REASON: &str = "helper-complete-selinux-policy";
const HOST_POLICY_BEHAVIOR: &str = "apply-when-selinux-present-dormant-when-absent";
const TARGET_SECURITY_POLICY: &str = "selinux-optional";

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelinuxPolicyInvocation {
    LabelRefresh {
        paths: Vec<String>,
        recursive: bool,
    },
    FileContext {
        action: &'static str,
        selinux_type: Option<String>,
        expression: String,
        payload_root: String,
    },
    Boolean {
        name: String,
        value: &'static str,
        persistent: bool,
    },
    ModuleInstall {
        path: String,
        format: &'static str,
    },
}

impl SelinuxPolicyInvocation {
    fn parse(input: AdapterInput<'_>) -> Option<Self> {
        match input.invocation.command.as_str() {
            "restorecon" => parse_restorecon(&input.invocation.argv, input.payload),
            "semanage" => parse_semanage_fcontext(&input.invocation.argv, input.payload),
            "setsebool" => parse_setsebool(&input.invocation.argv),
            "semodule" => parse_semodule_install(&input.invocation.argv, input.payload),
            _ => None,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::LabelRefresh { .. } => "selinux-label-refresh",
            Self::FileContext { .. } => "selinux-file-context",
            Self::Boolean { .. } => "selinux-boolean",
            Self::ModuleInstall { .. } => "selinux-policy-module",
        }
    }

    fn operation(&self) -> &'static str {
        match self {
            Self::LabelRefresh { .. } => "label-refresh",
            Self::FileContext { action, .. } => match *action {
                "add" => "file-context-add",
                "modify" => "file-context-modify",
                "delete" => "file-context-delete",
                _ => "file-context",
            },
            Self::Boolean { .. } => "boolean-set",
            Self::ModuleInstall { .. } => "module-install",
        }
    }

    fn path(&self) -> String {
        match self {
            Self::LabelRefresh { paths, .. } => paths[0].clone(),
            Self::FileContext { payload_root, .. } => payload_root.clone(),
            Self::Boolean { name, .. } => name.clone(),
            Self::ModuleInstall { path, .. } => path.clone(),
        }
    }

    fn extra(&self) -> BTreeMap<String, toml::Value> {
        let mut extra = BTreeMap::from([
            (
                "selinux_operation".to_string(),
                toml::Value::String(self.operation().to_string()),
            ),
            (
                "target_security_policy".to_string(),
                toml::Value::String(TARGET_SECURITY_POLICY.to_string()),
            ),
            (
                "host_policy_behavior".to_string(),
                toml::Value::String(HOST_POLICY_BEHAVIOR.to_string()),
            ),
        ]);

        match self {
            Self::LabelRefresh { paths, recursive } => {
                extra.insert(
                    "paths".to_string(),
                    toml::Value::Array(paths.iter().cloned().map(toml::Value::String).collect()),
                );
                extra.insert("recursive".to_string(), toml::Value::Boolean(*recursive));
                extra.insert("payload_backed".to_string(), toml::Value::Boolean(true));
            }
            Self::FileContext {
                action,
                selinux_type,
                expression,
                payload_root,
            } => {
                extra.insert(
                    "action".to_string(),
                    toml::Value::String((*action).to_string()),
                );
                if let Some(selinux_type) = selinux_type {
                    extra.insert(
                        "selinux_type".to_string(),
                        toml::Value::String(selinux_type.clone()),
                    );
                }
                extra.insert(
                    "expression".to_string(),
                    toml::Value::String(expression.clone()),
                );
                extra.insert(
                    "payload_root".to_string(),
                    toml::Value::String(payload_root.clone()),
                );
                extra.insert("payload_backed".to_string(), toml::Value::Boolean(true));
            }
            Self::Boolean {
                name,
                value,
                persistent,
            } => {
                extra.insert("boolean".to_string(), toml::Value::String(name.clone()));
                extra.insert(
                    "value".to_string(),
                    toml::Value::String((*value).to_string()),
                );
                extra.insert("persistent".to_string(), toml::Value::Boolean(*persistent));
                extra.insert(
                    "apply_when_boolean_exists".to_string(),
                    toml::Value::Boolean(true),
                );
            }
            Self::ModuleInstall { path, format } => {
                extra.insert("module_path".to_string(), toml::Value::String(path.clone()));
                extra.insert(
                    "module_format".to_string(),
                    toml::Value::String((*format).to_string()),
                );
                extra.insert("payload_backed".to_string(), toml::Value::Boolean(true));
            }
        }

        extra
    }
}

pub(super) struct SelinuxPolicyAdapter;

impl ScriptletEffectAdapter for SelinuxPolicyAdapter {
    fn id(&self) -> &'static str {
        "selinux-policy/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(
            b"selinux-policy/v1:optional-policy:payload-backed-label-module-boolean",
        )
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["restorecon", "semanage", "setsebool", "semodule"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        SelinuxPolicyInvocation::parse(input).is_some()
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let policy = SelinuxPolicyInvocation::parse(input)
            .expect("matches() must ensure SELinux policy invocation");

        ScriptletClassification::Known {
            reason_code: COMPLETE_REASON.to_string(),
            effects: vec![ScriptletEffectEvidence {
                kind: policy.kind().to_string(),
                source: effect_source(input.invocation.source),
                confidence: EffectConfidence::Inferred,
                replacement: EffectReplacement::Complete,
                adapter_id: Some(self.id().to_string()),
                adapter_digest: Some(self.digest()),
                command: Some(input.invocation.command.clone()),
                args: input.invocation.argv.clone(),
                path: Some(policy.path()),
                reason_code: Some(COMPLETE_REASON.to_string()),
                extra: policy.extra(),
            }],
        }
    }
}

fn parse_restorecon(argv: &[String], payload: &PayloadHints) -> Option<SelinuxPolicyInvocation> {
    let mut paths = Vec::new();
    let mut recursive = false;

    for arg in argv {
        if is_restorecon_flag(arg) {
            recursive |= arg.contains('R') || arg.contains('r');
            continue;
        }
        if arg.starts_with('-') || !payload_scoped_path(payload, arg) {
            return None;
        }
        paths.push(arg.clone());
    }

    (!paths.is_empty()).then_some(SelinuxPolicyInvocation::LabelRefresh { paths, recursive })
}

fn is_restorecon_flag(arg: &str) -> bool {
    matches!(
        arg,
        "-R" | "-r" | "-v" | "-F" | "-n" | "-D" | "--recursive" | "--verbose" | "--force"
    ) || short_flag_chars_are(arg, &['R', 'r', 'v', 'F', 'n', 'D'])
}

fn parse_semanage_fcontext(
    argv: &[String],
    payload: &PayloadHints,
) -> Option<SelinuxPolicyInvocation> {
    if argv.first().map(String::as_str) != Some("fcontext") {
        return None;
    }

    let mut action = None;
    let mut selinux_type = None;
    let mut expressions = Vec::new();
    let mut index = 1;
    while index < argv.len() {
        match argv[index].as_str() {
            "-a" | "--add" => action = Some("add"),
            "-m" | "--modify" => action = Some("modify"),
            "-d" | "--delete" => action = Some("delete"),
            "-t" | "--type" => {
                index += 1;
                selinux_type = Some(argv.get(index)?.clone());
            }
            "-f" | "--filetype" | "-s" | "--seuser" | "-r" | "--range" => {
                index += 1;
                argv.get(index)?;
            }
            value if value.starts_with('-') => return None,
            value => expressions.push(value.to_string()),
        }
        index += 1;
    }

    let action = action?;
    if matches!(action, "add" | "modify") && selinux_type.is_none() {
        return None;
    }
    if expressions.len() != 1 {
        return None;
    }
    let expression = expressions.pop()?;
    let payload_root = fcontext_payload_root(&expression)?;
    if !payload_scoped_path(payload, &payload_root) {
        return None;
    }

    Some(SelinuxPolicyInvocation::FileContext {
        action,
        selinux_type,
        expression,
        payload_root,
    })
}

fn fcontext_payload_root(expression: &str) -> Option<String> {
    let expression = expression.strip_prefix('^').unwrap_or(expression);
    let expression = expression.strip_suffix('$').unwrap_or(expression);
    let root = expression
        .strip_suffix("(/.*)?")
        .or_else(|| expression.strip_suffix("(/.*)"))
        .unwrap_or(expression);

    if root.starts_with('/') && !root.contains('|') && !root.contains('*') {
        Some(root.to_string())
    } else {
        None
    }
}

fn parse_setsebool(argv: &[String]) -> Option<SelinuxPolicyInvocation> {
    let mut persistent = false;
    let mut positional = Vec::new();
    for arg in argv {
        match arg.as_str() {
            "-P" => persistent = true,
            value if value.starts_with('-') => return None,
            value => positional.push(value),
        }
    }
    if positional.len() != 2 || !persistent {
        return None;
    }

    let name = positional[0];
    if !is_selinux_identifier(name) {
        return None;
    }
    let value = match positional[1] {
        "1" | "on" | "true" => "on",
        "0" | "off" | "false" => "off",
        _ => return None,
    };

    Some(SelinuxPolicyInvocation::Boolean {
        name: name.to_string(),
        value,
        persistent,
    })
}

fn parse_semodule_install(
    argv: &[String],
    payload: &PayloadHints,
) -> Option<SelinuxPolicyInvocation> {
    let mut module_path = None;
    let mut index = 0;
    while index < argv.len() {
        match argv[index].as_str() {
            "-i" | "--install" => {
                index += 1;
                module_path = Some(argv.get(index)?.clone());
            }
            value if value.starts_with('-') => return None,
            _ => return None,
        }
        index += 1;
    }

    let path = module_path?;
    if !payload.payload_paths.contains(&path) {
        return None;
    }
    let format = match path.rsplit('.').next() {
        Some("pp") => "pp",
        Some("cil") => "cil",
        _ => return None,
    };

    Some(SelinuxPolicyInvocation::ModuleInstall { path, format })
}

fn payload_scoped_path(payload: &PayloadHints, path: &str) -> bool {
    is_safe_absolute_path(path)
        && !is_broad_system_root(path)
        && payload
            .payload_paths
            .iter()
            .any(|payload_path| path_is_under(payload_path, path))
}

fn is_safe_absolute_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains("/../")
        && !path.ends_with("/..")
        && !path.contains('\0')
}

fn is_broad_system_root(path: &str) -> bool {
    matches!(
        path.trim_end_matches('/'),
        "" | "/"
            | "/bin"
            | "/boot"
            | "/etc"
            | "/lib"
            | "/lib64"
            | "/opt"
            | "/sbin"
            | "/usr"
            | "/usr/bin"
            | "/usr/lib"
            | "/usr/lib64"
            | "/usr/sbin"
            | "/usr/share"
            | "/var"
    )
}

fn path_is_under(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    path == root || path.starts_with(&format!("{root}/"))
}

fn is_selinux_identifier(value: &str) -> bool {
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn short_flag_chars_are(arg: &str, allowed: &[char]) -> bool {
    arg.starts_with('-')
        && !arg.starts_with("--")
        && arg.len() > 2
        && arg[1..].chars().all(|flag| allowed.contains(&flag))
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
