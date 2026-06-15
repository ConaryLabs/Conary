// conary-core/src/security/command_risk.rs
//! Shared command-risk taxonomy for build, conversion, and runtime scriptlet evidence.

use serde::{Deserialize, Serialize};

use crate::ccs::convert::command_evidence::CommandInvocation;
use crate::ccs::convert::command_evidence::extract_invocations_from_shell_text;

pub const COMMAND_RISK_CLASSIFIER_VERSION: &str = "m2-command-risk-v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CommandRiskReasonCode {
    PackageManagerFetch,
    NetworkFetch,
    DynamicLanguageExec,
    CredentialPath,
    Obfuscation,
    Persistence,
    BpfOrEbpf,
    ProcStealthOrDebug,
}

impl CommandRiskReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PackageManagerFetch => "package-manager-fetch",
            Self::NetworkFetch => "network-fetch",
            Self::DynamicLanguageExec => "dynamic-language-exec",
            Self::CredentialPath => "credential-path",
            Self::Obfuscation => "obfuscation",
            Self::Persistence => "persistence",
            Self::BpfOrEbpf => "bpf-or-ebpf",
            Self::ProcStealthOrDebug => "proc-stealth-or-debug",
        }
    }
}

pub const PACKAGE_MANAGER_FETCH: &str = CommandRiskReasonCode::PackageManagerFetch.as_str();
pub const NETWORK_FETCH: &str = CommandRiskReasonCode::NetworkFetch.as_str();
pub const DYNAMIC_LANGUAGE_EXEC: &str = CommandRiskReasonCode::DynamicLanguageExec.as_str();
pub const CREDENTIAL_PATH: &str = CommandRiskReasonCode::CredentialPath.as_str();
pub const OBFUSCATION: &str = CommandRiskReasonCode::Obfuscation.as_str();
pub const PERSISTENCE: &str = CommandRiskReasonCode::Persistence.as_str();
pub const BPF_OR_EBPF: &str = CommandRiskReasonCode::BpfOrEbpf.as_str();
pub const PROC_STEALTH_OR_DEBUG: &str = CommandRiskReasonCode::ProcStealthOrDebug.as_str();

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CommandRiskStatus {
    Clean,
    Review,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandRiskReport {
    pub status: CommandRiskStatus,
    pub classifier_version: String,
    pub entries: Vec<CommandRiskEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandRiskEntry {
    pub source: String,
    pub command: String,
    pub reason_code: String,
    pub severity: CommandRiskStatus,
    pub evidence: String,
}

impl CommandRiskReport {
    pub fn clean() -> Self {
        Self {
            status: CommandRiskStatus::Clean,
            classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries: Vec::new(),
        }
    }

    pub fn requires_runtime_sandbox(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(
                entry.reason_code.as_str(),
                PACKAGE_MANAGER_FETCH | NETWORK_FETCH | DYNAMIC_LANGUAGE_EXEC
            )
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct RiskSignal {
    command: &'static str,
    reason_code: &'static str,
}

pub fn classify_shell_text(source: &str, content: &str) -> CommandRiskReport {
    let mut entries = Vec::new();
    let invocations = extract_invocations_from_shell_text(source, content, Some(source));

    for invocation in &invocations {
        if let Some(signal) = signal_for_invocation(invocation) {
            push_entry(
                &mut entries,
                source,
                &invocation.command,
                signal.reason_code,
                invocation.raw_line.as_deref().unwrap_or(content.trim()),
            );
        }
    }

    for line in content.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for signal in raw_line_signals(line) {
            push_raw_entry(&mut entries, source, signal, line);
        }
    }

    if entries.is_empty() {
        CommandRiskReport::clean()
    } else {
        CommandRiskReport {
            status: CommandRiskStatus::Blocked,
            classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries,
        }
    }
}

fn signal_for_invocation(invocation: &CommandInvocation) -> Option<RiskSignal> {
    let command = invocation.command.as_str();
    let argv: Vec<&str> = invocation.argv.iter().map(String::as_str).collect();

    if is_package_manager_fetch(command, &argv) {
        return Some(RiskSignal {
            command: "package-manager",
            reason_code: PACKAGE_MANAGER_FETCH,
        });
    }

    if is_network_fetch(command, &argv) {
        return Some(RiskSignal {
            command: "network-fetch",
            reason_code: NETWORK_FETCH,
        });
    }

    if is_dynamic_language_exec(command, &argv) {
        return Some(RiskSignal {
            command: "dynamic-language",
            reason_code: DYNAMIC_LANGUAGE_EXEC,
        });
    }

    if is_obfuscation(command, &argv) {
        return Some(RiskSignal {
            command: "obfuscation",
            reason_code: OBFUSCATION,
        });
    }

    if is_persistence(command, &argv) {
        return Some(RiskSignal {
            command: "persistence",
            reason_code: PERSISTENCE,
        });
    }

    if is_bpf_or_ebpf(command) {
        return Some(RiskSignal {
            command: "bpf",
            reason_code: BPF_OR_EBPF,
        });
    }

    if is_debug_or_stealth_command(command) {
        return Some(RiskSignal {
            command: "proc-debug",
            reason_code: PROC_STEALTH_OR_DEBUG,
        });
    }

    None
}

fn raw_line_signals(line: &str) -> Vec<RiskSignal> {
    let lower = line.to_ascii_lowercase();
    let mut signals = Vec::new();

    if raw_package_manager_fetch(&lower) {
        signals.push(RiskSignal {
            command: "package-manager",
            reason_code: PACKAGE_MANAGER_FETCH,
        });
    }
    if raw_network_fetch(&lower) {
        signals.push(RiskSignal {
            command: "network-fetch",
            reason_code: NETWORK_FETCH,
        });
    }
    if raw_dynamic_language_exec(&lower) {
        signals.push(RiskSignal {
            command: "dynamic-language",
            reason_code: DYNAMIC_LANGUAGE_EXEC,
        });
    }
    if raw_credential_path(&lower) {
        signals.push(RiskSignal {
            command: "credential-path",
            reason_code: CREDENTIAL_PATH,
        });
    }
    if raw_obfuscation(&lower) {
        signals.push(RiskSignal {
            command: "obfuscation",
            reason_code: OBFUSCATION,
        });
    }
    if raw_persistence(&lower) {
        signals.push(RiskSignal {
            command: "persistence",
            reason_code: PERSISTENCE,
        });
    }
    if raw_bpf_or_ebpf(&lower) {
        signals.push(RiskSignal {
            command: "bpf",
            reason_code: BPF_OR_EBPF,
        });
    }
    if raw_proc_stealth_or_debug(&lower) {
        signals.push(RiskSignal {
            command: "proc-debug",
            reason_code: PROC_STEALTH_OR_DEBUG,
        });
    }

    signals
}

fn is_package_manager_fetch(command: &str, argv: &[&str]) -> bool {
    matches!(
        command,
        "npm" | "npx" | "pnpm" | "yarn" | "bun" | "pip" | "pip3" | "gem"
    ) || matches!(command, "cargo" | "go") && argv.first().is_some_and(|arg| *arg == "install")
}

fn is_network_fetch(command: &str, argv: &[&str]) -> bool {
    matches!(command, "curl" | "wget" | "aria2c" | "fetch")
        || command == "git" && argv.first().is_some_and(|arg| *arg == "clone")
}

fn is_dynamic_language_exec(command: &str, argv: &[&str]) -> bool {
    command == "node" && argv_contains(argv, "-e")
        || command.starts_with("python") && argv_contains(argv, "-c")
        || matches!(command, "perl" | "ruby") && argv_contains(argv, "-e")
}

fn is_obfuscation(command: &str, argv: &[&str]) -> bool {
    command == "eval"
        || command == "base64" && (argv_contains(argv, "-d") || argv_contains(argv, "--decode"))
}

fn is_persistence(command: &str, argv: &[&str]) -> bool {
    command == "crontab" || command == "systemctl" && argv_contains(argv, "enable")
}

fn is_bpf_or_ebpf(command: &str) -> bool {
    matches!(command, "bpf" | "bpftool") || command.contains("bpf")
}

fn is_debug_or_stealth_command(command: &str) -> bool {
    matches!(command, "ptrace" | "strace" | "gdb")
}

fn argv_contains(argv: &[&str], needle: &str) -> bool {
    argv.contains(&needle)
}

fn raw_package_manager_fetch(line: &str) -> bool {
    ["npm", "npx", "pnpm", "yarn", "bun", "pip", "pip3", "gem"]
        .iter()
        .any(|command| contains_shell_word(line, command))
        || contains_shell_words(line, "cargo", "install")
        || contains_shell_words(line, "go", "install")
}

fn raw_network_fetch(line: &str) -> bool {
    ["curl", "wget", "aria2c", "fetch"]
        .iter()
        .any(|command| contains_shell_word(line, command))
        || contains_shell_words(line, "git", "clone")
}

fn raw_dynamic_language_exec(line: &str) -> bool {
    contains_shell_words(line, "node", "-e")
        || contains_shell_words(line, "python", "-c")
        || contains_shell_words(line, "python3", "-c")
        || contains_shell_words(line, "perl", "-e")
        || contains_shell_words(line, "ruby", "-e")
}

fn raw_credential_path(line: &str) -> bool {
    line.contains("/etc/shadow")
        || line.contains("/etc/sudoers")
        || line.contains("authorized_keys")
        || line.contains(".npmrc")
        || line.contains(".pypirc")
        || line.contains(".cargo/credentials")
        || line.contains("ssh/id_")
        || line.contains("token")
}

fn raw_obfuscation(line: &str) -> bool {
    contains_shell_word(line, "eval")
        || contains_shell_words(line, "base64", "-d")
        || contains_shell_words(line, "base64", "--decode")
}

fn raw_persistence(line: &str) -> bool {
    contains_shell_word(line, "crontab")
        || contains_shell_words(line, "systemctl", "enable")
        || line.contains(".config/systemd/user")
            && (line.contains('>') || contains_shell_word(line, "tee"))
}

fn raw_bpf_or_ebpf(line: &str) -> bool {
    ["bpf", "bpftool", "libbpf", "perf_event_open", "ebpf"]
        .iter()
        .any(|token| contains_shell_word(line, token))
}

fn raw_proc_stealth_or_debug(line: &str) -> bool {
    contains_shell_word(line, "ptrace")
        || contains_shell_word(line, "strace")
        || contains_shell_word(line, "gdb")
        || line.contains("/proc/") && (line.contains("/mem") || line.contains("/environ"))
}

fn push_entry(
    entries: &mut Vec<CommandRiskEntry>,
    source: &str,
    command: &str,
    reason_code: &str,
    evidence: &str,
) {
    if entries.iter().any(|entry| {
        entry.source == source
            && entry.command == command
            && entry.reason_code == reason_code
            && entry.evidence == evidence
    }) {
        return;
    }

    entries.push(CommandRiskEntry {
        source: source.to_string(),
        command: command.to_string(),
        reason_code: reason_code.to_string(),
        severity: CommandRiskStatus::Blocked,
        evidence: evidence.to_string(),
    });
}

fn push_raw_entry(
    entries: &mut Vec<CommandRiskEntry>,
    source: &str,
    signal: RiskSignal,
    evidence: &str,
) {
    if entries.iter().any(|entry| {
        entry.source == source
            && entry.reason_code == signal.reason_code
            && entry.evidence == evidence
    }) {
        return;
    }

    push_entry(
        entries,
        source,
        signal.command,
        signal.reason_code,
        evidence,
    );
}

fn contains_shell_words(line: &str, first: &str, second: &str) -> bool {
    let mut first_search_start = 0;
    while let Some(relative_first_index) = find_shell_word(&line[first_search_start..], first) {
        let first_index = first_search_start + relative_first_index;
        let second_search_start = first_index + first.len();
        let Some(relative_second_index) = find_shell_word(&line[second_search_start..], second)
        else {
            return false;
        };
        let second_index = second_search_start + relative_second_index;

        if !contains_shell_command_separator(&line[second_search_start..second_index]) {
            return true;
        }

        first_search_start = second_search_start;
    }

    false
}

fn contains_shell_word(line: &str, word: &str) -> bool {
    find_shell_word(line, word).is_some()
}

fn find_shell_word(line: &str, word: &str) -> Option<usize> {
    line.match_indices(word).find_map(|(index, _)| {
        let before = line[..index].chars().next_back();
        let after = line[index + word.len()..].chars().next();
        if is_shell_word_boundary(before) && is_shell_word_boundary(after) {
            Some(index)
        } else {
            None
        }
    })
}

fn is_shell_word_boundary(ch: Option<char>) -> bool {
    ch.is_none_or(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')))
}

fn contains_shell_command_separator(text: &str) -> bool {
    let mut chars = text.chars().peekable();
    let mut previous = None;
    while let Some(ch) = chars.next() {
        match ch {
            '&' if chars.peek() == Some(&'&') => return true,
            '&' if previous.is_some_and(|ch| matches!(ch, '>' | '<'))
                || chars.peek().is_some_and(|ch| matches!(ch, '>' | '<')) => {}
            '&' => return true,
            '|' | ';' | '(' | ')' | '`' | '\n' => return true,
            '$' if chars.peek() == Some(&'(') => return true,
            _ => {}
        }
        previous = Some(ch);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aur_style_package_manager_commands_share_reason_codes() {
        let report = classify_shell_text(
            "pkgbuild:prepare",
            "npm install atomic-lockfile\nbun add js-digest",
        );

        assert_eq!(report.status, CommandRiskStatus::Blocked);
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.reason_code == PACKAGE_MANAGER_FETCH)
        );
    }

    #[test]
    fn dynamic_exec_and_bpf_share_reason_codes() {
        let report = classify_shell_text(
            "scriptlet:postinstall",
            "python -c 'print(1)'\nbpftool prog show",
        );

        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.reason_code == DYNAMIC_LANGUAGE_EXEC)
        );
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.reason_code == BPF_OR_EBPF)
        );
    }

    #[test]
    fn runtime_auto_sandbox_maps_shared_medium_signals() {
        let report = classify_shell_text("scriptlet:install", "node -e 'console.log(1)'");

        assert!(report.requires_runtime_sandbox());
    }
}
