// conary-core/src/security/command_risk.rs
//! Shared diagnostic command-observation taxonomy.
//!
//! The classifier parses shell structure and records observations for audit and
//! engineering prioritization. Its severity is never execution, mutation,
//! compatibility, trust, or publication authority.

use serde::{Deserialize, Serialize};

use crate::ccs::convert::command_evidence::{
    CommandInvocation, extract_invocations_from_shell_text,
};
use crate::ccs::native_lifecycle::CommandArgumentProvenance;

pub const COMMAND_RISK_CLASSIFIER_VERSION: &str = "command-diagnostics-v3";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CommandRiskReasonCode {
    ShellParseFailure,
    CommandFormUnresolved,
    RootDeletion,
    FilesystemFormat,
    DeviceWrite,
    SetuidMutation,
    RemoteShellExec,
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
            Self::ShellParseFailure => "shell-parse-failure",
            Self::CommandFormUnresolved => "command-form-unresolved",
            Self::RootDeletion => "root-deletion",
            Self::FilesystemFormat => "filesystem-format",
            Self::DeviceWrite => "device-write",
            Self::SetuidMutation => "setuid-mutation",
            Self::RemoteShellExec => "remote-shell-exec",
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
pub const SHELL_PARSE_FAILURE: &str = CommandRiskReasonCode::ShellParseFailure.as_str();
pub const COMMAND_FORM_UNRESOLVED: &str = CommandRiskReasonCode::CommandFormUnresolved.as_str();
pub const ROOT_DELETION: &str = CommandRiskReasonCode::RootDeletion.as_str();
pub const FILESYSTEM_FORMAT: &str = CommandRiskReasonCode::FilesystemFormat.as_str();
pub const DEVICE_WRITE: &str = CommandRiskReasonCode::DeviceWrite.as_str();
pub const SETUID_MUTATION: &str = CommandRiskReasonCode::SetuidMutation.as_str();
pub const REMOTE_SHELL_EXEC: &str = CommandRiskReasonCode::RemoteShellExec.as_str();
pub const NETWORK_FETCH: &str = CommandRiskReasonCode::NetworkFetch.as_str();
pub const DYNAMIC_LANGUAGE_EXEC: &str = CommandRiskReasonCode::DynamicLanguageExec.as_str();
pub const CREDENTIAL_PATH: &str = CommandRiskReasonCode::CredentialPath.as_str();
pub const OBFUSCATION: &str = CommandRiskReasonCode::Obfuscation.as_str();
pub const PERSISTENCE: &str = CommandRiskReasonCode::Persistence.as_str();
pub const BPF_OR_EBPF: &str = CommandRiskReasonCode::BpfOrEbpf.as_str();
pub const PROC_STEALTH_OR_DEBUG: &str = CommandRiskReasonCode::ProcStealthOrDebug.as_str();

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum CommandRiskSeverity {
    None,
    Notice,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandRiskReport {
    pub highest_severity: CommandRiskSeverity,
    pub classifier_version: String,
    pub entries: Vec<CommandRiskEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandRiskEntry {
    pub source: String,
    pub command: String,
    pub reason_code: String,
    pub severity: CommandRiskSeverity,
    pub evidence: String,
}

impl CommandRiskReport {
    pub fn no_findings() -> Self {
        Self {
            highest_severity: CommandRiskSeverity::None,
            classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RiskSignal {
    reason_code: &'static str,
}

pub fn classify_shell_text(source: &str, content: &str) -> CommandRiskReport {
    let mut entries = Vec::new();
    let invocations = match extract_invocations_from_shell_text(source, content, Some(source)) {
        Ok(invocations) => invocations,
        Err(error) => {
            push_entry(
                &mut entries,
                source,
                "shell-parser",
                SHELL_PARSE_FAILURE,
                &error.to_string(),
            );
            Vec::new()
        }
    };

    collect_pipeline_risk(source, &invocations, &mut entries);
    for invocation in &invocations {
        collect_invocation_risk(source, invocation, content, &mut entries, 0);
    }

    if entries.is_empty() {
        CommandRiskReport::no_findings()
    } else {
        CommandRiskReport {
            highest_severity: entries
                .iter()
                .map(|entry| entry.severity)
                .max()
                .unwrap_or(CommandRiskSeverity::None),
            classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries,
        }
    }
}

fn collect_pipeline_risk(
    source: &str,
    invocations: &[CommandInvocation],
    entries: &mut Vec<CommandRiskEntry>,
) {
    for (fetch_index, fetch) in invocations.iter().enumerate() {
        let fetch_argv: Vec<_> = fetch.argv.iter().map(String::as_str).collect();
        if fetch.command_provenance != CommandArgumentProvenance::Literal
            || fetch.pipeline_id.is_none()
            || !is_network_fetch(&fetch.command, &fetch_argv)
        {
            continue;
        }
        let shell = invocations.iter().skip(fetch_index + 1).find(|candidate| {
            candidate.pipeline_id == fetch.pipeline_id
                && candidate.command_provenance == CommandArgumentProvenance::Literal
                && matches!(
                    candidate.command.as_str(),
                    "sh" | "bash" | "dash" | "ksh" | "zsh"
                )
                && candidate.argv.is_empty()
        });
        if let Some(shell) = shell {
            push_entry(
                entries,
                source,
                &shell.command,
                REMOTE_SHELL_EXEC,
                shell.raw_line.as_deref().unwrap_or("pipeline shell"),
            );
        }
    }
}

fn collect_invocation_risk(
    source: &str,
    invocation: &CommandInvocation,
    content: &str,
    entries: &mut Vec<CommandRiskEntry>,
    depth: usize,
) {
    const MAX_WRAPPER_DEPTH: usize = 8;
    if depth >= MAX_WRAPPER_DEPTH {
        push_entry(
            entries,
            source,
            &invocation.command,
            SHELL_PARSE_FAILURE,
            "formal wrapper nesting exceeded the supported depth",
        );
        return;
    }

    if invocation.command_provenance != CommandArgumentProvenance::Literal {
        push_entry(
            entries,
            source,
            &invocation.command,
            COMMAND_FORM_UNRESOLVED,
            "command name is not a literal AST word",
        );
        return;
    }

    for signal in signals_for_invocation(invocation) {
        push_entry(
            entries,
            source,
            &invocation.command,
            signal.reason_code,
            invocation.raw_line.as_deref().unwrap_or(content.trim()),
        );
    }

    match env_wrapped_invocation(invocation) {
        Ok(Some(wrapped)) => {
            collect_invocation_risk(source, &wrapped, content, entries, depth + 1);
        }
        Ok(None) => {}
        Err(reason) => push_entry(
            entries,
            source,
            &invocation.command,
            COMMAND_FORM_UNRESOLVED,
            reason,
        ),
    }

    let script = match literal_shell_program(invocation) {
        Ok(Some(script)) => script,
        Ok(None) => return,
        Err(reason) => {
            push_entry(
                entries,
                source,
                &invocation.command,
                COMMAND_FORM_UNRESOLVED,
                reason,
            );
            return;
        }
    };
    match extract_invocations_from_shell_text(source, script, invocation.phase.as_deref()) {
        Ok(nested) => {
            for nested_invocation in &nested {
                collect_invocation_risk(source, nested_invocation, script, entries, depth + 1);
            }
        }
        Err(error) => push_entry(
            entries,
            source,
            "shell-parser",
            SHELL_PARSE_FAILURE,
            &error.to_string(),
        ),
    }
}

fn signals_for_invocation(invocation: &CommandInvocation) -> Vec<RiskSignal> {
    let command = invocation.command.as_str();
    let argv: Vec<&str> = invocation.argv.iter().map(String::as_str).collect();
    let mut signals = Vec::new();

    if is_root_deletion(invocation) {
        signals.push(RiskSignal {
            reason_code: ROOT_DELETION,
        });
    }

    if is_filesystem_format(command) {
        signals.push(RiskSignal {
            reason_code: FILESYSTEM_FORMAT,
        });
    }

    if is_device_write(invocation) {
        signals.push(RiskSignal {
            reason_code: DEVICE_WRITE,
        });
    }

    if is_setid_chmod(invocation) {
        signals.push(RiskSignal {
            reason_code: SETUID_MUTATION,
        });
    }

    if is_package_manager_fetch(command, &argv) {
        signals.push(RiskSignal {
            reason_code: PACKAGE_MANAGER_FETCH,
        });
    }

    if is_network_fetch(command, &argv) {
        signals.push(RiskSignal {
            reason_code: NETWORK_FETCH,
        });
    }

    if is_dynamic_language_exec(command, &argv) {
        signals.push(RiskSignal {
            reason_code: DYNAMIC_LANGUAGE_EXEC,
        });
    }

    if is_obfuscation(command, &argv) {
        signals.push(RiskSignal {
            reason_code: OBFUSCATION,
        });
    }

    if is_persistence(command, &argv) {
        signals.push(RiskSignal {
            reason_code: PERSISTENCE,
        });
    }

    if is_bpf_or_ebpf(command) {
        signals.push(RiskSignal {
            reason_code: BPF_OR_EBPF,
        });
    }

    if argv.iter().any(|arg| is_credential_path(arg)) {
        signals.push(RiskSignal {
            reason_code: CREDENTIAL_PATH,
        });
    }

    if is_debug_or_stealth_command(command) || argv.iter().any(|arg| is_proc_stealth_path(arg)) {
        signals.push(RiskSignal {
            reason_code: PROC_STEALTH_OR_DEBUG,
        });
    }

    signals
}

fn is_root_deletion(invocation: &CommandInvocation) -> bool {
    if invocation.command != "rm" {
        return false;
    }
    let recursive = invocation.argv.iter().any(|arg| {
        arg == "--recursive" || short_option_contains(arg, 'r') || short_option_contains(arg, 'R')
    });
    let force = invocation
        .argv
        .iter()
        .any(|arg| arg == "--force" || short_option_contains(arg, 'f'));
    recursive
        && force
        && invocation.argv.iter().enumerate().any(|(index, arg)| {
            invocation.argument_provenance.get(index) == Some(&CommandArgumentProvenance::Literal)
                && arg == "/"
                || invocation.argument_provenance.get(index)
                    == Some(&CommandArgumentProvenance::Glob)
                    && arg == "/*"
        })
}

fn is_filesystem_format(command: &str) -> bool {
    command == "mkfs"
        || command
            .strip_prefix("mkfs.")
            .is_some_and(|suffix| !suffix.is_empty())
}

fn is_device_write(invocation: &CommandInvocation) -> bool {
    invocation.command == "dd"
        && invocation.argv.iter().enumerate().any(|(index, arg)| {
            invocation.argument_provenance.get(index) == Some(&CommandArgumentProvenance::Literal)
                && arg
                    .strip_prefix("of=")
                    .is_some_and(|path| path.starts_with("/dev/"))
        })
}

pub(crate) fn is_setid_chmod(invocation: &CommandInvocation) -> bool {
    if invocation.command != "chmod" {
        return false;
    }
    invocation.argv.iter().enumerate().any(|(index, arg)| {
        invocation.argument_provenance.get(index) == Some(&CommandArgumentProvenance::Literal)
            && (is_setid_octal_mode(arg) || is_setid_symbolic_mode(arg))
    })
}

fn is_setid_octal_mode(value: &str) -> bool {
    let value = value.strip_prefix("0o").unwrap_or(value);
    !value.is_empty()
        && value.chars().all(|ch| matches!(ch, '0'..='7'))
        && u32::from_str_radix(value, 8).is_ok_and(|mode| mode & 0o6000 != 0)
}

fn is_setid_symbolic_mode(value: &str) -> bool {
    value.split(',').any(|clause| {
        let Some(operator_index) = clause.find(['+', '-', '=']) else {
            return false;
        };
        let (who, operation) = clause.split_at(operator_index);
        let mut operation_chars = operation.chars();
        let Some(operator) = operation_chars.next() else {
            return false;
        };
        let permissions = operation_chars.as_str();
        who.chars().all(|ch| matches!(ch, 'u' | 'g' | 'o' | 'a'))
            && (who.is_empty() || who.chars().any(|ch| matches!(ch, 'u' | 'g' | 'a')))
            && matches!(operator, '+' | '-' | '=')
            && !permissions.is_empty()
            && permissions
                .chars()
                .all(|ch| matches!(ch, 'r' | 'w' | 'x' | 'X' | 's' | 't' | 'u' | 'g' | 'o'))
            && permissions.contains('s')
    })
}

fn short_option_contains(value: &str, needle: char) -> bool {
    value.starts_with('-')
        && !value.starts_with("--")
        && value[1..].chars().any(|flag| flag == needle)
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
        || is_python_interpreter(command) && argv_contains(argv, "-c")
        || matches!(command, "perl" | "ruby") && argv_contains(argv, "-e")
}

fn is_python_interpreter(command: &str) -> bool {
    let Some(version) = command.strip_prefix("python") else {
        return false;
    };
    if version.is_empty() {
        return true;
    }
    let mut components = version.split('.');
    components
        .next()
        .is_some_and(|major| !major.is_empty() && major.chars().all(|ch| ch.is_ascii_digit()))
        && components.all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
}

fn is_obfuscation(command: &str, argv: &[&str]) -> bool {
    command == "eval"
        || command == "base64" && (argv_contains(argv, "-d") || argv_contains(argv, "--decode"))
}

fn is_persistence(command: &str, argv: &[&str]) -> bool {
    command == "crontab"
        || command == "systemctl" && argv_contains(argv, "enable")
        || argv
            .iter()
            .any(|arg| path_has_component_sequence(arg, &[".config", "systemd", "user"]))
}

fn is_bpf_or_ebpf(command: &str) -> bool {
    matches!(command, "bpf" | "bpftool" | "bpftrace")
}

fn is_debug_or_stealth_command(command: &str) -> bool {
    matches!(command, "ptrace" | "strace" | "gdb")
}

fn argv_contains(argv: &[&str], needle: &str) -> bool {
    argv.contains(&needle)
}

fn is_credential_path(value: &str) -> bool {
    value == "/etc/shadow"
        || value == "/etc/sudoers"
        || value.ends_with("/authorized_keys")
        || value.ends_with("/.npmrc")
        || value.ends_with("/.pypirc")
        || value.ends_with("/.cargo/credentials")
        || is_ssh_private_key_path(value)
}

fn is_proc_stealth_path(value: &str) -> bool {
    value.starts_with("/proc/") && (value.ends_with("/mem") || value.ends_with("/environ"))
}

fn is_ssh_private_key_path(value: &str) -> bool {
    let path = std::path::Path::new(value);
    path.parent()
        .and_then(std::path::Path::file_name)
        .is_some_and(|name| name == ".ssh")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("id_"))
}

fn path_has_component_sequence(value: &str, expected: &[&str]) -> bool {
    let components: Vec<_> = std::path::Path::new(value)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect();
    components
        .windows(expected.len())
        .any(|window| window == expected)
}

fn env_wrapped_invocation(
    invocation: &CommandInvocation,
) -> Result<Option<CommandInvocation>, &'static str> {
    if invocation.command != "env" {
        return Ok(None);
    }

    let mut index = 0;
    while let Some(arg) = invocation.argv.get(index) {
        match arg.as_str() {
            "--" => {
                index += 1;
                break;
            }
            "-i" | "--ignore-environment" | "-0" | "--null" => index += 1,
            "-u" | "--unset" | "-C" | "--chdir" => {
                if invocation.argv.get(index + 1).is_none() {
                    return Err("env wrapper option is missing its required operand");
                }
                index += 2;
            }
            "-S" | "--split-string" => {
                return Err("env split-string wrapper requires a dedicated typed grammar");
            }
            arg if arg.starts_with("--unset=") || arg.starts_with("--chdir=") => index += 1,
            arg if shell_assignment(arg) => index += 1,
            arg if arg.starts_with('-') => {
                return Err("env wrapper uses an unsupported option");
            }
            _ => break,
        }
    }
    let Some(command) = invocation.argv.get(index) else {
        return Ok(None);
    };
    if invocation.argument_provenance.get(index) != Some(&CommandArgumentProvenance::Literal) {
        return Err("env wrapper command is not a literal AST word");
    }
    let mut wrapped = invocation.clone();
    wrapped.command = command
        .rsplit('/')
        .next()
        .unwrap_or(command.as_str())
        .to_string();
    wrapped.command_provenance = CommandArgumentProvenance::Literal;
    wrapped.argv = invocation.argv[index + 1..].to_vec();
    wrapped.argument_provenance = invocation.argument_provenance[index + 1..].to_vec();
    Ok(Some(wrapped))
}

fn shell_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn literal_shell_program(invocation: &CommandInvocation) -> Result<Option<&str>, &'static str> {
    if !matches!(
        invocation.command.as_str(),
        "sh" | "bash" | "dash" | "ksh" | "zsh"
    ) {
        return Ok(None);
    }

    let mut index = 0;
    while let Some(option) = invocation.argv.get(index) {
        if option == "--" {
            index += 1;
            continue;
        }
        if option == "-c"
            || option.starts_with('-')
                && option.len() > 2
                && option[1..].chars().all(|ch| ch.is_ascii_alphabetic())
                && option[1..].contains('c')
        {
            let script_index = index + 1;
            if invocation.argv.get(script_index).is_none() {
                return Err("shell command option is missing its program operand");
            }
            if invocation.argument_provenance.get(script_index)
                != Some(&CommandArgumentProvenance::Literal)
            {
                return Err("shell command program is not a literal AST word");
            }
            return Ok(invocation.argv.get(script_index).map(String::as_str));
        }
        if !option.starts_with('-') {
            return Ok(None);
        }
        index += 1;
    }
    Ok(None)
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
        severity: severity_for_reason(reason_code),
        evidence: evidence.to_string(),
    });
}

fn severity_for_reason(reason_code: &str) -> CommandRiskSeverity {
    match reason_code {
        ROOT_DELETION
        | FILESYSTEM_FORMAT
        | DEVICE_WRITE
        | REMOTE_SHELL_EXEC
        | CREDENTIAL_PATH
        | PROC_STEALTH_OR_DEBUG => CommandRiskSeverity::Critical,
        _ => CommandRiskSeverity::Notice,
    }
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

        assert_eq!(report.highest_severity, CommandRiskSeverity::Notice);
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
    fn destructive_commands_use_literal_argv_grammars() {
        let report = classify_shell_text(
            "scriptlet:install",
            "rm -rf /\nmkfs.ext4 /dev/vdb\ndd if=/tmp/image of=/dev/vdc\nchmod u+s /usr/bin/demo",
        );
        let reasons: Vec<_> = report
            .entries
            .iter()
            .map(|entry| entry.reason_code.as_str())
            .collect();

        assert!(reasons.contains(&ROOT_DELETION));
        assert!(reasons.contains(&FILESYSTEM_FORMAT));
        assert!(reasons.contains(&DEVICE_WRITE));
        assert!(reasons.contains(&SETUID_MUTATION));
        assert_eq!(report.highest_severity, CommandRiskSeverity::Critical);
    }

    #[test]
    fn pipeline_relationship_is_formal_and_inert_text_stays_clean() {
        let remote_shell = classify_shell_text(
            "scriptlet:install",
            "curl https://example.invalid/install | sh",
        );
        assert!(
            remote_shell
                .entries
                .iter()
                .any(|entry| entry.reason_code == REMOTE_SHELL_EXEC)
        );
        assert_eq!(remote_shell.highest_severity, CommandRiskSeverity::Critical);

        let inert = classify_shell_text(
            "scriptlet:install",
            "printf '%s\\n' 'rm -rf /; mkfs.ext4 /dev/vdb; curl | sh'",
        );
        assert_eq!(inert, CommandRiskReport::no_findings());
    }

    #[test]
    fn diagnostic_schema_rejects_retired_policy_status_vocabulary() {
        for retired in ["review", "blocked"] {
            let error = serde_json::from_value::<CommandRiskSeverity>(serde_json::json!(retired))
                .unwrap_err();
            assert!(error.to_string().contains("unknown variant"), "{error}");
        }

        let old_report = serde_json::json!({
            "status": "blocked",
            "classifier_version": "m2-command-risk-v2",
            "entries": []
        });
        let error = serde_json::from_value::<CommandRiskReport>(old_report).unwrap_err();
        assert!(error.to_string().contains("highest_severity"), "{error}");
    }
}
