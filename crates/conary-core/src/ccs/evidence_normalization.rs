// conary-core/src/ccs/evidence_normalization.rs
//! Canonical privacy-safe normalization for persisted scriptlet evidence.

use regex::Regex;
use std::sync::LazyLock;

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

pub fn normalize_command_shape(command: &str, argv: &[String]) -> String {
    let mut parts = Vec::with_capacity(argv.len() + 1);
    if let Some(command) = normalize_command_token(command) {
        parts.push(command);
    }
    parts.extend(argv.iter().filter_map(|token| normalize_token(token)));
    collapse_whitespace(&parts.join(" "))
}

pub fn normalize_command_token(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let command = command.rsplit('/').next().unwrap_or(command);
    Some(collapse_whitespace(command))
}

pub fn normalize_tokens(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .map(|token| normalize_token(token).unwrap_or_else(|| "<empty>".to_string()))
        .collect()
}

pub fn normalize_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if let Some(unwrapped) = matching_quote_wrapper(token)
        && (requires_privacy_redaction(unwrapped) || is_approved_policy_path(unwrapped))
    {
        return normalize_token(unwrapped);
    }
    if ENV_ASSIGNMENT_RE.is_match(token) {
        return Some("<env-assignment>".to_string());
    }
    if ENV_REF_RE.is_match(token) {
        return Some("<env>".to_string());
    }
    if let Some((name, separator, value)) = option_value(token) {
        let value = if is_sensitive_option(name) {
            "<redacted>".to_string()
        } else {
            normalize_option_value(value)
        };
        return Some(format!("{name}{separator}{value}"));
    }
    if EMBEDDED_ENV_ASSIGNMENT_RE.is_match(token) {
        return Some("<env-assignment>".to_string());
    }
    if token.contains("://") {
        return Some("<url>".to_string());
    }
    let absolute_path_count = absolute_path_count(token);
    if absolute_path_count > 1 || (contains_parent_dir(token) && absolute_path_count > 0) {
        return Some("<path>".to_string());
    }
    if is_approved_policy_path(token) {
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
    if token.starts_with('/') || contains_absolute_path(&token) {
        return Some("<path>".to_string());
    }

    Some(collapse_whitespace(&token))
}

pub fn requires_privacy_redaction(token: &str) -> bool {
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
    if let Some((name, _, value)) = option_value(token) {
        if is_sensitive_option(name) {
            return true;
        }
        let value = value.trim();
        let value = matching_quote_wrapper(value).unwrap_or(value);
        return ENV_ASSIGNMENT_RE.is_match(value)
            || EMBEDDED_ENV_ASSIGNMENT_RE.is_match(value)
            || ENV_REF_RE.is_match(value)
            || value.contains("://")
            || contains_absolute_path(value);
    }
    if EMBEDDED_ENV_ASSIGNMENT_RE.is_match(token) || token.contains("://") {
        return true;
    }

    let absolute_path_count = absolute_path_count(token);
    if absolute_path_count > 1 || (contains_parent_dir(token) && absolute_path_count > 0) {
        return true;
    }
    if is_approved_policy_path(token) || token.starts_with("/boot/") {
        return false;
    }
    let kernel_normalized = KERNEL_VERSION_RE.replace_all(token, "<kver>");
    if is_approved_kernel_module_path(&kernel_normalized) {
        return false;
    }

    token.starts_with('/') || absolute_path_count > 0
}

pub fn matching_quote_wrapper(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    match (bytes[0], bytes[bytes.len() - 1]) {
        (b'\'', b'\'') | (b'"', b'"') => Some(value[1..value.len() - 1].trim()),
        _ => None,
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
    let value = option_value_for_inspection(value);
    if ENV_ASSIGNMENT_RE.is_match(value) || EMBEDDED_ENV_ASSIGNMENT_RE.is_match(value) {
        return "<env-assignment>".to_string();
    }
    if ENV_REF_RE.is_match(value) {
        return "<env>".to_string();
    }
    if value.contains("://") {
        return "<url>".to_string();
    }

    let absolute_path_count = absolute_path_count(value);
    if absolute_path_count > 1 || (contains_parent_dir(value) && absolute_path_count > 0) {
        return "<path>".to_string();
    }
    if is_approved_policy_path(value) {
        return value.to_string();
    }

    let value = KERNEL_VERSION_RE.replace_all(value, "<kver>").to_string();
    if contains_absolute_path(&value) {
        return "<path>".to_string();
    }
    collapse_whitespace(&value)
}

fn is_sensitive_option(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "credential",
        "api-key",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

pub fn is_approved_policy_path(token: &str) -> bool {
    let path = std::path::Path::new(token);
    if !path.is_absolute()
        || contains_parent_dir(token)
        || token.contains('"')
        || token.contains('\'')
    {
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

fn contains_absolute_path(value: &str) -> bool {
    EMBEDDED_ABSOLUTE_PATH_RE.is_match(value)
}

fn absolute_path_count(value: &str) -> usize {
    EMBEDDED_ABSOLUTE_PATH_RE.find_iter(value).count()
}

fn option_value_for_inspection(value: &str) -> &str {
    matching_quote_wrapper(value)
        .or_else(|| value.strip_prefix('"'))
        .or_else(|| value.strip_prefix('\''))
        .unwrap_or(value)
}

fn collapse_whitespace(value: &str) -> String {
    WHITESPACE_RE.replace_all(value.trim(), " ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_command_context_without_persisting_private_values() {
        let shape = normalize_command_shape(
            "/usr/libexec/custom-helper",
            &[
                "--root=/tmp/build-root".to_string(),
                "--token=secret".to_string(),
                "/boot/initramfs-6.10.12-200.fc40.x86_64.img".to_string(),
                "SECRET=/home/remi/private".to_string(),
            ],
        );

        assert_eq!(
            shape,
            "custom-helper --root=<path> --token=<redacted> <boot>/initramfs-<kver>.img <env-assignment>"
        );
        assert!(!shape.contains("/tmp"));
        assert!(!shape.contains("secret"));
        assert!(!shape.contains("remi"));
        assert!(!shape.contains("6.10.12-200"));
    }

    #[test]
    fn preserves_approved_policy_paths_but_rejects_traversal() {
        assert_eq!(
            normalize_token("/etc/apparmor.d/usr.bin.demo"),
            Some("/etc/apparmor.d/usr.bin.demo".to_string())
        );
        assert_eq!(
            normalize_token("/etc/apparmor.d/../../home/remi/private"),
            Some("<path>".to_string())
        );
    }

    #[test]
    fn command_argv_normalization_uses_the_same_token_contract_as_queue_identity() {
        let argv = vec![
            "--profile=/etc/apparmor.d/vendor.1.2.3".to_string(),
            "https://example.invalid/private".to_string(),
            "$PRIVATE_PATH".to_string(),
        ];

        assert_eq!(
            normalize_tokens(&argv),
            ["--profile=/etc/apparmor.d/vendor.1.2.3", "<url>", "<env>"]
        );
        assert_eq!(
            normalize_command_shape("apparmor_parser", &normalize_tokens(&argv)),
            "apparmor_parser --profile=/etc/apparmor.d/vendor.1.2.3 <url> <env>"
        );
    }

    #[test]
    fn argument_normalization_preserves_argv_positions() {
        let argv = vec!["".to_string(), "  ".to_string(), "--flag".to_string()];

        assert_eq!(normalize_tokens(&argv), ["<empty>", "<empty>", "--flag"]);
    }
}
