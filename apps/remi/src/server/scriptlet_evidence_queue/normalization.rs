// apps/remi/src/server/scriptlet_evidence_queue/normalization.rs

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

use conary_core::ccs::legacy_scriptlets::BootSecurityIntentEvidence;
use conary_core::ccs::security_policy::{
    SecurityPolicyFallback, SecurityPolicyIntent, SecurityPolicyProvider,
    SecurityPolicyReconciliationState,
};
use conary_core::db::models::CLUSTER_KEY_PREFIX;
use conary_core::repository::supported_profiles;
use regex::Regex;
use serde_json::{Value, json};

use super::types::{ClusterKeyInput, StableClusterKey};

static ENV_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\$(?:[A-Za-z_][A-Za-z0-9_]*|\{[A-Za-z_][A-Za-z0-9_]*\})$").unwrap()
});
static ENV_ASSIGNMENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=.*$").unwrap());
static EMBEDDED_ENV_ASSIGNMENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(^|[=,:;\s"'])[A-Za-z_][A-Za-z0-9_]*=.*$"#).unwrap());
static KERNEL_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+\.\d+\.\d+[-._A-Za-z0-9]*").unwrap());
static WHITESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static EMBEDDED_ABSOLUTE_PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(^|[=,:;\s"'])/+(?:$|[A-Za-z0-9._@%+-][^\s,;:="']*)"#).unwrap());

const SECURITY_POLICY_INTENT_FIELDS: &[&str] = &[
    "schema",
    "id",
    "source",
    "provider",
    "operation",
    "scope",
    "desired_state",
    "requirements",
    "fallback",
    "payload_evidence",
    "reconciliation",
];
const SECURITY_POLICY_SCOPE_FIELDS: &[&str] = &["kind", "name", "paths", "service", "port"];

pub fn normalize_command_shape(command: &str, argv: &[String]) -> String {
    let mut parts = Vec::with_capacity(argv.len() + 1);
    if let Some(command) = normalize_command_token(command) {
        parts.push(command);
    }
    parts.extend(argv.iter().filter_map(|token| normalize_token(token)));
    collapse_whitespace(&parts.join(" "))
}

pub fn normalize_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if let Some(unwrapped) = matching_quote_wrapper(token)
        && (requires_privacy_redaction(unwrapped) || is_approved_lsm_path(unwrapped))
    {
        return normalize_token(unwrapped);
    }
    if ENV_ASSIGNMENT_RE.is_match(token) {
        if env_assignment_has_quoted_absolute_path(token) {
            return Some("<path>".to_string());
        }
        return Some("<env-assignment>".to_string());
    }
    if ENV_REF_RE.is_match(token) {
        return Some("<env>".to_string());
    }
    if let Some((name, separator, value)) = option_value(token) {
        return Some(format!(
            "{name}{separator}{}",
            normalize_option_value(value)
        ));
    }
    if EMBEDDED_ENV_ASSIGNMENT_RE.is_match(token) {
        return Some("<env-assignment>".to_string());
    }
    let absolute_path_count = absolute_path_count(token);
    if absolute_path_count > 1 || (contains_parent_dir(token) && absolute_path_count > 0) {
        return Some("<path>".to_string());
    }
    if is_approved_lsm_path(token) {
        return Some(token.to_string());
    }

    let had_img_extension = token.ends_with(".img");
    let mut token = KERNEL_VERSION_RE.replace_all(token, "<kver>").to_string();
    if had_img_extension && !token.ends_with(".img") {
        token.push_str(".img");
    }
    if let Some(rest) = token.strip_prefix("/boot/") {
        return Some(format!("<boot>/{rest}"));
    }
    if is_approved_kernel_module_path(&token) {
        return Some(token);
    }
    if token.starts_with('/') {
        return Some("<path>".to_string());
    }
    if contains_absolute_path(&token) {
        return Some("<path>".to_string());
    }

    Some(collapse_whitespace(&token))
}

pub fn sanitize_boot_security_intents(
    intents: &[BootSecurityIntentEvidence],
) -> Vec<BootSecurityIntentEvidence> {
    intents
        .iter()
        .map(|intent| BootSecurityIntentEvidence {
            class_id: intent.class_id.clone(),
            reason_code: intent.reason_code.clone(),
            command: normalize_token(&intent.command).unwrap_or_else(|| "unknown".to_string()),
            argv: intent
                .argv
                .iter()
                .filter_map(|token| normalize_token(token))
                .collect(),
            phase: intent.phase.clone(),
            lifecycle_paths: intent
                .lifecycle_paths
                .iter()
                .filter_map(|path| normalize_token(path))
                .collect(),
        })
        .collect()
}

pub fn sanitize_boot_security_intents_value(value: &str) -> Value {
    let Ok(mut value) = serde_json::from_str::<Value>(value) else {
        return Value::Array(Vec::new());
    };
    let Value::Array(intents) = &mut value else {
        return Value::Array(Vec::new());
    };
    for intent in intents {
        sanitize_boot_security_intent_value(intent);
    }
    value
}

pub fn sanitize_security_policy_intents(
    intents: &[SecurityPolicyIntent],
) -> Vec<SecurityPolicyIntent> {
    intents
        .iter()
        .cloned()
        .map(|mut intent| {
            sanitize_security_policy_intent(&mut intent);
            intent
        })
        .collect()
}

pub fn sanitize_security_policy_intents_value(value: &str) -> Value {
    let Ok(mut value) = serde_json::from_str::<Value>(value) else {
        return Value::Array(Vec::new());
    };
    sanitize_security_policy_intents_value_inner(&mut value);
    value
}

pub fn target_profile_for_distro(distro: &str) -> String {
    supported_profiles::route_by_slug(distro)
        .and_then(|route| {
            let ids = route.public_profile_ids();
            (ids.len() == 1).then(|| ids[0].clone())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn stable_cluster_key(input: &ClusterKeyInput) -> StableClusterKey {
    let normalized_command_shape_hash =
        conary_core::hash::sha256(input.normalized_command_shape.as_bytes());
    let canonical_tuple = json!({
        "schema_version": input.schema_version,
        "distro": input.distro,
        "target_profile": input.target_profile,
        "blocked_class": input.blocked_class,
        "command": input.command,
        "normalized_command_shape_hash": normalized_command_shape_hash,
        "lifecycle_phase": input.lifecycle_phase,
    });
    let bytes =
        serde_json::to_vec(&canonical_tuple).expect("canonical JSON tuple should serialize");
    StableClusterKey {
        cluster_key: format!("{CLUSTER_KEY_PREFIX}{}", conary_core::hash::sha256(&bytes)),
        normalized_command_shape_hash,
    }
}

fn normalize_command_token(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() {
        None
    } else {
        Some(collapse_whitespace(command))
    }
}

fn option_value(token: &str) -> Option<(&str, char, &str)> {
    if let Some((name, value)) = token.split_once('=') {
        return name.starts_with('-').then_some((name, '=', value));
    }
    if let Some((name, value)) = token.split_once(':') {
        return name.starts_with('-').then_some((name, ':', value));
    }
    None
}

fn normalize_option_value(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    let value = matching_quote_wrapper(value).unwrap_or(value);
    if ENV_ASSIGNMENT_RE.is_match(value) || EMBEDDED_ENV_ASSIGNMENT_RE.is_match(value) {
        return "<env-assignment>".to_string();
    }
    if ENV_REF_RE.is_match(value) {
        return "<env>".to_string();
    }

    let absolute_path_count = absolute_path_count(value);
    if absolute_path_count > 1 || (contains_parent_dir(value) && absolute_path_count > 0) {
        return "<path>".to_string();
    }
    if is_approved_lsm_path(value) {
        return value.to_string();
    }

    let value = KERNEL_VERSION_RE.replace_all(value, "<kver>").to_string();
    if contains_absolute_path(&value) {
        return "<path>".to_string();
    }
    collapse_whitespace(&value)
}

fn sanitize_security_policy_intent(intent: &mut SecurityPolicyIntent) {
    redact_private_string(&mut intent.schema);
    redact_private_string(&mut intent.id);
    redact_optional_private_string(&mut intent.source.source_format);
    redact_optional_private_string(&mut intent.source.source_distro);
    redact_optional_private_string(&mut intent.source.entry_id);
    sanitize_optional_string(&mut intent.source.command);
    sanitize_strings(&mut intent.source.argv);
    redact_optional_private_string(&mut intent.source.adapter_id);
    if let SecurityPolicyProvider::Unknown(value) = &mut intent.provider {
        redact_private_string(value);
    }
    redact_private_string(&mut intent.operation);
    redact_private_string(&mut intent.scope.kind);
    redact_optional_private_string(&mut intent.scope.name);
    sanitize_strings(&mut intent.scope.paths);
    redact_optional_private_string(&mut intent.scope.service);
    redact_optional_private_string(&mut intent.scope.port);
    sanitize_toml_map(&mut intent.scope.extra, SECURITY_POLICY_SCOPE_FIELDS);
    sanitize_toml_map(&mut intent.desired_state, &[]);
    redact_optional_private_string(&mut intent.requirements.provider_mode);
    redact_private_strings(&mut intent.requirements.tools);
    redact_private_strings(&mut intent.requirements.modules);
    if let SecurityPolicyFallback::Unknown(value) = &mut intent.fallback {
        redact_private_string(value);
    }
    sanitize_strings(&mut intent.payload_evidence.paths);
    redact_optional_private_string(&mut intent.payload_evidence.digest);
    if let SecurityPolicyReconciliationState::Unknown(value) = &mut intent.reconciliation.state {
        redact_private_string(value);
    }
    redact_optional_private_string(&mut intent.reconciliation.reason);
    redact_optional_private_string(&mut intent.reconciliation.target_provider);
    sanitize_toml_map(&mut intent.extra, SECURITY_POLICY_INTENT_FIELDS);
}

fn redact_private_string(value: &mut String) {
    if requires_privacy_redaction(value)
        && let Some(normalized) = normalize_token(value)
    {
        *value = normalized;
    }
}

fn redact_optional_private_string(value: &mut Option<String>) {
    if let Some(value) = value {
        redact_private_string(value);
    }
}

fn redact_private_strings(values: &mut [String]) {
    for value in values {
        redact_private_string(value);
    }
}

fn sanitize_string(value: &mut String) {
    if let Some(normalized) = normalize_token(value) {
        *value = normalized;
    }
}

fn sanitize_optional_string(value: &mut Option<String>) {
    if let Some(value) = value {
        sanitize_string(value);
    }
}

fn sanitize_strings(values: &mut [String]) {
    for value in values {
        sanitize_string(value);
    }
}

fn sanitize_toml_map(fields: &mut BTreeMap<String, toml::Value>, reserved_fields: &[&str]) {
    let mut sanitized = BTreeMap::new();
    let mut occupied = reserved_fields
        .iter()
        .map(|field| (*field).to_string())
        .collect::<BTreeSet<_>>();
    for (key, mut field) in
        ordered_sanitized_entries(std::mem::take(fields), normalize_dynamic_policy_string)
    {
        sanitize_toml_value(&mut field);
        let key = collision_safe_key(key, &mut occupied);
        sanitized.insert(key, field);
    }
    *fields = sanitized;
}

fn sanitize_toml_value(value: &mut toml::Value) {
    match value {
        toml::Value::String(value) => *value = normalize_dynamic_policy_string(value),
        toml::Value::Array(values) => {
            for value in values {
                sanitize_toml_value(value);
            }
        }
        toml::Value::Table(fields) => {
            let mut sanitized = toml::map::Map::new();
            let mut occupied = BTreeSet::new();
            for (key, mut field) in
                ordered_sanitized_entries(std::mem::take(fields), normalize_dynamic_policy_string)
            {
                sanitize_toml_value(&mut field);
                let key = collision_safe_key(key, &mut occupied);
                sanitized.insert(key, field);
            }
            *fields = sanitized;
        }
        toml::Value::Integer(_)
        | toml::Value::Float(_)
        | toml::Value::Boolean(_)
        | toml::Value::Datetime(_) => {}
    }
}

fn sanitize_boot_security_intent_value(value: &mut Value) {
    sanitize_json_value(value);
}

fn sanitize_security_policy_intents_value_inner(value: &mut Value) {
    sanitize_json_value(value);
}

fn sanitize_json_value(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            let mut sanitized = serde_json::Map::new();
            let mut occupied = BTreeSet::new();
            for (key, mut field) in
                ordered_sanitized_entries(std::mem::take(fields), normalize_evidence_key)
            {
                sanitize_json_value(&mut field);
                let key = collision_safe_key(key, &mut occupied);
                sanitized.insert(key, field);
            }
            *fields = sanitized;
        }
        Value::Array(values) => {
            for value in values {
                sanitize_json_value(value);
            }
        }
        Value::String(value) => sanitize_string(value),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn ordered_sanitized_entries<V>(
    entries: impl IntoIterator<Item = (String, V)>,
    normalize_key: fn(&str) -> String,
) -> Vec<(String, V)> {
    let mut entries = entries
        .into_iter()
        .map(|(original, value)| {
            let normalized = normalize_key(&original);
            (normalized != original, original, normalized, value)
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    entries
        .into_iter()
        .map(|(_, _, normalized, value)| (normalized, value))
        .collect()
}

fn normalize_evidence_key(value: &str) -> String {
    normalize_token(value).unwrap_or_else(|| value.to_string())
}

fn normalize_dynamic_policy_string(value: &str) -> String {
    let value = value.trim();
    let inspected = matching_quote_wrapper(value).unwrap_or(value);
    if requires_privacy_redaction(inspected) {
        normalize_token(inspected).unwrap_or_else(|| value.to_string())
    } else if inspected != value && is_approved_lsm_path(inspected) {
        inspected.to_string()
    } else {
        value.to_string()
    }
}

fn collision_safe_key(base: String, occupied: &mut BTreeSet<String>) -> String {
    if occupied.insert(base.clone()) {
        return base;
    }
    for suffix in 2usize.. {
        let candidate = format!("{base}#{suffix}");
        if occupied.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("unbounded numeric suffixes must yield an unused key")
}

fn is_approved_lsm_path(token: &str) -> bool {
    let path = std::path::Path::new(token);
    if !path.is_absolute() || contains_parent_dir(token) {
        return false;
    }

    token.starts_with("/etc/apparmor.d/")
        || token.starts_with("/etc/selinux/")
        || token.starts_with("/usr/share/selinux/")
}

fn is_approved_kernel_module_path(token: &str) -> bool {
    let path = std::path::Path::new(token);
    path.is_absolute()
        && !contains_parent_dir(token)
        && (token.starts_with("/lib/modules/<kver>/")
            || token.starts_with("/usr/lib/modules/<kver>/"))
}

fn contains_parent_dir(value: &str) -> bool {
    std::path::Path::new(value)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn requires_privacy_redaction(token: &str) -> bool {
    let token = token.trim();
    if token.is_empty() {
        return false;
    }
    if let Some(unwrapped) = matching_quote_wrapper(token) {
        return requires_privacy_redaction(unwrapped);
    }
    if ENV_ASSIGNMENT_RE.is_match(token) || ENV_REF_RE.is_match(token) {
        return true;
    }
    if let Some((_, _, value)) = option_value(token) {
        let value = value.trim();
        let value = matching_quote_wrapper(value).unwrap_or(value);
        return ENV_ASSIGNMENT_RE.is_match(value)
            || EMBEDDED_ENV_ASSIGNMENT_RE.is_match(value)
            || ENV_REF_RE.is_match(value)
            || contains_absolute_path(value);
    }
    if EMBEDDED_ENV_ASSIGNMENT_RE.is_match(token) {
        return true;
    }

    let absolute_path_count = absolute_path_count(token);
    if absolute_path_count > 1 || (contains_parent_dir(token) && absolute_path_count > 0) {
        return true;
    }
    if is_approved_lsm_path(token) || token.starts_with("/boot/") {
        return false;
    }
    let kernel_normalized = KERNEL_VERSION_RE.replace_all(token, "<kver>");
    if is_approved_kernel_module_path(&kernel_normalized) {
        return false;
    }

    token.starts_with('/') || absolute_path_count > 0
}

fn contains_absolute_path(value: &str) -> bool {
    EMBEDDED_ABSOLUTE_PATH_RE.is_match(value)
}

fn absolute_path_count(value: &str) -> usize {
    EMBEDDED_ABSOLUTE_PATH_RE.find_iter(value).count()
}

fn matching_quote_wrapper(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    match (bytes[0], bytes[bytes.len() - 1]) {
        (b'\'', b'\'') | (b'"', b'"') => Some(value[1..value.len() - 1].trim()),
        _ => None,
    }
}

fn env_assignment_has_quoted_absolute_path(token: &str) -> bool {
    token
        .split_once('=')
        .and_then(|(_, value)| matching_quote_wrapper(value.trim()))
        .is_some_and(contains_absolute_path)
}

fn collapse_whitespace(value: &str) -> String {
    WHITESPACE_RE.replace_all(value.trim(), " ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::scriptlet_evidence_queue::types::ClusterKeyInput;

    #[test]
    fn normalizes_kernel_boot_and_env_values() {
        let shape = normalize_command_shape(
            "dracut",
            &[
                "--force".to_string(),
                "/boot/initramfs-6.10.12-200.fc40.x86_64.img".to_string(),
                "$KERNEL_VERSION".to_string(),
                "SECRET=/home/remi/private".to_string(),
            ],
        );

        assert_eq!(
            shape,
            "dracut --force <boot>/initramfs-<kver>.img <env> <env-assignment>"
        );
        assert!(!shape.contains("remi"));
        assert!(!shape.contains("6.10.12-200"));
    }

    #[test]
    fn normalizes_option_embedded_absolute_paths_and_env_values() {
        let shape = normalize_command_shape(
            "semodule",
            &[
                "--root=/tmp/build-root".to_string(),
                "--module=/tmp/foo.pp".to_string(),
                "--install=/home/remi/private.pp".to_string(),
                "--define=SECRET=/home/remi/token".to_string(),
                "--policy:/home/remi/policy.pp".to_string(),
            ],
        );

        assert_eq!(
            shape,
            "semodule --root=<path> --module=<path> --install=<path> --define=<env-assignment> --policy:<path>"
        );
        assert!(!shape.contains("/tmp"));
        assert!(!shape.contains("/home"));
        assert!(!shape.contains("SECRET=/home"));
    }

    #[test]
    fn sanitizer_preserves_approved_lsm_policy_paths_and_redacts_private_values() {
        assert_eq!(
            normalize_token("/etc/apparmor.d/usr.bin.demo"),
            Some("/etc/apparmor.d/usr.bin.demo".to_string())
        );
        assert_eq!(
            normalize_token("/etc/selinux/targeted/policy/policy.33"),
            Some("/etc/selinux/targeted/policy/policy.33".to_string())
        );
        assert_eq!(
            normalize_token("/usr/share/selinux/packages/demo.pp"),
            Some("/usr/share/selinux/packages/demo.pp".to_string())
        );
        assert_eq!(
            normalize_token("/home/remi/private.pp"),
            Some("<path>".to_string())
        );
        assert_eq!(
            normalize_token("SECRET=/home/remi/token"),
            Some("<env-assignment>".to_string())
        );
    }

    #[test]
    fn sanitizer_redacts_quoted_and_wrapped_private_values() {
        for (token, expected) in [
            (r#"--replace="/home/remi/private.pp""#, "--replace=<path>"),
            ("--replace='/home/remi/private.pp'", "--replace=<path>"),
            (r#"--flag="TOKEN=value""#, "--flag=<env-assignment>"),
            ("--flag='TOKEN=value'", "--flag=<env-assignment>"),
            (r#""/home/remi/private.pp""#, "<path>"),
            ("'TOKEN=value'", "<env-assignment>"),
            (r#"prefix="/home/remi/private.pp""#, "<path>"),
            (
                r#"--profile="/etc/apparmor.d/vendor.1.2.3""#,
                "--profile=/etc/apparmor.d/vendor.1.2.3",
            ),
        ] {
            assert_eq!(
                normalize_token(token).as_deref(),
                Some(expected),
                "quoted private value must be sanitized: {token}"
            );
        }
    }

    #[test]
    fn sanitizer_rejects_traversal_under_approved_absolute_path_prefixes() {
        for path in [
            "/lib/modules/<kver>/../../../home/remi/private.ko",
            "/usr/lib/modules/<kver>/../../../../home/remi/private.ko",
            "/etc/apparmor.d/../../home/remi/private.pp",
            "/etc/selinux/../home/remi/private.pp",
            "/usr/share/selinux/../../../home/remi/private.pp",
        ] {
            assert_eq!(
                normalize_token(path),
                Some("<path>".to_string()),
                "approved absolute-path prefixes must not admit traversal: {path}"
            );
        }
    }

    #[test]
    fn sanitizer_redacts_boot_traversal_embedded_paths_and_env_assignments() {
        assert_eq!(
            normalize_token("/boot/../../home/remi/private.img"),
            Some("<path>".to_string())
        );
        assert_eq!(
            normalize_token("origin:/home/remi/private"),
            Some("<path>".to_string())
        );
        assert_eq!(
            normalize_token("file:///home/remi/private"),
            Some("<path>".to_string())
        );
        assert_eq!(
            normalize_token("--root=//home/remi/private"),
            Some("--root=<path>".to_string())
        );
        assert_eq!(
            normalize_token("note:SECRET=/home/remi/token"),
            Some("<env-assignment>".to_string())
        );
        assert_eq!(
            normalize_token("origin:/etc/apparmor.d/../../home/remi/private.pp"),
            Some("<path>".to_string())
        );
    }

    #[test]
    fn sanitizer_preserves_versioned_approved_lsm_paths() {
        for path in [
            "/etc/apparmor.d/vendor.1.2.3",
            "/etc/selinux/targeted/policy/policy.1.2.3",
            "/usr/share/selinux/packages/vendor.1.2.3.pp",
        ] {
            assert_eq!(normalize_token(path), Some(path.to_string()));
        }
    }

    #[test]
    fn sanitizer_redacts_private_suffixes_after_approved_roots() {
        for token in [
            "/etc/apparmor.d/vendor:/home/remi/private",
            "/etc/apparmor.d/vendor=/home/remi/private",
            "/etc/selinux/targeted/policy:/home/remi/private",
            "/etc/selinux/targeted/policy=/home/remi/private",
            "/usr/share/selinux/packages/vendor.pp:/home/remi/private",
            "/usr/share/selinux/packages/vendor.pp=/home/remi/private",
            "/boot/initramfs.img:/home/remi/private",
            "/boot/initramfs.img=/home/remi/private",
            "/lib/modules/6.10.12/vendor.ko:/home/remi/private",
            "/lib/modules/6.10.12/vendor.ko=/home/remi/private",
            "/usr/lib/modules/6.10.12/vendor.ko:/home/remi/private",
            "/usr/lib/modules/6.10.12/vendor.ko=/home/remi/private",
        ] {
            assert_eq!(
                normalize_token(token),
                Some("<path>".to_string()),
                "approved roots must not hide a second private path: {token}"
            );
        }
    }

    #[test]
    fn security_policy_value_sanitizer_redacts_private_object_keys() {
        let value = r#"[{"provider":"selinux","desired_state":{"/home/remi/private.pp":"enabled"},"/home/remi/private-extra":"value"}]"#;

        let sanitized = sanitize_security_policy_intents_value(value);
        let json = serde_json::to_string(&sanitized).unwrap();

        assert!(!json.contains("/home/remi"));
        assert!(json.contains("<path>"));
    }

    #[test]
    fn security_policy_value_sanitizer_preserves_colliding_dynamic_keys() {
        let intent = colliding_security_policy_intent();
        let value = serde_json::to_string(&[intent]).unwrap();

        let sanitized = sanitize_security_policy_intents_value(&value);
        let intents = sanitized.as_array().unwrap();
        let intent = intents[0].as_object().unwrap();
        let desired_state = intent["desired_state"].as_object().unwrap();
        let json = serde_json::to_string(&sanitized).unwrap();

        assert_eq!(intents.len(), 1);
        assert_eq!(intent["provider"], "selinux");
        assert_eq!(intent["provider#2"], "shadow");
        assert_eq!(desired_state.len(), 2);
        assert_eq!(desired_state["<path>"], "enabled");
        assert_eq!(desired_state["<path>#2"], "disabled");
        assert!(!json.contains("/home/remi"));
    }

    #[test]
    fn security_policy_intent_sanitizer_preserves_cardinality_and_structural_fields() {
        let sanitized = sanitize_security_policy_intents(&[colliding_security_policy_intent()]);
        let intent = &sanitized[0];
        let json = serde_json::to_string(&sanitized).unwrap();

        assert_eq!(sanitized.len(), 1);
        assert_eq!(intent.schema, "conary.security-policy-intent.v1");
        assert_eq!(intent.id, "scriptlet:0:post-install:selinux:test");
        assert_eq!(intent.provider.as_str(), "selinux");
        assert_eq!(intent.operation, "module-install");
        assert_eq!(intent.fallback.as_str(), "block-on-enforcing-target");
        assert_eq!(intent.reconciliation.state.as_str(), "review");
        assert_eq!(intent.desired_state.len(), 2);
        assert_eq!(
            intent.desired_state.get("<path>"),
            Some(&toml::Value::String("enabled".to_string()))
        );
        assert_eq!(
            intent.desired_state.get("<path>#2"),
            Some(&toml::Value::String("disabled".to_string()))
        );
        assert_eq!(
            intent.extra.get("provider#2"),
            Some(&toml::Value::String("shadow".to_string()))
        );
        assert!(!json.contains("/home/remi"));
    }

    #[test]
    fn security_policy_intent_sanitizer_preserves_safe_semantics_and_redacts_unsafe_semantics() {
        let mut safe = colliding_security_policy_intent();
        safe.schema = "schema-1.2.3".to_string();
        safe.id = "intent-1.2.3".to_string();
        safe.source.source_format = Some("format-1.2.3".to_string());
        safe.provider = SecurityPolicyProvider::Unknown("provider-1.2.3".to_string());
        safe.operation = "operation-1.2.3".to_string();
        safe.scope.kind = "kind-1.2.3".to_string();
        safe.fallback = SecurityPolicyFallback::Unknown("fallback-1.2.3".to_string());
        safe.reconciliation.state =
            SecurityPolicyReconciliationState::Unknown("state-1.2.3".to_string());
        safe.reconciliation.reason = Some("reason-1.2.3".to_string());

        let mut unsafe_intent = colliding_security_policy_intent();
        unsafe_intent.provider = SecurityPolicyProvider::Unknown("/home/remi/provider".to_string());
        unsafe_intent.operation = "note:SECRET=/home/remi/token".to_string();
        unsafe_intent.fallback =
            SecurityPolicyFallback::Unknown("origin:/home/remi/fallback".to_string());
        unsafe_intent.reconciliation.state = SecurityPolicyReconciliationState::Unknown(
            "file:///home/remi/reconciliation".to_string(),
        );

        let sanitized = sanitize_security_policy_intents(&[safe, unsafe_intent]);
        let safe = &sanitized[0];
        let unsafe_intent = &sanitized[1];

        assert_eq!(safe.schema, "schema-1.2.3");
        assert_eq!(safe.id, "intent-1.2.3");
        assert_eq!(safe.source.source_format.as_deref(), Some("format-1.2.3"));
        assert_eq!(safe.provider.as_str(), "provider-1.2.3");
        assert_eq!(safe.operation, "operation-1.2.3");
        assert_eq!(safe.scope.kind, "kind-1.2.3");
        assert_eq!(safe.fallback.as_str(), "fallback-1.2.3");
        assert_eq!(safe.reconciliation.state.as_str(), "state-1.2.3");
        assert_eq!(safe.reconciliation.reason.as_deref(), Some("reason-1.2.3"));
        assert_eq!(unsafe_intent.provider.as_str(), "<path>");
        assert_eq!(unsafe_intent.operation, "<env-assignment>");
        assert_eq!(unsafe_intent.fallback.as_str(), "<path>");
        assert_eq!(unsafe_intent.reconciliation.state.as_str(), "<path>");
    }

    #[test]
    fn security_policy_intent_sanitizer_preserves_dynamic_versioned_semantics() {
        let mut intent = colliding_security_policy_intent();
        intent.desired_state = BTreeMap::from([
            (
                "profile_path".to_string(),
                toml::Value::String("/etc/apparmor.d/vendor.1.2.3".to_string()),
            ),
            (
                "profile_name".to_string(),
                toml::Value::String("vendor.1.2.3".to_string()),
            ),
            (
                "private_path".to_string(),
                toml::Value::String("/home/remi/private.pp".to_string()),
            ),
            (
                "quoted_private_path".to_string(),
                toml::Value::String(r#""/home/remi/quoted-private.pp""#.to_string()),
            ),
            (
                "quoted_env_assignment".to_string(),
                toml::Value::String("'TOKEN=value'".to_string()),
            ),
        ]);
        intent.scope.extra.insert(
            "scope_profile_name".to_string(),
            toml::Value::String("vendor.1.2.3".to_string()),
        );
        intent.extra.clear();
        intent.extra.insert(
            "intent_profile_name".to_string(),
            toml::Value::String("vendor.1.2.3".to_string()),
        );

        let sanitized = sanitize_security_policy_intents(&[intent]);
        let intent = &sanitized[0];

        assert_eq!(
            intent.desired_state.get("profile_path"),
            Some(&toml::Value::String(
                "/etc/apparmor.d/vendor.1.2.3".to_string()
            ))
        );
        assert_eq!(
            intent.desired_state.get("profile_name"),
            Some(&toml::Value::String("vendor.1.2.3".to_string()))
        );
        assert_eq!(
            intent.scope.extra.get("scope_profile_name"),
            Some(&toml::Value::String("vendor.1.2.3".to_string()))
        );
        assert_eq!(
            intent.extra.get("intent_profile_name"),
            Some(&toml::Value::String("vendor.1.2.3".to_string()))
        );
        assert_eq!(
            intent.desired_state.get("private_path"),
            Some(&toml::Value::String("<path>".to_string()))
        );
        assert_eq!(
            intent.desired_state.get("quoted_private_path"),
            Some(&toml::Value::String("<path>".to_string()))
        );
        assert_eq!(
            intent.desired_state.get("quoted_env_assignment"),
            Some(&toml::Value::String("<env-assignment>".to_string()))
        );
    }

    #[test]
    fn cluster_key_ignores_architecture_and_package_identity() {
        let base = ClusterKeyInput {
            schema_version: 1,
            distro: "fedora".to_string(),
            target_profile: "fedora-44".to_string(),
            blocked_class: "initramfs".to_string(),
            command: "dracut".to_string(),
            normalized_command_shape: "dracut --force <boot>/initramfs-<kver>.img".to_string(),
            lifecycle_phase: "postinstall".to_string(),
        };

        let first = stable_cluster_key(&base);
        let second = stable_cluster_key(&ClusterKeyInput { ..base });

        assert_eq!(first.cluster_key, second.cluster_key);
        assert!(first.cluster_key.starts_with("s1-"));
        assert!(!first.cluster_key.contains(':'));
    }

    #[test]
    fn target_profile_uses_single_public_route_or_unknown() {
        assert_eq!(target_profile_for_distro("fedora"), "fedora-44");
        assert_eq!(target_profile_for_distro("ubuntu"), "ubuntu-26.04");
        assert_eq!(target_profile_for_distro("arch"), "arch");
        assert_eq!(target_profile_for_distro("debian"), "unknown");
    }

    fn colliding_security_policy_intent() -> SecurityPolicyIntent {
        serde_json::from_value(serde_json::json!({
            "schema": "conary.security-policy-intent.v1",
            "id": "scriptlet:0:post-install:selinux:test",
            "provider": "selinux",
            "operation": "module-install",
            "desired_state": {
                "/home/remi/a.pp": "enabled",
                "/home/remi/b.pp": "disabled"
            },
            "fallback": "block-on-enforcing-target",
            "reconciliation": {
                "state": "review"
            },
            "provider ": "shadow"
        }))
        .unwrap()
    }
}
