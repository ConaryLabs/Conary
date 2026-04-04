// conary-core/src/container/analysis.rs

use regex::RegexSet;
use std::sync::LazyLock;

/// Severity levels for dangerous script detection
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScriptRisk {
    /// Safe - no risky patterns detected
    Safe,
    /// Low risk - minor concerns
    Low,
    /// Medium risk - should probably sandbox
    Medium,
    /// High risk - definitely sandbox
    High,
    /// Critical - extremely dangerous patterns
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

/// Result of analyzing a script for dangerous patterns
#[derive(Debug)]
pub struct ScriptAnalysis {
    /// Overall risk level
    pub risk: ScriptRisk,
    /// Dangerous patterns found
    pub patterns: Vec<String>,
    /// Recommendations
    pub recommendations: Vec<String>,
}

/// Dangerous patterns: (regex, risk level, human description).
///
/// All patterns are case-insensitive compiled regexes. Each entry maps to one
/// danger category. Patterns that previously used ad-hoc `.*` splitting are now
/// proper regex; special characters (`|`, `*`, `(`, `)`, `{`, `}`, `+`) that
/// should be treated as literals are escaped with `\`.
const DANGEROUS_PATTERNS: &[(&str, ScriptRisk, &str)] = &[
    (
        r"curl.*\|.*sh",
        ScriptRisk::Critical,
        "Downloads and executes remote code",
    ),
    (
        r"wget.*\|.*sh",
        ScriptRisk::Critical,
        "Downloads and executes remote code",
    ),
    (r"eval.*$", ScriptRisk::Critical, "Dynamic code execution"),
    (r"rm -rf /", ScriptRisk::High, "Recursive deletion of root"),
    (
        r"rm -rf /\*",
        ScriptRisk::High,
        "Recursive deletion of root contents",
    ),
    (r"mkfs", ScriptRisk::High, "Filesystem formatting"),
    (
        r"dd if=.* of=/dev/",
        ScriptRisk::High,
        "Direct device write",
    ),
    (r":\(\)\{ :\|:& \};:", ScriptRisk::High, "Fork bomb"),
    (
        r"chmod.*4[0-7][0-7][0-7]",
        ScriptRisk::Medium,
        "Setuid bit manipulation",
    ),
    (
        r"chmod.*u\+s",
        ScriptRisk::Medium,
        "Setuid bit manipulation",
    ),
    ("crontab", ScriptRisk::Medium, "Cron job modification"),
    ("/etc/shadow", ScriptRisk::Medium, "Password file access"),
    (
        "/etc/sudoers",
        ScriptRisk::Medium,
        "Sudo configuration access",
    ),
    (
        r"ssh.*authorized_keys",
        ScriptRisk::Medium,
        "SSH key manipulation",
    ),
    (
        r"nc ",
        ScriptRisk::Low,
        "Netcat usage (network backdoor potential)",
    ),
    (
        r"ncat ",
        ScriptRisk::Low,
        "Ncat usage (network backdoor potential)",
    ),
    (
        r"/dev/tcp/",
        ScriptRisk::Low,
        "Bash TCP device (network comms)",
    ),
    (
        r"/dev/udp/",
        ScriptRisk::Low,
        "Bash UDP device (network comms)",
    ),
    (
        r"base64.*-d",
        ScriptRisk::Low,
        "Base64 decoding (obfuscation)",
    ),
];

/// Compiled `RegexSet` for all dangerous-pattern regexes (case-insensitive).
///
/// Built once at program startup via `LazyLock`. The index into the set
/// corresponds 1-to-1 with the index into `DANGEROUS_PATTERNS`.
static DANGEROUS_REGEX_SET: LazyLock<RegexSet> = LazyLock::new(|| {
    let patterns: Vec<&str> = DANGEROUS_PATTERNS.iter().map(|(p, _, _)| *p).collect();
    RegexSet::new(&patterns).expect("DANGEROUS_PATTERNS contains an invalid regex")
});

/// Analyze a script for dangerous patterns
pub fn analyze_script(content: &str) -> ScriptAnalysis {
    let mut patterns = Vec::new();
    let mut recommendations = Vec::new();
    let mut max_risk = ScriptRisk::Safe;

    let matches = DANGEROUS_REGEX_SET.matches(content);
    for idx in matches.iter() {
        let (_, risk, description) = &DANGEROUS_PATTERNS[idx];
        patterns.push(format!("{} ({})", description, risk.as_str()));
        if *risk > max_risk {
            max_risk = *risk;
        }
    }

    match max_risk {
        ScriptRisk::Safe => {
            recommendations.push("Script appears safe for execution".to_string());
        }
        ScriptRisk::Low => {
            recommendations.push("Consider sandboxing if running untrusted package".to_string());
        }
        ScriptRisk::Medium => {
            recommendations.push("Sandboxed execution recommended".to_string());
        }
        ScriptRisk::High | ScriptRisk::Critical => {
            recommendations.push("MUST sandbox this script".to_string());
            recommendations.push("Review script contents before execution".to_string());
        }
    }

    ScriptAnalysis {
        risk: max_risk,
        patterns,
        recommendations,
    }
}
