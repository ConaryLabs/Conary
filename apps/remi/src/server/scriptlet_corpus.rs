// apps/remi/src/server/scriptlet_corpus.rs
//! Evidence-only scriptlet corpus summaries for adapter planning.

use conary_core::packages::traits::Scriptlet;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize)]
pub struct ScriptletCorpusSummary {
    pub distro: String,
    pub package: String,
    pub scriptlet_count: usize,
    pub command_counts: BTreeMap<String, usize>,
    pub command_form_counts: BTreeMap<String, usize>,
    pub blocked_class_hints: Vec<String>,
}

impl ScriptletCorpusSummary {
    pub fn from_scriptlets(distro: &str, package: &str, scriptlets: &[Scriptlet]) -> Self {
        let mut command_counts = BTreeMap::new();
        let mut command_form_counts = BTreeMap::new();
        let mut blocked = BTreeSet::new();

        for scriptlet in scriptlets {
            for evidence in commands_from_scriptlet(&scriptlet.content, &scriptlet.interpreter) {
                *command_counts.entry(evidence.command.clone()).or_insert(0) += 1;
                *command_form_counts.entry(evidence.form.clone()).or_insert(0) += 1;
                for class in blocked_class_hints_for_command(&evidence.command, &evidence.form) {
                    blocked.insert(class);
                }
            }
        }

        Self {
            distro: distro.to_string(),
            package: package.to_string(),
            scriptlet_count: scriptlets.len(),
            command_counts,
            command_form_counts,
            blocked_class_hints: blocked.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandEvidence {
    command: String,
    form: String,
}

fn commands_from_scriptlet(content: &str, interpreter: &str) -> Vec<CommandEvidence> {
    if !looks_like_shell_interpreter(interpreter) {
        return Vec::new();
    }

    content.lines().flat_map(commands_from_line).collect()
}

fn looks_like_shell_interpreter(interpreter: &str) -> bool {
    interpreter.ends_with("sh")
        || interpreter.contains("/sh")
        || interpreter.contains("bash")
        || interpreter.contains("dash")
}

fn commands_from_line(line: &str) -> Vec<CommandEvidence> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Vec::new();
    }

    let normalized = trimmed
        .replace("&&", ";")
        .replace("||", ";")
        .replace('|', ";")
        .replace("$(", ";")
        .replace('(', ";")
        .replace(')', ";")
        .replace('`', ";");

    normalized
        .split(';')
        .filter_map(command_from_segment)
        .collect()
}

fn command_from_segment(segment: &str) -> Option<CommandEvidence> {
    let tokens: Vec<&str> = segment.split_whitespace().collect();
    let mut index = 0;

    while index < tokens.len() {
        let token = tokens[index];
        if token.contains('=') && !token.starts_with('/') {
            index += 1;
            continue;
        }
        if matches!(token, "if" | "then" | "else" | "elif" | "fi" | "do" | "done") {
            index += 1;
            continue;
        }
        if matches!(token, "sudo" | "env" | "chroot") {
            index += 1;
            while index < tokens.len() && tokens[index].starts_with('-') {
                index += 1;
                if index < tokens.len() && !tokens[index].starts_with('-') {
                    index += 1;
                }
            }
            continue;
        }
        break;
    }

    let token = tokens.get(index)?;
    let command = token
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '[' || c == ']')
        .rsplit('/')
        .next()
        .unwrap_or(token);
    if command.is_empty() || command.starts_with('-') || command.contains('=') {
        return None;
    }

    let args = tokens
        .iter()
        .skip(index + 1)
        .filter(|arg| !arg.starts_with('>') && !arg.starts_with("2>"))
        .take(2)
        .copied()
        .collect::<Vec<_>>();
    let form = if args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args.join(" "))
    };

    Some(CommandEvidence {
        command: command.to_string(),
        form,
    })
}

fn blocked_class_hints_for_command(command: &str, form: &str) -> Vec<String> {
    let mut classes = Vec::new();
    match command {
        "dnf" | "yum" | "rpm" | "apt" | "apt-get" | "dpkg" | "pacman" => {
            classes.push("package-manager-recursion".to_string());
        }
        "curl" | "wget" | "scp" | "ssh" => {
            classes.push("network".to_string());
        }
        "restorecon" | "semanage" | "setsebool" => {
            classes.push("selinux".to_string());
        }
        "authselect" | "pam-auth-update" => {
            classes.push("pam".to_string());
        }
        "dracut" | "mkinitcpio" | "update-initramfs" => {
            classes.push("initramfs".to_string());
        }
        "grub-mkconfig" | "grub2-mkconfig" | "update-grub" | "bootctl" => {
            classes.push("bootloader".to_string());
        }
        "modprobe" | "depmod" | "dkms" => {
            classes.push("kernel-module".to_string());
        }
        "setcap" | "setpriv" => {
            classes.push("setuid-setcap".to_string());
        }
        "chmod" if form.contains("u+s") || form.contains("4") => {
            classes.push("setuid-setcap".to_string());
        }
        "sysctl" => {
            classes.push("sysctl".to_string());
        }
        _ => {}
    }

    classes
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::packages::traits::{Scriptlet, ScriptletPhase};

    fn scriptlet(content: &str) -> Scriptlet {
        Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: content.to_string(),
            flags: None,
        }
    }

    #[test]
    fn corpus_summary_counts_helper_commands() {
        let summary = ScriptletCorpusSummary::from_scriptlets(
            "fedora",
            "nginx",
            &[scriptlet("systemctl daemon-reload\nldconfig\n")],
        );

        assert_eq!(summary.package, "nginx");
        assert_eq!(summary.scriptlet_count, 1);
        assert_eq!(summary.command_counts.get("systemctl"), Some(&1));
        assert_eq!(summary.command_counts.get("ldconfig"), Some(&1));
        assert_eq!(
            summary.command_form_counts.get("systemctl daemon-reload"),
            Some(&1)
        );
        assert!(summary.blocked_class_hints.is_empty());
    }

    #[test]
    fn corpus_summary_marks_package_manager_recursion() {
        let summary = ScriptletCorpusSummary::from_scriptlets(
            "arch",
            "bad-news",
            &[scriptlet("pacman -Syu\ncurl https://example.invalid/script.sh\n")],
        );

        assert!(
            summary
                .blocked_class_hints
                .contains(&"package-manager-recursion".to_string())
        );
        assert!(summary.blocked_class_hints.contains(&"network".to_string()));
    }

    #[test]
    fn corpus_summary_handles_empty_scriptlets() {
        let summary = ScriptletCorpusSummary::from_scriptlets("fedora", "empty", &[]);

        assert_eq!(summary.scriptlet_count, 0);
        assert!(summary.command_counts.is_empty());
        assert!(summary.command_form_counts.is_empty());
        assert!(summary.blocked_class_hints.is_empty());
    }

    #[test]
    fn corpus_summary_splits_shell_control_operators() {
        let summary = ScriptletCorpusSummary::from_scriptlets(
            "fedora",
            "compound",
            &[scriptlet(
                "VAR=1 /usr/bin/systemctl daemon-reload && dracut -f | sysctl -p\n",
            )],
        );

        assert_eq!(summary.command_counts.get("systemctl"), Some(&1));
        assert_eq!(summary.command_counts.get("dracut"), Some(&1));
        assert_eq!(summary.command_counts.get("sysctl"), Some(&1));
        assert!(
            summary
                .blocked_class_hints
                .contains(&"initramfs".to_string())
        );
        assert!(summary.blocked_class_hints.contains(&"sysctl".to_string()));
    }
}
