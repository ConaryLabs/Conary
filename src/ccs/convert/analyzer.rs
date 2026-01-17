// src/ccs/convert/analyzer.rs
//! Scriptlet analyzer for detecting declarative operations
//!
//! Pattern-matches shell scripts to DETECT operations (not remove):
//! - `useradd`/`groupadd` -> `hooks.users`/`hooks.groups`
//! - `systemctl enable` -> `hooks.systemd`
//! - `mkdir -p`/`install -d` -> `hooks.directories`
//!
//! Original scriptlet is preserved and run AFTER declarative hooks.
//! No script modification (avoids regex madness).

use crate::ccs::convert::fidelity::{
    DetectedOperation, FidelityReport, OperationType, UncertainOperation, UncertaintySeverity,
};
use crate::ccs::manifest::{
    DirectoryHook, GroupHook, Hooks, SystemdHook, UserHook,
};
use crate::packages::traits::Scriptlet;
use regex::Regex;
use std::sync::LazyLock;

/// Detected hook from scriptlet analysis
#[derive(Debug, Clone)]
pub enum DetectedHook {
    User(UserHook),
    Group(GroupHook),
    Directory(DirectoryHook),
    Systemd(SystemdHook),
}

/// Analyzer for extracting declarative operations from scriptlets
///
/// This is a zero-sized type that provides access to pre-compiled regex patterns
/// stored in a static `LazyLock`. No per-instance state is needed.
pub struct ScriptletAnalyzer;

/// Pre-compiled regex patterns for script analysis
struct Patterns {
    useradd: Regex,
    groupadd: Regex,
    mkdir: Regex,
    install_d: Regex,
    systemctl_enable: Regex,
    systemctl_reload: Regex,
    ldconfig: Regex,
    external_script: Regex,
    complex_logic: Regex,
}

// Pre-compile patterns at first use
static PATTERNS: LazyLock<Patterns> = LazyLock::new(|| {
    Patterns {
        // User/Group management
        useradd: Regex::new(r"(?m)^\s*(getent\s+passwd\s+(\S+)\s*\|\|\s*)?useradd\s+(.+)$").unwrap(),
        groupadd: Regex::new(r"(?m)^\s*(getent\s+group\s+(\S+)\s*\|\|\s*)?groupadd\s+(.+)$").unwrap(),

        // Directory creation
        mkdir: Regex::new(r"(?m)^\s*mkdir\s+(-p\s+)?(.+)$").unwrap(),
        install_d: Regex::new(r"(?m)^\s*install\s+.*-d\s+(.+)$").unwrap(),

        // Systemd operations
        systemctl_enable: Regex::new(r"(?m)^\s*systemctl\s+(--no-reload\s+)?(enable|disable)\s+(\S+)").unwrap(),
        systemctl_reload: Regex::new(r"(?m)^\s*systemctl\s+daemon-reload").unwrap(),

        // System cache updates (detected but handled by triggers)
        ldconfig: Regex::new(r"(?m)^\s*(/sbin/)?ldconfig").unwrap(),

        // Complex patterns to flag as uncertain
        external_script: Regex::new(r"(?m)^\s*(/[\w/.-]+\.sh|source\s+|\.[\s/])").unwrap(),
        complex_logic: Regex::new(r"(?m)(for\s+\w+\s+in|while\s+|case\s+|function\s+\w+|\$\([^)]+\))").unwrap(),
    }
});

impl Default for ScriptletAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptletAnalyzer {
    /// Create a new analyzer
    #[inline]
    pub fn new() -> Self {
        Self
    }

    /// Analyze all scriptlets and extract declarative hooks
    ///
    /// Returns detected hooks and a fidelity report.
    /// Original scriptlets are NOT modified - they should be run after hooks.
    pub fn analyze(&self, scriptlets: &[Scriptlet]) -> (Vec<DetectedHook>, FidelityReport) {
        let mut hooks = Vec::new();
        let mut report = FidelityReport::new();

        for scriptlet in scriptlets {
            let phase = scriptlet.phase.to_string();
            self.analyze_scriptlet(&scriptlet.content, &phase, &mut hooks, &mut report);
            report.mark_scriptlet_preserved();
        }

        report.recalculate_level();
        (hooks, report)
    }

    /// Analyze a single scriptlet's content
    fn analyze_scriptlet(
        &self,
        content: &str,
        phase: &str,
        hooks: &mut Vec<DetectedHook>,
        report: &mut FidelityReport,
    ) {
        // User additions
        for cap in PATTERNS.useradd.captures_iter(content) {
            let args = cap.get(3).map(|m| m.as_str()).unwrap_or("");
            if let Some(hook) = self.parse_useradd(args) {
                let mut params = std::collections::HashMap::new();
                params.insert("name".to_string(), hook.name.clone());
                if hook.system {
                    params.insert("system".to_string(), "true".to_string());
                }
                if let Some(ref home) = hook.home {
                    params.insert("home".to_string(), home.clone());
                }

                report.add_detected(DetectedOperation {
                    operation_type: OperationType::UserAdd,
                    phase: phase.to_string(),
                    parameters: params,
                    source_lines: vec![cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()],
                });

                hooks.push(DetectedHook::User(hook));
            }
        }

        // Group additions
        for cap in PATTERNS.groupadd.captures_iter(content) {
            let args = cap.get(3).map(|m| m.as_str()).unwrap_or("");
            if let Some(hook) = self.parse_groupadd(args) {
                let mut params = std::collections::HashMap::new();
                params.insert("name".to_string(), hook.name.clone());
                if hook.system {
                    params.insert("system".to_string(), "true".to_string());
                }

                report.add_detected(DetectedOperation {
                    operation_type: OperationType::GroupAdd,
                    phase: phase.to_string(),
                    parameters: params,
                    source_lines: vec![cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()],
                });

                hooks.push(DetectedHook::Group(hook));
            }
        }

        // Directory creation (mkdir -p)
        for cap in PATTERNS.mkdir.captures_iter(content) {
            let path = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            if !path.is_empty() && path.starts_with('/') {
                let hook = DirectoryHook {
                    path: path.to_string(),
                    mode: "0755".to_string(),
                    owner: "root".to_string(),
                    group: "root".to_string(),
                    cleanup: None,
                };

                let mut params = std::collections::HashMap::new();
                params.insert("path".to_string(), path.to_string());

                report.add_detected(DetectedOperation {
                    operation_type: OperationType::DirectoryCreate,
                    phase: phase.to_string(),
                    parameters: params,
                    source_lines: vec![cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()],
                });

                hooks.push(DetectedHook::Directory(hook));
            }
        }

        // Directory creation (install -d)
        for cap in PATTERNS.install_d.captures_iter(content) {
            let path = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            if !path.is_empty() && path.starts_with('/') {
                let hook = DirectoryHook {
                    path: path.to_string(),
                    mode: "0755".to_string(),
                    owner: "root".to_string(),
                    group: "root".to_string(),
                    cleanup: None,
                };

                let mut params = std::collections::HashMap::new();
                params.insert("path".to_string(), path.to_string());

                report.add_detected(DetectedOperation {
                    operation_type: OperationType::DirectoryCreate,
                    phase: phase.to_string(),
                    parameters: params,
                    source_lines: vec![cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()],
                });

                hooks.push(DetectedHook::Directory(hook));
            }
        }

        // Systemd enable/disable
        for cap in PATTERNS.systemctl_enable.captures_iter(content) {
            let action = cap.get(2).map(|m| m.as_str()).unwrap_or("enable");
            let unit = cap.get(3).map(|m| m.as_str()).unwrap_or("");

            if !unit.is_empty() {
                let hook = SystemdHook {
                    unit: unit.to_string(),
                    enable: action == "enable",
                };

                let mut params = std::collections::HashMap::new();
                params.insert("unit".to_string(), unit.to_string());
                params.insert("enable".to_string(), hook.enable.to_string());

                report.add_detected(DetectedOperation {
                    operation_type: OperationType::SystemdEnable,
                    phase: phase.to_string(),
                    parameters: params,
                    source_lines: vec![cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()],
                });

                hooks.push(DetectedHook::Systemd(hook));
            }
        }

        // Detect common operations that don't need hooks (handled by triggers)
        for cap in PATTERNS.ldconfig.captures_iter(content) {
            report.add_detected(DetectedOperation {
                operation_type: OperationType::Ldconfig,
                phase: phase.to_string(),
                parameters: std::collections::HashMap::new(),
                source_lines: vec![cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()],
            });
        }

        for cap in PATTERNS.systemctl_reload.captures_iter(content) {
            report.add_detected(DetectedOperation {
                operation_type: OperationType::SystemdReload,
                phase: phase.to_string(),
                parameters: std::collections::HashMap::new(),
                source_lines: vec![cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()],
            });
        }

        // Detect uncertain operations
        for cap in PATTERNS.external_script.captures_iter(content) {
            let line = cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default();
            // Skip if it's a shebang line
            if !line.trim().starts_with("#!") {
                report.add_uncertain(UncertainOperation {
                    description: "External script execution".to_string(),
                    reason: "Script calls external script file".to_string(),
                    severity: UncertaintySeverity::High,
                    source_lines: vec![line],
                });
            }
        }

        for cap in PATTERNS.complex_logic.captures_iter(content) {
            report.add_uncertain(UncertainOperation {
                description: "Complex control flow".to_string(),
                reason: "Contains loops, conditionals, or command substitution".to_string(),
                severity: UncertaintySeverity::Medium,
                source_lines: vec![cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()],
            });
        }
    }

    /// Parse useradd arguments into a UserHook
    fn parse_useradd(&self, args: &str) -> Option<UserHook> {
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        let mut system = false;
        let mut home = None;
        let mut shell = None;
        let mut group = None;
        let mut name = None;

        let mut i = 0;
        while i < parts.len() {
            let part = parts[i];
            match part {
                "-r" | "--system" => system = true,
                "-d" | "--home-dir" | "--home" => {
                    if i + 1 < parts.len() {
                        home = Some(parts[i + 1].to_string());
                        i += 1;
                    }
                }
                "-s" | "--shell" => {
                    if i + 1 < parts.len() {
                        shell = Some(parts[i + 1].to_string());
                        i += 1;
                    }
                }
                "-g" | "--gid" => {
                    if i + 1 < parts.len() {
                        group = Some(parts[i + 1].to_string());
                        i += 1;
                    }
                }
                "-M" | "--no-create-home" | "-c" | "--comment" | "-u" | "--uid" | "-G" | "--groups" => {
                    // Skip these and their arguments if any
                    if i + 1 < parts.len() && !parts[i + 1].starts_with('-') {
                        i += 1;
                    }
                }
                _ if !part.starts_with('-') => {
                    // This is the username
                    name = Some(part.to_string());
                }
                _ => {}
            }
            i += 1;
        }

        name.map(|n| UserHook {
            name: n,
            system,
            home,
            shell,
            group,
        })
    }

    /// Parse groupadd arguments into a GroupHook
    fn parse_groupadd(&self, args: &str) -> Option<GroupHook> {
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        let mut system = false;
        let mut name = None;

        for part in &parts {
            match *part {
                "-r" | "--system" => system = true,
                "-g" | "--gid" => {
                    // Skip GID argument
                }
                _ if !part.starts_with('-') => {
                    name = Some(part.to_string());
                }
                _ => {}
            }
        }

        name.map(|n| GroupHook { name: n, system })
    }

    /// Build a complete Hooks structure from detected hooks
    pub fn build_hooks(detected: &[DetectedHook]) -> Hooks {
        let mut hooks = Hooks::default();

        for hook in detected {
            match hook {
                DetectedHook::User(u) => {
                    // Deduplicate by name
                    if !hooks.users.iter().any(|h| h.name == u.name) {
                        hooks.users.push(u.clone());
                    }
                }
                DetectedHook::Group(g) => {
                    if !hooks.groups.iter().any(|h| h.name == g.name) {
                        hooks.groups.push(g.clone());
                    }
                }
                DetectedHook::Directory(d) => {
                    if !hooks.directories.iter().any(|h| h.path == d.path) {
                        hooks.directories.push(d.clone());
                    }
                }
                DetectedHook::Systemd(s) => {
                    if !hooks.systemd.iter().any(|h| h.unit == s.unit) {
                        hooks.systemd.push(s.clone());
                    }
                }
            }
        }

        hooks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::convert::fidelity::FidelityLevel;
    use crate::packages::traits::ScriptletPhase;

    fn make_scriptlet(content: &str, phase: ScriptletPhase) -> Scriptlet {
        Scriptlet {
            phase,
            interpreter: "/bin/sh".to_string(),
            content: content.to_string(),
            flags: None,
        }
    }

    #[test]
    fn test_analyze_useradd() {
        let analyzer = ScriptletAnalyzer::new();
        let scriptlet = make_scriptlet(
            "getent passwd nginx || useradd -r -d /var/lib/nginx nginx",
            ScriptletPhase::PreInstall,
        );

        let (hooks, report) = analyzer.analyze(&[scriptlet]);

        assert_eq!(hooks.len(), 1);
        if let DetectedHook::User(user) = &hooks[0] {
            assert_eq!(user.name, "nginx");
            assert!(user.system);
            assert_eq!(user.home, Some("/var/lib/nginx".to_string()));
        } else {
            panic!("Expected User hook");
        }

        assert_eq!(report.hooks_extracted, 1);
        assert_eq!(report.level, FidelityLevel::Full);
    }

    #[test]
    fn test_analyze_groupadd() {
        let analyzer = ScriptletAnalyzer::new();
        let scriptlet = make_scriptlet(
            "getent group www-data || groupadd -r www-data",
            ScriptletPhase::PreInstall,
        );

        let (hooks, report) = analyzer.analyze(&[scriptlet]);

        assert_eq!(hooks.len(), 1);
        if let DetectedHook::Group(group) = &hooks[0] {
            assert_eq!(group.name, "www-data");
            assert!(group.system);
        } else {
            panic!("Expected Group hook");
        }

        assert_eq!(report.hooks_extracted, 1);
    }

    #[test]
    fn test_analyze_mkdir() {
        let analyzer = ScriptletAnalyzer::new();
        let scriptlet = make_scriptlet(
            "mkdir -p /var/lib/myapp\nmkdir -p /var/log/myapp",
            ScriptletPhase::PostInstall,
        );

        let (hooks, report) = analyzer.analyze(&[scriptlet]);

        assert_eq!(hooks.len(), 2);
        for hook in &hooks {
            if let DetectedHook::Directory(dir) = hook {
                assert!(dir.path.starts_with("/var/"));
            } else {
                panic!("Expected Directory hook");
            }
        }

        assert_eq!(report.hooks_extracted, 2);
    }

    #[test]
    fn test_analyze_systemctl_enable() {
        let analyzer = ScriptletAnalyzer::new();
        let scriptlet = make_scriptlet(
            "systemctl enable nginx.service\nsystemctl daemon-reload",
            ScriptletPhase::PostInstall,
        );

        let (hooks, report) = analyzer.analyze(&[scriptlet]);

        // Should detect both enable and daemon-reload
        assert!(!hooks.is_empty());

        let has_systemd = hooks.iter().any(|h| matches!(h, DetectedHook::Systemd(s) if s.unit == "nginx.service"));
        assert!(has_systemd);

        assert!(report.hooks_extracted >= 1);
    }

    #[test]
    fn test_analyze_complex_script() {
        let analyzer = ScriptletAnalyzer::new();
        let scriptlet = make_scriptlet(
            r#"
if [ -f /etc/myapp/config ]; then
    cp /etc/myapp/config /etc/myapp/config.bak
fi
for file in /var/lib/myapp/*; do
    chmod 644 "$file"
done
useradd -r myapp
"#,
            ScriptletPhase::PreInstall,
        );

        let (hooks, report) = analyzer.analyze(&[scriptlet]);

        // Should detect useradd
        assert_eq!(hooks.len(), 1);

        // Should flag complex logic as uncertain
        assert!(!report.uncertain_operations.is_empty());

        // Fidelity should be degraded due to complex logic
        assert!(report.level < FidelityLevel::Full);
    }

    #[test]
    fn test_analyze_external_script() {
        let analyzer = ScriptletAnalyzer::new();
        let scriptlet = make_scriptlet(
            "/usr/local/bin/custom-setup.sh",
            ScriptletPhase::PostInstall,
        );

        let (hooks, report) = analyzer.analyze(&[scriptlet]);

        assert!(hooks.is_empty());
        assert!(!report.uncertain_operations.is_empty());
        assert!(report.uncertain_operations[0].severity == UncertaintySeverity::High);
    }

    #[test]
    fn test_build_hooks_dedup() {
        let detected = vec![
            DetectedHook::User(UserHook {
                name: "nginx".to_string(),
                system: true,
                home: None,
                shell: None,
                group: None,
            }),
            DetectedHook::User(UserHook {
                name: "nginx".to_string(),
                system: true,
                home: Some("/var/lib/nginx".to_string()),
                shell: None,
                group: None,
            }),
        ];

        let hooks = ScriptletAnalyzer::build_hooks(&detected);

        // Should deduplicate by name
        assert_eq!(hooks.users.len(), 1);
    }

    #[test]
    fn test_parse_useradd_variants() {
        let analyzer = ScriptletAnalyzer::new();

        // Test various useradd invocations
        let user1 = analyzer.parse_useradd("-r -d /home/test -s /bin/false testuser");
        assert!(user1.is_some());
        let u1 = user1.unwrap();
        assert_eq!(u1.name, "testuser");
        assert!(u1.system);
        assert_eq!(u1.home, Some("/home/test".to_string()));
        assert_eq!(u1.shell, Some("/bin/false".to_string()));

        // Test with --system flag
        let user2 = analyzer.parse_useradd("--system --home-dir /var/lib/app appuser");
        assert!(user2.is_some());
        let u2 = user2.unwrap();
        assert_eq!(u2.name, "appuser");
        assert!(u2.system);
        assert_eq!(u2.home, Some("/var/lib/app".to_string()));

        // Test minimal
        let user3 = analyzer.parse_useradd("simpleuser");
        assert!(user3.is_some());
        let u3 = user3.unwrap();
        assert_eq!(u3.name, "simpleuser");
        assert!(!u3.system);
    }

    #[test]
    fn test_no_scriptlets() {
        let analyzer = ScriptletAnalyzer::new();
        let (hooks, report) = analyzer.analyze(&[]);

        assert!(hooks.is_empty());
        assert_eq!(report.level, FidelityLevel::Full);
        assert_eq!(report.scriptlets_preserved, 0);
    }

    #[test]
    fn test_ldconfig_detection() {
        let analyzer = ScriptletAnalyzer::new();
        let scriptlet = make_scriptlet("/sbin/ldconfig", ScriptletPhase::PostInstall);

        let (_, report) = analyzer.analyze(&[scriptlet]);

        // Should detect ldconfig as a known operation
        let has_ldconfig = report
            .detected_operations
            .iter()
            .any(|op| op.operation_type == OperationType::Ldconfig);
        assert!(has_ldconfig);
    }
}
