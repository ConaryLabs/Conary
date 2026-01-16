// src/ccs/convert/fidelity.rs
//! Conversion fidelity tracking and reporting
//!
//! Tracks how well a legacy package conversion preserves the original semantics.
//! Used to warn users when complex scriptlets could not be fully analyzed.

use serde::{Deserialize, Serialize};

/// Level of conversion fidelity
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FidelityLevel {
    /// All operations fully understood and converted to declarative hooks
    Full = 4,
    /// Most operations understood, minor uncertainties
    High = 3,
    /// Significant operations could not be analyzed
    Partial = 2,
    /// Most scriptlet operations are opaque
    Low = 1,
}

impl FidelityLevel {
    /// Get a human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Full => "All scriptlet operations fully converted to declarative hooks",
            Self::High => "Most scriptlet operations converted, original scripts preserved",
            Self::Partial => "Some scriptlet operations could not be analyzed",
            Self::Low => "Complex scriptlets - declarative extraction limited",
        }
    }

    /// Check if this level requires a warning to the user
    pub fn requires_warning(&self) -> bool {
        matches!(self, Self::Partial | Self::Low)
    }
}

impl std::fmt::Display for FidelityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "full"),
            Self::High => write!(f, "high"),
            Self::Partial => write!(f, "partial"),
            Self::Low => write!(f, "low"),
        }
    }
}

impl std::str::FromStr for FidelityLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "full" => Ok(Self::Full),
            "high" => Ok(Self::High),
            "partial" => Ok(Self::Partial),
            "low" => Ok(Self::Low),
            _ => Err(format!("Unknown fidelity level: {}", s)),
        }
    }
}

/// Detailed report of conversion fidelity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FidelityReport {
    /// Overall fidelity level
    pub level: FidelityLevel,

    /// Number of declarative hooks extracted
    pub hooks_extracted: usize,

    /// Number of scriptlets preserved (run as-is after hooks)
    pub scriptlets_preserved: usize,

    /// Operations that were detected and converted to hooks
    pub detected_operations: Vec<DetectedOperation>,

    /// Operations that could not be analyzed (uncertain semantics)
    pub uncertain_operations: Vec<UncertainOperation>,

    /// Warnings about the conversion
    pub warnings: Vec<String>,
}

impl FidelityReport {
    /// Create a new empty report
    pub fn new() -> Self {
        Self {
            level: FidelityLevel::Full,
            hooks_extracted: 0,
            scriptlets_preserved: 0,
            detected_operations: Vec::new(),
            uncertain_operations: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Add a detected operation
    pub fn add_detected(&mut self, op: DetectedOperation) {
        self.detected_operations.push(op);
        self.hooks_extracted += 1;
    }

    /// Add an uncertain operation
    pub fn add_uncertain(&mut self, op: UncertainOperation) {
        self.uncertain_operations.push(op);
        // Degrade fidelity level based on uncertainty
        self.recalculate_level();
    }

    /// Add a warning message
    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    /// Mark that a scriptlet was preserved
    pub fn mark_scriptlet_preserved(&mut self) {
        self.scriptlets_preserved += 1;
    }

    /// Recalculate the overall fidelity level
    pub fn recalculate_level(&mut self) {
        let detected = self.detected_operations.len();
        let uncertain = self.uncertain_operations.len();

        // Calculate ratio of detected vs uncertain
        self.level = if uncertain == 0 && detected > 0 {
            FidelityLevel::Full
        } else if uncertain == 0 {
            // No scriptlets at all
            FidelityLevel::Full
        } else {
            // Count critical uncertainties (subprocesses, complex logic)
            let critical_uncertain = self
                .uncertain_operations
                .iter()
                .filter(|u| u.severity == UncertaintySeverity::High)
                .count();

            if critical_uncertain == 0 && uncertain <= detected {
                FidelityLevel::High
            } else if critical_uncertain <= 2 && detected >= uncertain {
                FidelityLevel::Partial
            } else {
                FidelityLevel::Low
            }
        };
    }

    /// Get a summary string for display
    pub fn summary(&self) -> String {
        format!(
            "Fidelity: {} ({} hooks extracted, {} scriptlets preserved, {} uncertainties)",
            self.level,
            self.hooks_extracted,
            self.scriptlets_preserved,
            self.uncertain_operations.len()
        )
    }

    /// Serialize to JSON for database storage
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

impl Default for FidelityReport {
    fn default() -> Self {
        Self::new()
    }
}

/// An operation that was detected and can be converted to a declarative hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedOperation {
    /// Type of operation
    pub operation_type: OperationType,
    /// Phase where this was detected (pre-install, post-install, etc.)
    pub phase: String,
    /// Extracted parameters
    pub parameters: std::collections::HashMap<String, String>,
    /// Original line(s) from the scriptlet
    pub source_lines: Vec<String>,
}

/// Types of operations we can detect and convert
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationType {
    /// User creation (useradd)
    UserAdd,
    /// Group creation (groupadd)
    GroupAdd,
    /// Directory creation (mkdir, install -d)
    DirectoryCreate,
    /// Systemd unit enable/disable
    SystemdEnable,
    /// Systemd daemon-reload
    SystemdReload,
    /// Ldconfig (library cache update)
    Ldconfig,
    /// File permission change
    Chmod,
    /// File ownership change
    Chown,
    /// Update alternatives
    UpdateAlternatives,
    /// Fc-cache (font cache)
    FcCache,
    /// GTK/icon cache updates
    GtkCache,
    /// MIME database update
    MimeUpdate,
    /// Desktop database update
    DesktopUpdate,
    /// GSettings schema compile
    GlibCompileSchemas,
    /// sysctl setting
    Sysctl,
    /// tmpfiles create
    Tmpfiles,
}

impl std::fmt::Display for OperationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserAdd => write!(f, "useradd"),
            Self::GroupAdd => write!(f, "groupadd"),
            Self::DirectoryCreate => write!(f, "mkdir"),
            Self::SystemdEnable => write!(f, "systemctl-enable"),
            Self::SystemdReload => write!(f, "systemd-reload"),
            Self::Ldconfig => write!(f, "ldconfig"),
            Self::Chmod => write!(f, "chmod"),
            Self::Chown => write!(f, "chown"),
            Self::UpdateAlternatives => write!(f, "update-alternatives"),
            Self::FcCache => write!(f, "fc-cache"),
            Self::GtkCache => write!(f, "gtk-cache"),
            Self::MimeUpdate => write!(f, "mime-update"),
            Self::DesktopUpdate => write!(f, "desktop-update"),
            Self::GlibCompileSchemas => write!(f, "glib-compile-schemas"),
            Self::Sysctl => write!(f, "sysctl"),
            Self::Tmpfiles => write!(f, "tmpfiles"),
        }
    }
}

/// An operation that could not be fully analyzed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UncertainOperation {
    /// Description of the uncertain operation
    pub description: String,
    /// Why it couldn't be analyzed
    pub reason: String,
    /// Severity of the uncertainty
    pub severity: UncertaintySeverity,
    /// Original line(s) from the scriptlet
    pub source_lines: Vec<String>,
}

/// Severity of uncertain operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UncertaintySeverity {
    /// Low: Probably safe (e.g., echo statements, comments)
    Low,
    /// Medium: Potentially significant (e.g., unknown command)
    Medium,
    /// High: Likely critical (e.g., external script execution, complex logic)
    High,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fidelity_level_ordering() {
        assert!(FidelityLevel::Full > FidelityLevel::High);
        assert!(FidelityLevel::High > FidelityLevel::Partial);
        assert!(FidelityLevel::Partial > FidelityLevel::Low);
    }

    #[test]
    fn test_fidelity_level_parse() {
        assert_eq!("full".parse::<FidelityLevel>().unwrap(), FidelityLevel::Full);
        assert_eq!("HIGH".parse::<FidelityLevel>().unwrap(), FidelityLevel::High);
        assert_eq!(
            "partial".parse::<FidelityLevel>().unwrap(),
            FidelityLevel::Partial
        );
        assert_eq!("low".parse::<FidelityLevel>().unwrap(), FidelityLevel::Low);
    }

    #[test]
    fn test_fidelity_report_new() {
        let report = FidelityReport::new();
        assert_eq!(report.level, FidelityLevel::Full);
        assert_eq!(report.hooks_extracted, 0);
        assert!(report.detected_operations.is_empty());
    }

    #[test]
    fn test_fidelity_report_add_detected() {
        let mut report = FidelityReport::new();
        report.add_detected(DetectedOperation {
            operation_type: OperationType::UserAdd,
            phase: "pre-install".to_string(),
            parameters: [("name".to_string(), "nginx".to_string())]
                .into_iter()
                .collect(),
            source_lines: vec!["useradd nginx".to_string()],
        });

        assert_eq!(report.hooks_extracted, 1);
        assert_eq!(report.level, FidelityLevel::Full);
    }

    #[test]
    fn test_fidelity_report_add_uncertain() {
        let mut report = FidelityReport::new();
        report.add_detected(DetectedOperation {
            operation_type: OperationType::UserAdd,
            phase: "pre-install".to_string(),
            parameters: std::collections::HashMap::new(),
            source_lines: vec![],
        });
        report.add_uncertain(UncertainOperation {
            description: "Complex script logic".to_string(),
            reason: "Contains conditionals".to_string(),
            severity: UncertaintySeverity::Medium,
            source_lines: vec!["if [ -f /etc/foo ]; then ...".to_string()],
        });

        // Should be High since 1 detected >= 1 uncertain and no critical
        assert_eq!(report.level, FidelityLevel::High);
    }

    #[test]
    fn test_fidelity_report_critical_uncertain() {
        let mut report = FidelityReport::new();
        report.add_uncertain(UncertainOperation {
            description: "External script execution".to_string(),
            reason: "Calls external script".to_string(),
            severity: UncertaintySeverity::High,
            source_lines: vec!["/usr/local/bin/custom-script.sh".to_string()],
        });
        report.add_uncertain(UncertainOperation {
            description: "External script execution".to_string(),
            reason: "Calls external script".to_string(),
            severity: UncertaintySeverity::High,
            source_lines: vec!["/usr/local/bin/other-script.sh".to_string()],
        });
        report.add_uncertain(UncertainOperation {
            description: "External script execution".to_string(),
            reason: "Calls external script".to_string(),
            severity: UncertaintySeverity::High,
            source_lines: vec!["/usr/local/bin/third-script.sh".to_string()],
        });

        // 3 critical uncertainties with no detected -> Low
        assert_eq!(report.level, FidelityLevel::Low);
    }

    #[test]
    fn test_fidelity_report_json_roundtrip() {
        let mut report = FidelityReport::new();
        report.add_detected(DetectedOperation {
            operation_type: OperationType::GroupAdd,
            phase: "pre-install".to_string(),
            parameters: [("name".to_string(), "www-data".to_string())]
                .into_iter()
                .collect(),
            source_lines: vec!["groupadd www-data".to_string()],
        });
        report.add_warning("Test warning".to_string());

        let json = report.to_json().unwrap();
        let parsed = FidelityReport::from_json(&json).unwrap();

        assert_eq!(parsed.level, report.level);
        assert_eq!(parsed.hooks_extracted, report.hooks_extracted);
        assert_eq!(parsed.warnings, report.warnings);
    }

    #[test]
    fn test_requires_warning() {
        assert!(!FidelityLevel::Full.requires_warning());
        assert!(!FidelityLevel::High.requires_warning());
        assert!(FidelityLevel::Partial.requires_warning());
        assert!(FidelityLevel::Low.requires_warning());
    }
}
