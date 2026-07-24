// conary-core/src/container/analysis.rs
//! Runtime scriptlet risk derived from formal shell command evidence.

use crate::security::command_risk::{
    BPF_OR_EBPF, COMMAND_FORM_UNRESOLVED, CREDENTIAL_PATH, DEVICE_WRITE, DYNAMIC_LANGUAGE_EXEC,
    FILESYSTEM_FORMAT, NETWORK_FETCH, OBFUSCATION, PACKAGE_MANAGER_FETCH, PERSISTENCE,
    PROC_STEALTH_OR_DEBUG, REMOTE_SHELL_EXEC, ROOT_DELETION, SETUID_MUTATION, SHELL_PARSE_FAILURE,
    classify_shell_text,
};

/// Severity levels for dangerous script detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScriptRisk {
    /// Safe - no risky command evidence detected.
    Safe,
    /// Low risk - minor concerns.
    Low,
    /// Medium risk - protected execution is required in auto mode.
    Medium,
    /// High risk - the command is destructive or its syntax is unresolved.
    High,
    /// Critical - reserved for formally proven compound destructive behavior.
    Critical,
}

impl ScriptRisk {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScriptRisk::Safe => "safe",
            ScriptRisk::Low => "low",
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
    /// Recommendations.
    pub recommendations: Vec<String>,
}

/// Analyze a script using the shared tree-sitter Bash command classifier.
pub fn analyze_script(content: &str) -> ScriptAnalysis {
    let report = classify_shell_text("runtime-scriptlet", content);
    let mut patterns = Vec::new();
    let mut max_risk = ScriptRisk::Safe;

    for entry in report.entries {
        let risk = risk_for_reason(&entry.reason_code);
        patterns.push(format!("{} ({})", entry.reason_code, risk.as_str()));
        max_risk = max_risk.max(risk);
    }

    let recommendations = match max_risk {
        ScriptRisk::Safe => vec!["No risky command evidence was found".to_string()],
        ScriptRisk::Low => vec!["Consider protected execution for untrusted input".to_string()],
        ScriptRisk::Medium => vec!["Protected execution required in auto mode".to_string()],
        ScriptRisk::High | ScriptRisk::Critical => vec![
            "MUST use protected execution".to_string(),
            "Inspect the typed command evidence before any direct execution".to_string(),
        ],
    };

    ScriptAnalysis {
        risk: max_risk,
        patterns,
        recommendations,
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
    fn package_manager_fetches_are_medium_for_auto_sandbox() {
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
    fn dynamic_language_execution_is_medium_for_auto_sandbox() {
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
