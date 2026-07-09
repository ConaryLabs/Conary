// apps/remi/src/server/scriptlet_evidence_queue/normalization.rs

use std::sync::LazyLock;

use conary_core::ccs::legacy_scriptlets::BootSecurityIntentEvidence;
use conary_core::ccs::security_policy::SecurityPolicyIntent;
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
static KERNEL_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+\.\d+\.\d+[-._A-Za-z0-9]*").unwrap());
static WHITESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static EMBEDDED_ABSOLUTE_PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[=,:;\s])/(?:$|[A-Za-z0-9._@%+-][^\s,;:]*)").unwrap());

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
    if ENV_ASSIGNMENT_RE.is_match(token) {
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

    let had_img_extension = token.ends_with(".img");
    let mut token = KERNEL_VERSION_RE.replace_all(token, "<kver>").to_string();
    if had_img_extension && !token.ends_with(".img") {
        token.push_str(".img");
    }
    if let Some(rest) = token.strip_prefix("/boot/") {
        return Some(format!("<boot>/{rest}"));
    }
    if is_approved_absolute_path(&token) {
        return Some(token);
    }
    if token.starts_with('/') {
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
    let mut value = serde_json::to_value(intents).unwrap_or_else(|_| Value::Array(Vec::new()));
    sanitize_security_policy_intents_value_inner(&mut value);
    serde_json::from_value(value).unwrap_or_default()
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
    if ENV_ASSIGNMENT_RE.is_match(value) {
        return "<env-assignment>".to_string();
    }
    if ENV_REF_RE.is_match(value) {
        return "<env>".to_string();
    }

    let value = KERNEL_VERSION_RE.replace_all(value, "<kver>").to_string();
    if contains_absolute_path(&value) {
        return "<path>".to_string();
    }
    collapse_whitespace(&value)
}

fn sanitize_boot_security_intent_value(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            let mut sanitized = serde_json::Map::new();
            for (key, mut field) in std::mem::take(fields) {
                sanitize_boot_security_intent_value(&mut field);
                let key = normalize_token(&key).unwrap_or(key);
                sanitized.insert(key, field);
            }
            *fields = sanitized;
        }
        Value::Array(values) => {
            for value in values {
                sanitize_boot_security_intent_value(value);
            }
        }
        Value::String(value) => {
            if let Some(normalized) = normalize_token(value) {
                *value = normalized;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn sanitize_security_policy_intents_value_inner(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            let mut sanitized = serde_json::Map::new();
            for (key, mut field) in std::mem::take(fields) {
                sanitize_security_policy_intents_value_inner(&mut field);
                let key = normalize_token(&key).unwrap_or(key);
                sanitized.insert(key, field);
            }
            *fields = sanitized;
        }
        Value::Array(values) => {
            for value in values {
                sanitize_security_policy_intents_value_inner(value);
            }
        }
        Value::String(value) => {
            if let Some(normalized) = normalize_token(value) {
                *value = normalized;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn is_approved_absolute_path(token: &str) -> bool {
    let path = std::path::Path::new(token);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return false;
    }

    token.starts_with("/lib/modules/<kver>/")
        || token.starts_with("/usr/lib/modules/<kver>/")
        || token.starts_with("/etc/apparmor.d/")
        || token.starts_with("/etc/selinux/")
        || token.starts_with("/usr/share/selinux/")
}

fn contains_absolute_path(value: &str) -> bool {
    EMBEDDED_ABSOLUTE_PATH_RE.is_match(value)
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
    fn security_policy_value_sanitizer_redacts_private_object_keys() {
        let value = r#"[{"provider":"selinux","desired_state":{"/home/remi/private.pp":"enabled"},"/home/remi/private-extra":"value"}]"#;

        let sanitized = sanitize_security_policy_intents_value(value);
        let json = serde_json::to_string(&sanitized).unwrap();

        assert!(!json.contains("/home/remi"));
        assert!(json.contains("<path>"));
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
}
