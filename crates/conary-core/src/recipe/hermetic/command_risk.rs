// conary-core/src/recipe/hermetic/command_risk.rs

use crate::ccs::convert::command_evidence::{
    CommandInvocation, extract_invocations_from_shell_text,
};
use crate::recipe::format::Recipe;
use crate::recipe::hermetic::evidence::{
    BuildCommandRiskEntry, BuildCommandRiskReport, COMMAND_RISK_CLASSIFIER_VERSION, PolicyStatus,
};

#[derive(Debug, Clone)]
pub struct BuildCommandText {
    pub phase: String,
    pub content: String,
}

impl BuildCommandText {
    pub fn new(phase: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            content: content.into(),
        }
    }
}

pub fn collect_recipe_command_text(recipe: &Recipe) -> Vec<BuildCommandText> {
    [
        ("setup", &recipe.build.setup),
        ("configure", &recipe.build.configure),
        ("make", &recipe.build.make),
        ("check", &recipe.build.check),
        ("install", &recipe.build.install),
        ("post_install", &recipe.build.post_install),
    ]
    .into_iter()
    .filter_map(|(phase, content)| {
        let content = content.as_ref()?;
        if content.trim().is_empty() {
            return None;
        }
        Some(BuildCommandText::new(phase, content.clone()))
    })
    .collect()
}

pub fn classify_build_commands(commands: &[BuildCommandText]) -> BuildCommandRiskReport {
    let mut entries = Vec::new();

    for command_text in commands {
        let invocations = extract_invocations_from_shell_text(
            &format!("recipe:{}", command_text.phase),
            &command_text.content,
            Some(&command_text.phase),
        );

        for invocation in &invocations {
            if let Some(signal) = signal_for_invocation(invocation) {
                push_entry(
                    &mut entries,
                    &command_text.phase,
                    &invocation.command,
                    signal.reason_code,
                    invocation
                        .raw_line
                        .as_deref()
                        .unwrap_or(command_text.content.trim()),
                );
            }
        }

        for line in command_text.content.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            for signal in raw_line_signals(line) {
                push_raw_entry(&mut entries, &command_text.phase, signal, line);
            }
        }
    }

    if entries.is_empty() {
        BuildCommandRiskReport::clean()
    } else {
        BuildCommandRiskReport {
            status: PolicyStatus::Blocked,
            classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RiskSignal {
    command: &'static str,
    reason_code: &'static str,
}

fn signal_for_invocation(invocation: &CommandInvocation) -> Option<RiskSignal> {
    let command = invocation.command.as_str();
    let argv: Vec<&str> = invocation.argv.iter().map(String::as_str).collect();

    if is_package_manager_fetch(command, &argv) {
        return Some(RiskSignal {
            command: "package-manager",
            reason_code: "package-manager-fetch",
        });
    }

    if is_network_fetch(command, &argv) {
        return Some(RiskSignal {
            command: "network-fetch",
            reason_code: "network-fetch",
        });
    }

    if is_dynamic_language_exec(command, &argv) {
        return Some(RiskSignal {
            command: "dynamic-language",
            reason_code: "dynamic-language-exec",
        });
    }

    if is_obfuscation(command, &argv) {
        return Some(RiskSignal {
            command: "obfuscation",
            reason_code: "obfuscation",
        });
    }

    if is_persistence(command, &argv) {
        return Some(RiskSignal {
            command: "persistence",
            reason_code: "persistence",
        });
    }

    if is_bpf_or_ebpf(command) {
        return Some(RiskSignal {
            command: "bpf",
            reason_code: "bpf-or-ebpf",
        });
    }

    if is_debug_or_stealth_command(command) {
        return Some(RiskSignal {
            command: "proc-debug",
            reason_code: "proc-stealth-or-debug",
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
            reason_code: "package-manager-fetch",
        });
    }
    if raw_network_fetch(&lower) {
        signals.push(RiskSignal {
            command: "network-fetch",
            reason_code: "network-fetch",
        });
    }
    if raw_dynamic_language_exec(&lower) {
        signals.push(RiskSignal {
            command: "dynamic-language",
            reason_code: "dynamic-language-exec",
        });
    }
    if raw_credential_path(&lower) {
        signals.push(RiskSignal {
            command: "credential-path",
            reason_code: "credential-path",
        });
    }
    if raw_obfuscation(&lower) {
        signals.push(RiskSignal {
            command: "obfuscation",
            reason_code: "obfuscation",
        });
    }
    if raw_persistence(&lower) {
        signals.push(RiskSignal {
            command: "persistence",
            reason_code: "persistence",
        });
    }
    if raw_bpf_or_ebpf(&lower) {
        signals.push(RiskSignal {
            command: "bpf",
            reason_code: "bpf-or-ebpf",
        });
    }
    if raw_proc_stealth_or_debug(&lower) {
        signals.push(RiskSignal {
            command: "proc-debug",
            reason_code: "proc-stealth-or-debug",
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
    argv.iter().any(|arg| *arg == needle)
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
    ["bpf", "bpftool", "libbpf", "perf_event_open"]
        .iter()
        .any(|token| contains_shell_word(line, token))
}

fn raw_proc_stealth_or_debug(line: &str) -> bool {
    contains_shell_word(line, "ptrace")
        || contains_shell_word(line, "strace")
        || contains_shell_word(line, "gdb")
        || line.contains("/proc/") && (line.contains("/mem") || line.contains("/environ"))
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
    while let Some(ch) = chars.next() {
        match ch {
            '&' if chars.peek() == Some(&'&') => return true,
            '|' | ';' | '(' | ')' | '`' | '\n' => return true,
            '$' if chars.peek() == Some(&'(') => return true,
            _ => {}
        }
    }
    false
}

fn push_entry(
    entries: &mut Vec<BuildCommandRiskEntry>,
    phase: &str,
    command: &str,
    reason_code: &str,
    evidence: &str,
) {
    if entries.iter().any(|entry| {
        entry.phase == phase
            && entry.command == command
            && entry.reason_code == reason_code
            && entry.evidence == evidence
    }) {
        return;
    }

    entries.push(BuildCommandRiskEntry {
        phase: phase.to_string(),
        command: command.to_string(),
        reason_code: reason_code.to_string(),
        severity: PolicyStatus::Blocked,
        evidence: evidence.to_string(),
    });
}

fn push_raw_entry(
    entries: &mut Vec<BuildCommandRiskEntry>,
    phase: &str,
    signal: RiskSignal,
    evidence: &str,
) {
    if entries.iter().any(|entry| {
        entry.phase == phase
            && entry.reason_code == signal.reason_code
            && entry.evidence == evidence
    }) {
        return;
    }

    push_entry(entries, phase, signal.command, signal.reason_code, evidence);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::format::{
        BuildSection, PackageSection, Recipe, RemoteSourceSection, SourceSection,
    };
    use crate::recipe::hermetic::evidence::{BuildCommandRiskEntry, PolicyStatus};
    use std::collections::HashMap;

    fn recipe_with_build(build: BuildSection) -> Recipe {
        Recipe {
            package: PackageSection {
                name: "demo".to_string(),
                version: "1.0.0".to_string(),
                release: "1".to_string(),
                summary: None,
                description: None,
                license: None,
                homepage: None,
            },
            source: SourceSection::Remote(RemoteSourceSection {
                archive: "https://example.invalid/demo-1.0.0.tar.gz".to_string(),
                checksum: "sha256:demo".to_string(),
                signature: None,
                additional: Vec::new(),
                extract_dir: None,
            }),
            build,
            cross: None,
            patches: None,
            components: None,
            variables: HashMap::new(),
        }
    }

    fn build_section() -> BuildSection {
        BuildSection {
            requires: Vec::new(),
            makedepends: Vec::new(),
            configure: None,
            make: None,
            install: None,
            check: None,
            setup: None,
            post_install: None,
            environment: HashMap::new(),
            workdir: None,
            script_file: None,
            jobs: None,
            stage: None,
        }
    }

    fn reason_codes(entries: &[BuildCommandRiskEntry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| entry.reason_code.as_str())
            .collect()
    }

    fn assert_reason(report: &BuildCommandRiskReport, reason_code: &str) {
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.reason_code == reason_code
                    && entry.severity == PolicyStatus::Blocked),
            "expected {reason_code} in {:#?}",
            report.entries
        );
    }

    #[test]
    fn package_manager_fetches_are_blocked_without_evidence() {
        let mut build = build_section();
        build.setup = Some("   \n".to_string());
        build.configure = Some("npm install atomic-lockfile".to_string());
        build.make = Some("cargo install cargo-audit".to_string());
        build.install = Some("make DESTDIR=%(destdir)s install".to_string());
        let recipe = recipe_with_build(build);

        let commands = collect_recipe_command_text(&recipe);
        let phases: Vec<&str> = commands
            .iter()
            .map(|command| command.phase.as_str())
            .collect();
        assert_eq!(phases, vec!["configure", "make", "install"]);

        let report = classify_build_commands(&commands);

        assert_eq!(report.status, PolicyStatus::Blocked);
        assert_eq!(report.classifier_version, COMMAND_RISK_CLASSIFIER_VERSION);
        assert_reason(&report, "package-manager-fetch");
    }

    #[test]
    fn dynamic_language_execution_and_bpf_are_reported() {
        let report = classify_build_commands(&[BuildCommandText::new(
            "check",
            "python -c 'print(1)'\nbpftool prog list",
        )]);

        assert_eq!(report.status, PolicyStatus::Blocked);
        assert_reason(&report, "dynamic-language-exec");
        assert_reason(&report, "bpf-or-ebpf");
    }

    #[test]
    fn clean_commands_are_clean() {
        let report = classify_build_commands(&[
            BuildCommandText::new("configure", "./configure --prefix=/usr"),
            BuildCommandText::new("make", "make -j4"),
            BuildCommandText::new("install", "install -Dm755 demo %(destdir)s/usr/bin/demo"),
        ]);

        assert_eq!(report.status, PolicyStatus::Clean);
        assert_eq!(report.classifier_version, COMMAND_RISK_CLASSIFIER_VERSION);
        assert!(report.entries.is_empty());
    }

    #[test]
    fn command_risk_detects_wrappers_and_every_block_family() {
        let cases = [
            (
                "env -i npm install atomic-lockfile",
                "package-manager-fetch",
            ),
            (
                "/usr/bin/curl https://example.invalid/payload",
                "network-fetch",
            ),
            (
                "bash -c 'curl https://example.invalid/payload'",
                "network-fetch",
            ),
            (
                "echo $(wget https://example.invalid/payload)",
                "network-fetch",
            ),
            ("python -c 'print(1)'", "dynamic-language-exec"),
            ("cat /etc/shadow", "credential-path"),
            ("base64 --decode payload.txt", "obfuscation"),
            ("systemctl --user enable payload.service", "persistence"),
            ("cat /proc/self/environ", "proc-stealth-or-debug"),
        ];

        for (content, reason_code) in cases {
            let report = classify_build_commands(&[BuildCommandText::new("make", content)]);
            assert_eq!(
                report.status,
                PolicyStatus::Blocked,
                "{content} should be blocked"
            );
            assert_eq!(
                reason_codes(&report.entries),
                vec![reason_code],
                "{content}"
            );
        }
    }

    #[test]
    fn command_risk_preserves_multiple_commands_with_same_reason_and_line() {
        let report = classify_build_commands(&[BuildCommandText::new(
            "make",
            "npm install foo && bun add bar",
        )]);

        let mut package_manager_commands: Vec<&str> = report
            .entries
            .iter()
            .filter(|entry| entry.reason_code == "package-manager-fetch")
            .map(|entry| entry.command.as_str())
            .collect();
        package_manager_commands.sort_unstable();

        assert_eq!(report.status, PolicyStatus::Blocked);
        assert_eq!(package_manager_commands, vec!["bun", "npm"]);
    }

    #[test]
    fn raw_fallback_does_not_pair_multiword_signals_across_shell_segments() {
        let report = classify_build_commands(&[BuildCommandText::new(
            "make",
            "go env && make install\ngit status && make clone\npython script.py && echo -c",
        )]);

        assert_eq!(report.status, PolicyStatus::Clean);
        assert!(report.entries.is_empty());

        let report = classify_build_commands(&[
            BuildCommandText::new("make", "go install example.org/tool"),
            BuildCommandText::new("setup", "git clone https://example.invalid/repo"),
            BuildCommandText::new("check", "/usr/bin/python3 -c 'print(1)'"),
        ]);

        assert_eq!(report.status, PolicyStatus::Blocked);
        assert_reason(&report, "package-manager-fetch");
        assert_reason(&report, "network-fetch");
        assert_reason(&report, "dynamic-language-exec");
    }
}
