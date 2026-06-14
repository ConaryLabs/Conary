// conary-core/src/recipe/hermetic/divergence.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostBuildRecord {
    pub package_name: String,
    pub package_version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    pub output_merkle_root: String,
    pub diagnostic_input_key: Option<String>,
    pub diagnostic_dna_hash: Option<String>,
    pub package_path: Option<String>,
    pub build_timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DivergenceReport {
    pub compared: bool,
    pub status: DivergenceStatus,
    pub diagnostics: Vec<String>,
}

impl DivergenceReport {
    pub fn no_host_record(diagnostic: impl Into<String>) -> Self {
        Self {
            compared: false,
            status: DivergenceStatus::NoHostRecord,
            diagnostics: vec![diagnostic.into()],
        }
    }

    pub fn not_compared() -> Self {
        Self::no_host_record("no host record available for comparison")
    }
}

impl Default for DivergenceReport {
    fn default() -> Self {
        Self::not_compared()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DivergenceStatus {
    NoHostRecord,
    MatchesHost,
    DiffersFromHost,
}

pub fn compare_host_record(
    host_record: Option<&HostBuildRecord>,
    hermetic_output_merkle_root: Option<&str>,
) -> DivergenceReport {
    let Some(host_record) = host_record else {
        return DivergenceReport::not_compared();
    };
    let Some(hermetic_output_merkle_root) = hermetic_output_merkle_root else {
        return DivergenceReport::no_host_record(
            "hermetic output merkle root unavailable for host comparison",
        );
    };

    if host_record.output_merkle_root == hermetic_output_merkle_root {
        return DivergenceReport {
            compared: true,
            status: DivergenceStatus::MatchesHost,
            diagnostics: Vec::new(),
        };
    }

    DivergenceReport {
        compared: true,
        status: DivergenceStatus::DiffersFromHost,
        diagnostics: vec![format!(
            "hermetic output merkle root differs from latest host build record: host={} hermetic={}",
            host_record.output_merkle_root, hermetic_output_merkle_root
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_record(output_merkle_root: &str) -> HostBuildRecord {
        HostBuildRecord {
            package_name: "pkg".to_string(),
            package_version: "1.0".to_string(),
            package_release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            output_merkle_root: output_merkle_root.to_string(),
            diagnostic_input_key: Some("sha256:input".to_string()),
            diagnostic_dna_hash: Some("sha256:dna".to_string()),
            package_path: None,
            build_timestamp: None,
        }
    }

    #[test]
    fn divergence_report_marks_missing_host_record() {
        let report = compare_host_record(None, Some("sha256:output"));

        assert_eq!(report.status, DivergenceStatus::NoHostRecord);
        assert!(!report.compared);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("host record"))
        );
    }

    #[test]
    fn divergence_report_marks_matching_output() {
        let host = host_record("sha256:same");

        let report = compare_host_record(Some(&host), Some("sha256:same"));

        assert_eq!(report.status, DivergenceStatus::MatchesHost);
        assert!(report.compared);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn divergence_report_marks_different_output() {
        let host = host_record("sha256:host");

        let report = compare_host_record(Some(&host), Some("sha256:hermetic"));

        assert_eq!(report.status, DivergenceStatus::DiffersFromHost);
        assert!(report.compared);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("differs"))
        );
    }

    #[test]
    fn divergence_report_marks_absent_hermetic_output_as_uncompared() {
        let host = host_record("sha256:host");

        let report = compare_host_record(Some(&host), None);

        assert_eq!(report.status, DivergenceStatus::NoHostRecord);
        assert!(!report.compared);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("merkle"))
        );
    }

    #[test]
    fn divergence_report_ignores_diagnostic_dna_hash_for_decision() {
        let mut host = host_record("sha256:same");
        host.diagnostic_dna_hash = Some("sha256:host-dna".to_string());

        let report = compare_host_record(Some(&host), Some("sha256:same"));

        assert_eq!(report.status, DivergenceStatus::MatchesHost);
        assert!(report.compared);
    }
}
