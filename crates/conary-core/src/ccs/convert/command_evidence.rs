// conary-core/src/ccs/convert/command_evidence.rs

use crate::packages::native_abi::{
    NativeLifecyclePath, NativeScriptletEntry, NativeScriptletKind, NativeScriptletSupport,
};
use crate::packages::traits::Scriptlet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInvocation {
    pub id: String,
    pub entry_id: String,
    pub source: CommandEvidenceSource,
    pub phase: Option<String>,
    pub lifecycle_paths: Vec<String>,
    pub interpreter: Option<String>,
    pub command: String,
    pub argv: Vec<String>,
    pub raw_line: Option<String>,
    pub cwd: Option<String>,
    pub environment: Vec<CommandEnvironmentFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEvidenceSource {
    StaticSignal,
    CaptureLog,
    NativeMetadata,
    PayloadHeuristic,
    CuratedRule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEnvironmentFact {
    pub name: String,
    pub value: Option<String>,
}

pub fn extract_scriptlet_invocations(
    entry_id: &str,
    scriptlet: &Scriptlet,
) -> Vec<CommandInvocation> {
    if !is_shell_interpreter(&scriptlet.interpreter) {
        return Vec::new();
    }

    extract_invocations_from_text(InvocationText {
        entry_id,
        content: &scriptlet.content,
        source: CommandEvidenceSource::StaticSignal,
        phase: Some(scriptlet.phase.to_string()),
        lifecycle_paths: vec![scriptlet.phase.to_string()],
        interpreter: Some(scriptlet.interpreter.clone()),
    })
}

pub fn extract_native_entry_invocations(entry: &NativeScriptletEntry) -> Vec<CommandInvocation> {
    if entry.kind != NativeScriptletKind::Executable {
        return Vec::new();
    }
    if entry.support != NativeScriptletSupport::Parsed {
        return Vec::new();
    }

    let Some(content) = entry.body.text.as_deref() else {
        return Vec::new();
    };

    if !entry
        .interpreter
        .as_deref()
        .is_none_or(is_shell_interpreter)
    {
        return Vec::new();
    }

    extract_invocations_from_text(InvocationText {
        entry_id: &entry.id,
        content,
        source: CommandEvidenceSource::NativeMetadata,
        phase: entry.compatibility_phase.map(|phase| phase.to_string()),
        lifecycle_paths: entry
            .lifecycle_paths
            .iter()
            .copied()
            .map(native_lifecycle_label)
            .map(str::to_string)
            .collect(),
        interpreter: entry.interpreter.clone(),
    })
}

pub fn extract_invocations_from_shell_text(
    entry_id: &str,
    content: &str,
    phase: Option<&str>,
) -> Vec<CommandInvocation> {
    extract_invocations_from_text(InvocationText {
        entry_id,
        content,
        source: CommandEvidenceSource::StaticSignal,
        phase: phase.map(str::to_string),
        lifecycle_paths: phase.map(str::to_string).into_iter().collect(),
        interpreter: Some("/bin/sh".to_string()),
    })
}

struct InvocationText<'a> {
    entry_id: &'a str,
    content: &'a str,
    source: CommandEvidenceSource,
    phase: Option<String>,
    lifecycle_paths: Vec<String>,
    interpreter: Option<String>,
}

fn extract_invocations_from_text(input: InvocationText<'_>) -> Vec<CommandInvocation> {
    let mut invocations = Vec::new();

    for (line_index, line) in input.content.lines().enumerate() {
        let raw_line = line.trim();
        if raw_line.is_empty() || raw_line.starts_with('#') {
            continue;
        }

        let mut command_index = 0;
        for segment in split_command_segments(raw_line) {
            let Some(invocation) =
                invocation_from_segment(&input, line_index, command_index, raw_line, &segment)
            else {
                continue;
            };
            invocations.push(invocation);
            command_index += 1;
        }
    }

    invocations
}

fn invocation_from_segment(
    input: &InvocationText<'_>,
    line_index: usize,
    command_index: usize,
    raw_line: &str,
    segment: &str,
) -> Option<CommandInvocation> {
    let tokens: Vec<&str> = segment.split_whitespace().collect();
    let mut environment = Vec::new();
    let mut index = 0;

    while let Some(token) = tokens.get(index).copied() {
        if let Some(fact) = environment_fact(token) {
            environment.push(fact);
            index += 1;
            continue;
        }
        if is_shell_keyword(token) {
            index += 1;
            continue;
        }
        break;
    }

    index = skip_wrappers(&tokens, index, &mut environment);

    let command_token = tokens.get(index)?;
    let command = normalize_command(command_token)?;
    let argv = tokens
        .iter()
        .skip(index + 1)
        .filter(|arg| !is_redirect(arg))
        .map(|arg| clean_token(arg))
        .filter(|arg| !arg.is_empty())
        .collect();

    Some(CommandInvocation {
        id: format!("{}:line{line_index}:cmd{command_index}", input.entry_id),
        entry_id: input.entry_id.to_string(),
        source: input.source,
        phase: input.phase.clone(),
        lifecycle_paths: input.lifecycle_paths.clone(),
        interpreter: input.interpreter.clone(),
        command,
        argv,
        raw_line: Some(raw_line.to_string()),
        cwd: None,
        environment,
    })
}

fn split_command_segments(line: &str) -> Vec<String> {
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
                push_segment(&mut segments, &mut current);
            }
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                push_segment(&mut segments, &mut current);
            }
            ';' | '(' | ')' | '`' => push_segment(&mut segments, &mut current),
            '$' if chars.peek() == Some(&'(') => {
                chars.next();
                push_segment(&mut segments, &mut current);
            }
            _ => current.push(ch),
        }
    }

    push_segment(&mut segments, &mut current);
    segments
}

fn push_segment(segments: &mut Vec<String>, current: &mut String) {
    let segment = current.trim();
    if !segment.is_empty() {
        segments.push(segment.to_string());
    }
    current.clear();
}

fn skip_wrappers(
    tokens: &[&str],
    mut index: usize,
    environment: &mut Vec<CommandEnvironmentFact>,
) -> usize {
    while let Some(token) = tokens.get(index).map(|token| clean_token(token)) {
        match token.as_str() {
            "chroot" => {
                index += 1;
                while tokens
                    .get(index)
                    .is_some_and(|token| token.starts_with('-'))
                {
                    index += 1;
                }
                if index < tokens.len() {
                    index += 1;
                }
            }
            "sudo" => {
                index += 1;
                while let Some(flag) = tokens.get(index).map(|token| clean_token(token)) {
                    if !flag.starts_with('-') {
                        break;
                    }
                    index += 1;
                    if sudo_flag_takes_arg(&flag) && index < tokens.len() {
                        index += 1;
                    }
                }
            }
            "env" => {
                index += 1;
                while let Some(token) = tokens.get(index).copied() {
                    if token.starts_with('-') {
                        index += 1;
                        continue;
                    }
                    if let Some(fact) = environment_fact(token) {
                        environment.push(fact);
                        index += 1;
                        continue;
                    }
                    break;
                }
            }
            _ => break,
        }
    }

    index
}

fn sudo_flag_takes_arg(flag: &str) -> bool {
    matches!(
        flag,
        "-u" | "-g" | "-h" | "-p" | "--user" | "--group" | "--host" | "--prompt"
    )
}

fn environment_fact(token: &str) -> Option<CommandEnvironmentFact> {
    let (name, value) = token.split_once('=')?;
    if name.is_empty() || name.starts_with('/') || !is_env_name(name) {
        return None;
    }
    Some(CommandEnvironmentFact {
        name: name.to_string(),
        value: Some(clean_token(value)),
    })
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn normalize_command(token: &str) -> Option<String> {
    let cleaned = clean_token(token);
    let command = cleaned.rsplit('/').next().unwrap_or(cleaned.as_str());
    if command.is_empty() || command.starts_with('-') || command.contains('=') {
        return None;
    }
    Some(command.to_string())
}

fn clean_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '[' | ']'))
        .to_string()
}

fn is_redirect(token: &str) -> bool {
    token.starts_with('>') || token.starts_with("2>")
}

fn is_shell_keyword(token: &str) -> bool {
    matches!(
        token,
        "if" | "then" | "else" | "elif" | "fi" | "do" | "done" | "while" | "for" | "case" | "esac"
    )
}

fn is_shell_interpreter(interpreter: &str) -> bool {
    let interpreter = interpreter.split_whitespace().next().unwrap_or(interpreter);
    let command = interpreter.rsplit('/').next().unwrap_or(interpreter);
    matches!(command, "sh" | "bash" | "dash" | "ksh" | "zsh")
}

fn native_lifecycle_label(path: NativeLifecyclePath) -> &'static str {
    match path {
        NativeLifecyclePath::PreInstall => "pre-install",
        NativeLifecyclePath::PostInstall => "post-install",
        NativeLifecyclePath::PreUpgrade => "pre-upgrade",
        NativeLifecyclePath::PostUpgrade => "post-upgrade",
        NativeLifecyclePath::PreRemove => "pre-remove",
        NativeLifecyclePath::PostRemove => "post-remove",
        NativeLifecyclePath::PreTransaction => "pre-transaction",
        NativeLifecyclePath::PostTransaction => "post-transaction",
        NativeLifecyclePath::PreUntransaction => "pre-untransaction",
        NativeLifecyclePath::PostUntransaction => "post-untransaction",
        NativeLifecyclePath::Verify => "verify",
        NativeLifecyclePath::Config => "config",
        NativeLifecyclePath::Trigger => "trigger",
        NativeLifecyclePath::FileTrigger => "file-trigger",
        NativeLifecyclePath::TransactionFileTrigger => "transaction-file-trigger",
        NativeLifecyclePath::Purge => "purge",
        NativeLifecyclePath::Abort => "abort",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::native_abi::*;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};

    fn scriptlet(content: &str) -> Scriptlet {
        Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: content.to_string(),
            flags: None,
        }
    }

    fn native_entry(content: &str, support: NativeScriptletSupport) -> NativeScriptletEntry {
        NativeScriptletEntry {
            id: "rpm:%post".to_string(),
            format: NativeScriptletFormat::Rpm,
            kind: NativeScriptletKind::Executable,
            native_slot: "%post".to_string(),
            primary_lifecycle: NativeLifecyclePath::PostInstall,
            compatibility_phase: Some(ScriptletPhase::PostInstall),
            lifecycle_paths: vec![NativeLifecyclePath::PostInstall],
            interpreter: Some("/bin/sh".to_string()),
            interpreter_args: vec![],
            body: NativeScriptletBody::from_bytes(content.as_bytes().to_vec()),
            invocation: NativeInvocationContract::none(),
            order: NativeTransactionOrder::new(NativeTransactionPosition::AfterPayload),
            support,
            metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
                slot: RpmScriptletSlot::Post,
                scriptlet_flags: None,
                trigger: None,
            }),
        }
    }

    #[test]
    fn command_evidence_splits_control_operators_with_stable_ids() {
        let invocations = extract_scriptlet_invocations(
            "rpm:%post",
            &scriptlet("VAR=1 /usr/bin/systemctl daemon-reload && /sbin/ldconfig\n"),
        );

        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].id, "rpm:%post:line0:cmd0");
        assert_eq!(invocations[0].entry_id, "rpm:%post");
        assert_eq!(invocations[0].source, CommandEvidenceSource::StaticSignal);
        assert_eq!(invocations[0].phase.as_deref(), Some("post-install"));
        assert_eq!(invocations[0].lifecycle_paths, vec!["post-install"]);
        assert_eq!(invocations[0].interpreter.as_deref(), Some("/bin/sh"));
        assert_eq!(
            invocations[0].environment,
            vec![CommandEnvironmentFact {
                name: "VAR".to_string(),
                value: Some("1".to_string()),
            }]
        );
        assert_eq!(invocations[0].command, "systemctl");
        assert_eq!(invocations[0].argv, vec!["daemon-reload"]);
        assert_eq!(invocations[1].id, "rpm:%post:line0:cmd1");
        assert_eq!(invocations[1].command, "ldconfig");
    }

    #[test]
    fn command_evidence_skips_wrappers_and_preserves_raw_line() {
        let invocations = extract_scriptlet_invocations(
            "deb:postinst",
            &scriptlet("env -i chroot /target /usr/bin/update-mime-database /usr/share/mime\n"),
        );

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].command, "update-mime-database");
        assert_eq!(invocations[0].argv, vec!["/usr/share/mime"]);
        assert_eq!(
            invocations[0].raw_line.as_deref(),
            Some("env -i chroot /target /usr/bin/update-mime-database /usr/share/mime")
        );
    }

    #[test]
    fn command_evidence_skips_wrapper_positional_arguments() {
        let invocations = extract_scriptlet_invocations(
            "rpm:%post",
            &scriptlet("sudo -u nobody chroot /target /sbin/ldconfig\n"),
        );

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].command, "ldconfig");
        assert!(invocations[0].argv.is_empty());
    }

    #[test]
    fn command_evidence_ignores_non_shell_interpreters() {
        let mut perl = scriptlet("#!/usr/bin/perl\nprint 'ok';\n");
        perl.interpreter = "/usr/bin/perl".to_string();

        assert!(extract_scriptlet_invocations("deb:config", &perl).is_empty());
    }

    #[test]
    fn native_command_evidence_preserves_native_metadata_source() {
        let invocations = extract_native_entry_invocations(&native_entry(
            "/sbin/ldconfig\n",
            NativeScriptletSupport::Parsed,
        ));

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].id, "rpm:%post:line0:cmd0");
        assert_eq!(invocations[0].entry_id, "rpm:%post");
        assert_eq!(invocations[0].source, CommandEvidenceSource::NativeMetadata);
        assert_eq!(invocations[0].phase.as_deref(), Some("post-install"));
        assert_eq!(invocations[0].lifecycle_paths, vec!["post-install"]);
        assert_eq!(invocations[0].command, "ldconfig");

        assert!(
            extract_native_entry_invocations(&native_entry(
                "/sbin/ldconfig\n",
                NativeScriptletSupport::DeferredReview {
                    reason_code: "rpm-trigger-semantics-deferred".to_string(),
                },
            ))
            .is_empty()
        );
    }

    #[test]
    fn command_evidence_extracts_invocations_from_shell_text() {
        let invocations = extract_invocations_from_shell_text(
            "recipe:make",
            "npm install atomic-lockfile && bun add js-digest",
            Some("make"),
        );

        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].entry_id, "recipe:make");
        assert_eq!(invocations[0].source, CommandEvidenceSource::StaticSignal);
        assert_eq!(invocations[0].phase.as_deref(), Some("make"));
        assert_eq!(invocations[0].lifecycle_paths, vec!["make"]);
        assert_eq!(invocations[0].interpreter.as_deref(), Some("/bin/sh"));
        assert_eq!(invocations[0].command, "npm");
        assert_eq!(invocations[0].argv, vec!["install", "atomic-lockfile"]);
        assert_eq!(invocations[1].command, "bun");
        assert_eq!(invocations[1].argv, vec!["add", "js-digest"]);
    }
}
