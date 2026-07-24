// apps/remi/src/server/scriptlet_evidence_queue/classification.rs

/// Version of the queue-only unknown-command classification contract.
///
/// This does not change CCS conversion, adapter selection, legacy replay, or
/// publication authority. It only decides whether an already-unknown command
/// is useful adapter-planning evidence in Remi's admin queue.
pub const UNKNOWN_COMMAND_NORMALIZATION_VERSION: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownCommandDisposition {
    Signal,
    StructuralNoise(&'static str),
}

impl UnknownCommandDisposition {
    pub fn noise_reason(self) -> Option<&'static str> {
        match self {
            Self::Signal => None,
            Self::StructuralNoise(reason) => Some(reason),
        }
    }
}

pub fn classify_unknown_command(command: &str) -> UnknownCommandDisposition {
    let token = command.trim();
    if token.is_empty() {
        return UnknownCommandDisposition::StructuralNoise("empty-token");
    }

    let unquoted = strip_matching_quotes(token);
    if is_function_definition(unquoted) {
        return UnknownCommandDisposition::StructuralNoise("function-definition");
    }
    if is_variable_reference(unquoted) {
        return UnknownCommandDisposition::StructuralNoise("variable-reference");
    }
    if is_environment_assignment(unquoted) {
        return UnknownCommandDisposition::StructuralNoise("environment-assignment");
    }
    if is_shell_punctuation(unquoted) {
        return UnknownCommandDisposition::StructuralNoise("shell-punctuation");
    }

    let dispatch_syntax = unquoted.ends_with(')') || unquoted.ends_with(':');
    let bare = unquoted.trim_matches(|ch: char| matches!(ch, '(' | ')' | '{' | '}' | ';' | ':'));

    if is_shell_keyword(bare) {
        return UnknownCommandDisposition::StructuralNoise("shell-keyword");
    }
    if is_queue_structural_builtin(bare) {
        return UnknownCommandDisposition::StructuralNoise("shell-builtin");
    }
    if is_unambiguous_maintainer_dispatch_label(bare)
        || (dispatch_syntax && is_ambiguous_dpkg_dispatch_label(bare))
    {
        return UnknownCommandDisposition::StructuralNoise("maintainer-dispatch-label");
    }

    UnknownCommandDisposition::Signal
}

fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() < 2 {
        return value;
    }
    match (bytes[0], bytes[bytes.len() - 1]) {
        (b'\'', b'\'') | (b'"', b'"') => value[1..value.len() - 1].trim(),
        _ => value,
    }
}

fn is_function_definition(token: &str) -> bool {
    token.strip_suffix("()").is_some_and(is_shell_identifier)
}

fn is_variable_reference(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('$') else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    if matches!(rest, "@" | "*" | "#" | "?" | "$" | "!" | "-" | "0")
        || rest.chars().all(|ch| ch.is_ascii_digit())
    {
        return true;
    }
    if rest.starts_with('{') && rest.ends_with('}') {
        return true;
    }
    if rest.starts_with('(') {
        return true;
    }
    is_shell_identifier(rest)
}

fn is_environment_assignment(token: &str) -> bool {
    token
        .split_once('=')
        .is_some_and(|(name, _)| is_shell_identifier(name))
}

fn is_shell_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_shell_punctuation(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|ch| {
            matches!(
                ch,
                '{' | '}' | '(' | ')' | '[' | ']' | ';' | ':' | '|' | '&' | '!'
            )
        })
}

fn is_shell_keyword(token: &str) -> bool {
    matches!(
        token,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "do"
            | "done"
            | "while"
            | "until"
            | "for"
            | "in"
            | "case"
            | "esac"
            | "select"
            | "function"
            | "time"
            | "coproc"
    )
}

fn is_queue_structural_builtin(token: &str) -> bool {
    matches!(
        token,
        ":" | "["
            | "[["
            | "break"
            | "continue"
            | "declare"
            | "dirs"
            | "echo"
            | "exit"
            | "export"
            | "false"
            | "getopts"
            | "hash"
            | "help"
            | "history"
            | "jobs"
            | "let"
            | "local"
            | "printf"
            | "pwd"
            | "read"
            | "readonly"
            | "return"
            | "set"
            | "shift"
            | "test"
            | "times"
            | "true"
            | "type"
            | "typeset"
            | "unset"
            | "wait"
    )
}

fn is_unambiguous_maintainer_dispatch_label(token: &str) -> bool {
    matches!(
        token,
        "preinst"
            | "postinst"
            | "prerm"
            | "postrm"
            | "abort-upgrade"
            | "abort-remove"
            | "abort-deconfigure"
            | "failed-upgrade"
            | "disappear"
            | "triggered"
            | "pre_install"
            | "post_install"
            | "pre_upgrade"
            | "post_upgrade"
            | "pre_remove"
            | "post_remove"
            | "pre_transaction"
            | "post_transaction"
            | "%pre"
            | "%post"
            | "%preun"
            | "%postun"
            | "%trigger"
            | "%triggerin"
            | "%triggerun"
            | "%triggerpostun"
            | "%pretrans"
            | "%posttrans"
    )
}

fn is_ambiguous_dpkg_dispatch_label(token: &str) -> bool {
    matches!(
        token,
        "configure" | "install" | "upgrade" | "remove" | "purge" | "deconfigure"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_provable_shell_and_package_manager_structure_as_noise() {
        for (command, reason) in [
            ("set", "shell-builtin"),
            ("true", "shell-builtin"),
            ("exit", "shell-builtin"),
            ("$1", "variable-reference"),
            ("${DPKG_MAINTSCRIPT_PACKAGE}", "variable-reference"),
            ("NAME=value", "environment-assignment"),
            ("{", "shell-punctuation"),
            (":", "shell-punctuation"),
            ("then", "shell-keyword"),
            ("helper()", "function-definition"),
            ("configure)", "maintainer-dispatch-label"),
            ("postinst", "maintainer-dispatch-label"),
            ("post_install", "maintainer-dispatch-label"),
            ("%posttrans", "maintainer-dispatch-label"),
        ] {
            assert_eq!(
                classify_unknown_command(command),
                UnknownCommandDisposition::StructuralNoise(reason),
                "{command}"
            );
        }
    }

    #[test]
    fn keeps_ambiguous_effectful_and_security_sensitive_commands_visible() {
        for command in [
            "custom-helper",
            "configure",
            "install",
            "eval",
            "exec",
            "source",
            "kill",
            "umask",
            "dracut",
            "semanage",
            "depmod",
            "curl",
            "dnf",
            "pam-auth-update",
        ] {
            assert_eq!(
                classify_unknown_command(command),
                UnknownCommandDisposition::Signal,
                "{command}"
            );
        }
    }
}
