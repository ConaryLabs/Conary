// conary-core/src/recipe/kitchen/reproducibility_env/shell_syntax.rs

use super::*;

pub(super) fn validate_no_shell_expansion(phase: &str, token: &str, context: &str) -> Result<()> {
    if has_dynamic_shell_expansion(token) {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support dynamic shell expansion in {context} token {token} in {phase} phase"
        )));
    }
    Ok(())
}

pub(super) fn has_dynamic_shell_expansion(token: &str) -> bool {
    for ch in token.chars() {
        if matches!(ch, '$' | '{' | '}' | '*' | '?' | '[' | '\\' | '"' | '\'') {
            return true;
        }
    }
    false
}

pub(super) fn validate_no_command_substitution(phase: &str, line: &str) -> Result<()> {
    if line.contains("$(") || line.contains('`') {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support command substitution in {phase} phase"
        )));
    }
    if line.contains("<(") || line.contains(">(") {
        return Err(Error::ConfigError(format!(
            "hermetic reproducibility does not support process substitution in {phase} phase"
        )));
    }
    Ok(())
}

pub(super) fn is_make_command(command: &str) -> bool {
    matches!(command, "make" | "gmake")
}

pub(super) fn command_local_env_error(phase: &str, key: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects command-local {key} assignment in {phase} phase"
    ))
}

pub(super) fn command_local_env_clear_error(phase: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects command-local environment clearing in {phase} phase"
    ))
}

pub(super) fn shell_keyword_mode_error(phase: &str, option: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects shell keyword-mode {option} in {phase} phase"
    ))
}

pub(super) fn shell_alias_expansion_error(phase: &str, surface: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects shell alias expansion surface {surface} in {phase} phase"
    ))
}

pub(super) fn nested_shell_stdin_error(phase: &str, shell: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects nested shell {shell} reading script from stdin in {phase} phase"
    ))
}

pub(super) fn nested_shell_script_error(phase: &str, shell: &str, script: &str) -> Error {
    Error::ConfigError(format!(
        "hermetic reproducibility rejects nested shell {shell} script operand {script} in {phase} phase"
    ))
}

pub(super) fn is_controlled_reproducibility_key(key: &str) -> bool {
    controlled_reproducibility_target(key).is_some()
}

pub(super) fn controlled_reproducibility_target(target: &str) -> Option<&'static str> {
    let (base, _) = shell_variable_base(target)?;
    ReproducibilityConfig::controlled_env_keys()
        .iter()
        .copied()
        .find(|key| *key == base)
}

pub(super) fn controlled_key_mentioned_in_expression(expression: &str) -> Option<&'static str> {
    ReproducibilityConfig::controlled_env_keys()
        .iter()
        .copied()
        .find(|key| shell_identifier_present(expression, key))
}

pub(super) fn shell_identifier_present(expression: &str, name: &str) -> bool {
    let mut start = None;
    for (index, ch) in expression.char_indices() {
        if is_shell_identifier_char(ch) {
            start.get_or_insert(index);
            continue;
        }
        if let Some(identifier_start) = start.take()
            && &expression[identifier_start..index] == name
        {
            return true;
        }
    }
    start
        .map(|identifier_start| &expression[identifier_start..] == name)
        .unwrap_or(false)
}

pub(super) fn is_shell_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

pub(super) fn shell_assignment(token: &str) -> Option<(String, String, bool)> {
    let (key, value) = token.split_once('=')?;
    let (key, is_array_target) = shell_variable_base(key)?;
    Some((key.to_string(), value.to_string(), is_array_target))
}

pub(super) fn shell_append_assignment(token: &str) -> Option<(String, bool)> {
    let (key, _) = token.split_once("+=")?;
    let (key, is_array_target) = shell_variable_base(key)?;
    Some((key.to_string(), is_array_target))
}

pub(super) fn shell_variable_base(target: &str) -> Option<(&str, bool)> {
    if is_shell_env_name(target) {
        return Some((target, false));
    }
    let (base, subscript) = target.split_once('[')?;
    if subscript.ends_with(']') && is_shell_env_name(base) {
        return Some((base, true));
    }
    None
}

pub(super) fn is_shell_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(super) fn command_basename(command: &str) -> &str {
    command.rsplit('/').next().unwrap_or(command)
}

pub(super) fn clean_shell_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| matches!(ch, '"' | '\''))
        .to_string()
}

pub(super) fn split_shell_env_segments(line: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut quote = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            current.push(ch);
            escaped = true;
            continue;
        }
        if let Some(quote_ch) = quote {
            current.push(ch);
            if ch == quote_ch {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            current.push(ch);
            quote = Some(ch);
            continue;
        }

        match ch {
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                push_shell_env_segment(&mut segments, &mut current);
            }
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                push_shell_env_segment(&mut segments, &mut current);
            }
            ';' | '(' | ')' | '`' => push_shell_env_segment(&mut segments, &mut current),
            '$' if chars.peek() == Some(&'(') => {
                chars.next();
                push_shell_env_segment(&mut segments, &mut current);
            }
            _ => current.push(ch),
        }
    }

    push_shell_env_segment(&mut segments, &mut current);
    segments
}

pub(super) fn push_shell_env_segment(segments: &mut Vec<String>, current: &mut String) {
    let segment = current.trim();
    if !segment.is_empty() {
        segments.push(segment.to_string());
    }
    current.clear();
}
