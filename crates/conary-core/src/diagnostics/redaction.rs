// crates/conary-core/src/diagnostics/redaction.rs

use serde_json::Value;

use super::RedactionMarker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedText {
    pub value: String,
    pub redactions: Vec<RedactionMarker>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedCommand {
    pub value: Vec<String>,
    pub redactions: Vec<RedactionMarker>,
}

const SECRET_KEYS: &[&str] = &[
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "API_KEY",
    "ACCESS_KEY",
    "PRIVATE_KEY",
];

pub const MAX_DIAGNOSTIC_LOG_BYTES: usize = 16 * 1024;

pub fn redact_text(input: &str) -> RedactedText {
    let mut value = input.to_string();
    let mut redactions = Vec::new();

    if is_private_key_path(&value) {
        return RedactedText {
            value: "[REDACTED-PATH]".to_string(),
            redactions: vec![RedactionMarker::new("text", "private-key-path")],
        };
    }

    for path in private_key_path_tokens(&value) {
        value = value.replace(&path, "[REDACTED-PATH]");
        redactions.push(RedactionMarker::new("text", "private-key-path"));
    }

    if let Some(redacted) = redact_credentialed_url(&value) {
        value = redacted;
        redactions.push(RedactionMarker::new("text", "credentialed-url"));
    }

    for key in SECRET_KEYS {
        let marker = format!("{key}=");
        if let Some(start) = value.to_ascii_uppercase().find(&marker) {
            let key_start = start;
            let value_start = start + marker.len();
            let value_end = value[value_start..]
                .find(|ch: char| ch.is_whitespace() || ch == '\'' || ch == '"')
                .map(|offset| value_start + offset)
                .unwrap_or_else(|| value.len());
            value.replace_range(key_start..value_end, &format!("{key}=[REDACTED]"));
            redactions.push(RedactionMarker::new("text", "secret-env-assignment"));
        }
    }

    for prefix in ["Bearer ", "bearer "] {
        let mut search_start = 0;
        while let Some(relative_start) = value[search_start..].find(prefix) {
            let start = search_start + relative_start;
            let token_start = start + prefix.len();
            if value[token_start..].starts_with("[REDACTED]") {
                search_start = token_start + "[REDACTED]".len();
                continue;
            }
            let token_end = value[token_start..]
                .find(|ch: char| ch.is_whitespace() || ch == '\'' || ch == '"')
                .map(|offset| token_start + offset)
                .unwrap_or_else(|| value.len());
            value.replace_range(token_start..token_end, "[REDACTED]");
            redactions.push(RedactionMarker::new("text", "bearer-token"));
            search_start = token_start + "[REDACTED]".len();
        }
    }

    RedactedText { value, redactions }
}

fn private_key_path_tokens(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|ch: char| {
                ch == '\'' || ch == '"' || ch == ',' || ch == ';' || ch == ':' || ch == ')'
            })
        })
        .filter(|token| is_private_key_path(token))
        .map(ToOwned::to_owned)
        .collect()
}

pub fn redact_log(input: &str) -> RedactedText {
    let mut redactions = Vec::new();
    let bounded = if input.len() > MAX_DIAGNOSTIC_LOG_BYTES {
        let mut end = MAX_DIAGNOSTIC_LOG_BYTES;
        while !input.is_char_boundary(end) {
            end -= 1;
        }
        redactions.push(RedactionMarker::new("log", "log-truncated"));
        format!("{}\n[TRUNCATED]", &input[..end])
    } else {
        input.to_string()
    };
    let redacted = redact_text(&bounded);
    redactions.extend(redacted.redactions);
    RedactedText {
        value: redacted.value,
        redactions,
    }
}

pub fn redact_command(command: &[String]) -> RedactedCommand {
    let mut redacted = Vec::with_capacity(command.len());
    let mut markers = Vec::new();
    for arg in command {
        let item = redact_text(arg);
        redacted.push(item.value);
        markers.extend(item.redactions);
    }
    RedactedCommand {
        value: redacted,
        redactions: markers,
    }
}

pub fn redact_json_value(value: &mut Value, field: &str) -> Vec<RedactionMarker> {
    match value {
        Value::String(text) => {
            let redacted = redact_text(text);
            *text = redacted.value;
            redacted
                .redactions
                .into_iter()
                .map(|item| RedactionMarker::new(field, item.reason))
                .collect()
        }
        Value::Array(items) => {
            let mut redactions = Vec::new();
            for (index, item) in items.iter_mut().enumerate() {
                redactions.extend(redact_json_value(item, &format!("{field}[{index}]")));
            }
            redactions
        }
        Value::Object(map) => {
            let mut redactions = Vec::new();
            for (key, item) in map {
                redactions.extend(redact_json_value(item, &format!("{field}.{key}")));
            }
            redactions
        }
        _ => Vec::new(),
    }
}

fn is_private_key_path(value: &str) -> bool {
    let token = value
        .trim_matches(|ch: char| ch == '\'' || ch == '"' || ch == ',' || ch == ';' || ch == ':');
    let path_like = token.starts_with('/') || token.starts_with('~') || token.starts_with("./");
    path_like
        && (token.contains("/.ssh/id_")
            || token.ends_with("/id_rsa")
            || token.ends_with("/id_ed25519")
            || token.ends_with(".pem")
            || token.ends_with(".key")
            || token.contains("/private_key"))
}

fn redact_credentialed_url(value: &str) -> Option<String> {
    let scheme_end = value.find("://")?;
    let rest_start = scheme_end + 3;
    let at = value[rest_start..].find('@')? + rest_start;
    let slash = value[rest_start..]
        .find('/')
        .map(|offset| rest_start + offset)
        .unwrap_or(value.len());
    if at > slash || !value[rest_start..at].contains(':') {
        return None;
    }
    Some(format!(
        "{}[REDACTED]@{}",
        &value[..rest_start],
        &value[at + 1..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_token_env_assignments_and_bearer_values() {
        let value = redact_text("API_TOKEN=sk-secret curl -H 'Authorization: Bearer abc.def'");
        assert!(!value.value.contains("sk-secret"));
        assert!(!value.value.contains("abc.def"));
        assert!(value.value.contains("API_TOKEN=[REDACTED]"));
        assert!(value.value.contains("Bearer [REDACTED]"));
        assert!(value.redactions.iter().any(|item| item.field == "text"));
    }

    #[test]
    fn redacts_credentialed_urls() {
        let value = redact_text("https://user:pass@example.invalid/source.tar.gz");
        assert_eq!(
            value.value,
            "https://[REDACTED]@example.invalid/source.tar.gz"
        );
        assert_eq!(value.redactions[0].reason, "credentialed-url");
    }

    #[test]
    fn redacts_private_key_paths() {
        let value = redact_text("/home/dev/.ssh/id_ed25519");
        assert_eq!(value.value, "[REDACTED-PATH]");
        assert_eq!(value.redactions[0].reason, "private-key-path");
    }

    #[test]
    fn redacts_embedded_private_key_paths() {
        let value = redact_text("failed for /home/dev/.conary/keys/root.pem");
        assert_eq!(value.value, "failed for [REDACTED-PATH]");
        assert_eq!(value.redactions[0].reason, "private-key-path");
    }

    #[test]
    fn does_not_redact_generic_pem_or_key_words_without_path_shape() {
        let value = redact_text("documented files include bundle.pem and api.key examples");
        assert_eq!(
            value.value,
            "documented files include bundle.pem and api.key examples"
        );
        assert!(value.redactions.is_empty());
    }

    #[test]
    fn redact_command_preserves_argument_boundaries() {
        let command = redact_command(&[
            "curl".to_string(),
            "-H".to_string(),
            "Authorization: Bearer abc.def".to_string(),
        ]);
        assert_eq!(command.value[0], "curl");
        assert_eq!(command.value[2], "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn redact_log_redacts_and_bounds_long_output() {
        let input = format!(
            "Authorization: Bearer abc.def\n{}",
            "x".repeat(MAX_DIAGNOSTIC_LOG_BYTES + 64)
        );
        let value = redact_log(&input);
        assert!(!value.value.contains("abc.def"));
        assert!(value.value.contains("Bearer [REDACTED]"));
        assert!(value.value.contains("[TRUNCATED]"));
        assert!(
            value
                .redactions
                .iter()
                .any(|item| item.reason == "log-truncated")
        );
    }

    #[test]
    fn redact_json_value_walks_nested_metadata() {
        let mut value = serde_json::json!({
            "publish_lint_report": {
                "url": "https://user:pass@example.invalid/pkg.ccs",
                "nested": ["API_TOKEN=sk-secret"]
            }
        });
        let redactions = redact_json_value(&mut value, "metadata");
        let text = value.to_string();
        assert!(!text.contains("user:pass"));
        assert!(!text.contains("sk-secret"));
        assert!(text.contains("[REDACTED]"));
        assert!(
            redactions
                .iter()
                .any(|item| item.field.contains("metadata"))
        );
    }
}
