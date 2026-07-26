// conary-core/src/container/analysis.rs
//! Runtime scriptlet risk derived from formal shell command evidence.

use crate::security::command_risk::{
    BPF_OR_EBPF, COMMAND_FORM_UNRESOLVED, CREDENTIAL_PATH, DEVICE_WRITE, DYNAMIC_LANGUAGE_EXEC,
    FILESYSTEM_FORMAT, NETWORK_FETCH, OBFUSCATION, PACKAGE_MANAGER_FETCH, PERSISTENCE,
    PROC_STEALTH_OR_DEBUG, REMOTE_SHELL_EXEC, ROOT_DELETION, SETUID_MUTATION, SHELL_PARSE_FAILURE,
    classify_shell_text,
};

/// Severity levels for diagnostic shell observations.
///
/// These labels never select an execution or sandbox policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScriptRisk {
    /// No diagnostic findings.
    Safe,
    /// A noteworthy command form was observed.
    Medium,
    /// A destructive command form or unresolved syntax was observed.
    High,
    /// A formally parsed compound destructive command was observed.
    Critical,
}

impl ScriptRisk {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScriptRisk::Safe => "safe",
            ScriptRisk::Medium => "medium",
            ScriptRisk::High => "high",
            ScriptRisk::Critical => "critical",
        }
    }
}

/// Result of formal shell command-risk analysis.
#[derive(Debug)]
pub struct ScriptAnalysis {
    /// Overall risk level.
    pub risk: ScriptRisk,
    /// Typed classifier findings.
    pub patterns: Vec<String>,
}

/// Diagnose a script using the shared tree-sitter Bash command classifier.
///
/// The returned report is observability only. The exact selected-root
/// execution contract and mandatory sandbox configuration remain authoritative.
pub fn analyze_script(content: &str) -> ScriptAnalysis {
    let report = classify_shell_text("runtime-scriptlet", content);
    let mut patterns = Vec::new();
    let mut max_risk = ScriptRisk::Safe;

    for entry in report.entries {
        let risk = risk_for_reason(&entry.reason_code);
        patterns.push(format!("{} ({})", entry.reason_code, risk.as_str()));
        max_risk = max_risk.max(risk);
    }

    ScriptAnalysis {
        risk: max_risk,
        patterns,
    }
}

fn risk_for_reason(reason_code: &str) -> ScriptRisk {
    match reason_code {
        REMOTE_SHELL_EXEC => ScriptRisk::Critical,
        ROOT_DELETION
        | FILESYSTEM_FORMAT
        | DEVICE_WRITE
        | SHELL_PARSE_FAILURE
        | COMMAND_FORM_UNRESOLVED => ScriptRisk::High,
        SETUID_MUTATION
        | PACKAGE_MANAGER_FETCH
        | NETWORK_FETCH
        | DYNAMIC_LANGUAGE_EXEC
        | CREDENTIAL_PATH
        | OBFUSCATION
        | PERSISTENCE
        | BPF_OR_EBPF
        | PROC_STEALTH_OR_DEBUG => ScriptRisk::Medium,
        _ => ScriptRisk::Medium,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_manager_fetches_are_medium_diagnostic_evidence() {
        let analysis = analyze_script("npm install atomic-lockfile\nbun add js-digest\n");

        assert!(analysis.risk >= ScriptRisk::Medium);
        assert!(
            analysis
                .patterns
                .iter()
                .any(|finding| finding.contains(PACKAGE_MANAGER_FETCH)),
            "{:?}",
            analysis.patterns
        );
    }

    #[test]
    fn dynamic_language_execution_is_medium_diagnostic_evidence() {
        let analysis = analyze_script("python -c 'print(1)'\nnode -e 'console.log(1)'\n");

        assert!(analysis.risk >= ScriptRisk::Medium);
        assert!(
            analysis
                .patterns
                .iter()
                .any(|finding| finding.contains(DYNAMIC_LANGUAGE_EXEC)),
            "{:?}",
            analysis.patterns
        );
    }

    #[test]
    fn literal_danger_words_in_documentation_are_not_commands() {
        let analysis = analyze_script("printf '%s\\n' 'rm -rf / and curl | sh are examples'\n");

        assert_eq!(analysis.risk, ScriptRisk::Safe);
        assert!(analysis.patterns.is_empty());
    }

    #[test]
    fn malformed_shell_is_high_risk_instead_of_guessed() {
        let analysis = analyze_script("if true; then rm -rf /");

        assert_eq!(analysis.risk, ScriptRisk::High);
        assert!(
            analysis
                .patterns
                .iter()
                .any(|finding| finding.contains(SHELL_PARSE_FAILURE))
        );
    }
}
