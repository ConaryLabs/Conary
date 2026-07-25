// conary-core/src/recipe/hermetic/command_risk.rs

use crate::recipe::format::Recipe;
use crate::recipe::hermetic::evidence::{BuildCommandRiskEntry, BuildCommandRiskReport};
use crate::security::command_risk::CommandRiskSeverity;

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
        Some(BuildCommandText::new(
            phase,
            recipe_command_for_analysis(recipe, content),
        ))
    })
    .collect()
}

fn recipe_command_for_analysis(recipe: &Recipe, content: &str) -> String {
    let mut rendered = recipe.substitute(content, "/__conary_destdir__");
    rendered = rendered.replace(
        "%(jobs)s",
        &recipe.build.jobs.unwrap_or(1).max(1).to_string(),
    );
    rendered = rendered.replace(
        "%(target)s",
        recipe
            .cross
            .as_ref()
            .and_then(|cross| cross.target.as_deref())
            .unwrap_or("analysis-target"),
    );
    rendered = rendered.replace(
        "%(stage1_sysroot)s",
        recipe
            .cross
            .as_ref()
            .and_then(|cross| cross.sysroot.as_deref())
            .unwrap_or("/__conary_sysroot__"),
    );
    rendered
}

pub fn classify_build_commands(commands: &[BuildCommandText]) -> BuildCommandRiskReport {
    let mut entries = Vec::new();

    for command_text in commands {
        let content = command_text
            .content
            .replace("%(destdir)s", "/__conary_destdir__");
        let report = crate::security::command_risk::classify_shell_text(
            &format!("recipe:{}", command_text.phase),
            &content,
        );
        entries.extend(
            report
                .entries
                .into_iter()
                .map(|entry| BuildCommandRiskEntry {
                    phase: command_text.phase.clone(),
                    command: entry.command,
                    reason_code: entry.reason_code,
                    severity: entry.severity,
                    evidence: entry.evidence,
                }),
        );
    }

    if entries.is_empty() {
        BuildCommandRiskReport::no_findings()
    } else {
        BuildCommandRiskReport {
            highest_severity: entries
                .iter()
                .map(|entry| entry.severity)
                .max()
                .unwrap_or(CommandRiskSeverity::None),
            classifier_version: crate::security::command_risk::COMMAND_RISK_CLASSIFIER_VERSION
                .to_string(),
            entries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::format::{
        BuildSection, PackageSection, Recipe, RemoteSourceSection, SourceSection,
    };
    use crate::recipe::hermetic::evidence::{
        BuildCommandRiskEntry, COMMAND_RISK_CLASSIFIER_VERSION,
    };
    use crate::security::command_risk::CommandRiskSeverity;
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

    fn assert_reason(
        report: &BuildCommandRiskReport,
        reason_code: &str,
        severity: CommandRiskSeverity,
    ) {
        assert!(
            report
                .entries
                .iter()
                .any(|entry| entry.reason_code == reason_code && entry.severity == severity),
            "expected {reason_code} in {:#?}",
            report.entries
        );
    }

    #[test]
    fn package_manager_fetches_are_diagnostic_notices() {
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

        assert_eq!(report.highest_severity, CommandRiskSeverity::Notice);
        assert_eq!(report.classifier_version, COMMAND_RISK_CLASSIFIER_VERSION);
        assert_reason(
            &report,
            "package-manager-fetch",
            CommandRiskSeverity::Notice,
        );
    }

    #[test]
    fn dynamic_language_execution_and_bpf_are_reported() {
        let report = classify_build_commands(&[BuildCommandText::new(
            "check",
            "python -c 'print(1)'\nbpftool prog list",
        )]);

        assert_eq!(report.highest_severity, CommandRiskSeverity::Notice);
        assert_reason(
            &report,
            "dynamic-language-exec",
            CommandRiskSeverity::Notice,
        );
        assert_reason(&report, "bpf-or-ebpf", CommandRiskSeverity::Notice);
    }

    #[test]
    fn clean_commands_are_clean() {
        let report = classify_build_commands(&[
            BuildCommandText::new("configure", "./configure --prefix=/usr"),
            BuildCommandText::new("make", "make -j4"),
            BuildCommandText::new("install", "install -Dm755 demo %(destdir)s/usr/bin/demo"),
        ]);

        assert_eq!(report.highest_severity, CommandRiskSeverity::None);
        assert_eq!(report.classifier_version, COMMAND_RISK_CLASSIFIER_VERSION);
        assert!(report.entries.is_empty());
    }

    #[test]
    fn command_risk_detects_wrappers_and_diagnostic_families() {
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
            assert_ne!(
                report.highest_severity,
                CommandRiskSeverity::None,
                "{content} should produce diagnostic evidence"
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

        assert_eq!(report.highest_severity, CommandRiskSeverity::Notice);
        assert_eq!(package_manager_commands, vec!["bun", "npm"]);
    }

    #[test]
    fn formal_ast_does_not_pair_signals_across_shell_commands() {
        let report = classify_build_commands(&[BuildCommandText::new(
            "make",
            "go env && make install\ngit status & make clone\npython script.py && echo -c\ngo env&make install",
        )]);

        assert_eq!(report.highest_severity, CommandRiskSeverity::None);
        assert!(report.entries.is_empty());

        let report = classify_build_commands(&[
            BuildCommandText::new("make", "go install example.org/tool"),
            BuildCommandText::new("setup", "git clone https://example.invalid/repo"),
            BuildCommandText::new("check", "/usr/bin/python3 -c 'print(1)'"),
        ]);

        assert_eq!(report.highest_severity, CommandRiskSeverity::Notice);
        assert_reason(
            &report,
            "package-manager-fetch",
            CommandRiskSeverity::Notice,
        );
        assert_reason(&report, "network-fetch", CommandRiskSeverity::Notice);
        assert_reason(
            &report,
            "dynamic-language-exec",
            CommandRiskSeverity::Notice,
        );
    }

    #[test]
    fn formal_ast_preserves_command_arguments_across_redirections() {
        let cases = [
            ("go 2>&1 install example.org/tool", "package-manager-fetch"),
            (
                "bash -c 'git 2>&1 clone https://example.invalid/repo'",
                "network-fetch",
            ),
            (
                "bash -c '/usr/bin/python3 2>&1 -c \"print(1)\"'",
                "dynamic-language-exec",
            ),
            ("bash -c 'base64 2>&1 --decode payload.txt'", "obfuscation"),
            (
                "bash -c 'systemctl 2>&1 enable payload.service'",
                "persistence",
            ),
        ];

        for (content, reason_code) in cases {
            let report = classify_build_commands(&[BuildCommandText::new("make", content)]);
            assert_eq!(
                report.highest_severity,
                CommandRiskSeverity::Notice,
                "{content} should produce a diagnostic notice"
            );
            assert_reason(&report, reason_code, CommandRiskSeverity::Notice);
        }
    }

    #[test]
    fn formal_wrapper_contracts_report_unresolved_forms_without_guessing() {
        let unresolved = [
            "env -S 'npm install foo'",
            "bash -c \"$BUILD_PROGRAM\"",
            "env --unknown-option npm install foo",
        ];
        for content in unresolved {
            let report = classify_build_commands(&[BuildCommandText::new("make", content)]);
            assert_eq!(
                report.highest_severity,
                CommandRiskSeverity::Notice,
                "{content}"
            );
            assert_reason(
                &report,
                "command-form-unresolved",
                CommandRiskSeverity::Notice,
            );
        }

        let literal_mention = classify_build_commands(&[BuildCommandText::new(
            "make",
            "printf '%s\\n' 'npm install is documentation, not an invocation'",
        )]);
        assert_eq!(literal_mention.highest_severity, CommandRiskSeverity::None);
        assert!(literal_mention.entries.is_empty());
    }
}
