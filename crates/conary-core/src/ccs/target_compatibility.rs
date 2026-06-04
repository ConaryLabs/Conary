// conary-core/src/ccs/target_compatibility.rs
//! Target compatibility matrix for legacy scriptlet replay.

use crate::hash;
use crate::repository::distro::{ReplayTarget, replay_target_id};
use crate::scriptlet::SandboxMode;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TargetCompatibilityMatrix {
    entries: Vec<TargetCompatibilityMatrixEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCompatibilityMatrixEntry {
    pub id: String,
    pub source: TargetSelector,
    pub target: TargetSelector,
    #[serde(default)]
    pub requirements: MatrixPreflightRequirements,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetSelector {
    pub format: String,
    pub distro: String,
    pub release: TargetSelectorRelease,
    pub arch: TargetSelectorArch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetSelectorRelease {
    Exact(String),
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetSelectorArch {
    Exact(String),
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MatrixPreflightRequirements {
    #[serde(default)]
    pub required_helpers: Vec<RequiredHelper>,
    #[serde(default)]
    pub required_paths: Vec<RequiredPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_manager: Option<ServiceManagerRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_policy: Option<SecurityPolicyRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_floor: Option<SandboxMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredHelper {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredPath {
    pub id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceManagerRequirement {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityPolicyRequirement {
    pub id: String,
    pub policy: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityPreflightEnvironment {
    #[serde(default)]
    pub helpers: Vec<ObservedHelper>,
    #[serde(default)]
    pub paths: Vec<ObservedPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_manager: Option<String>,
    #[serde(default)]
    pub security_policies: Vec<String>,
    pub effective_sandbox: SandboxMode,
}

impl Default for CompatibilityPreflightEnvironment {
    fn default() -> Self {
        Self {
            helpers: Vec::new(),
            paths: Vec::new(),
            service_manager: None,
            security_policies: Vec::new(),
            effective_sandbox: SandboxMode::Always,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedHelper {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedPath {
    pub path: String,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCompatibilityMatch {
    pub entry_id: String,
    pub matrix_digest: Option<String>,
    pub entry_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCompatibilityDecision {
    pub decision: CompatibilityDecisionStatus,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_digest: Option<String>,
    pub override_required: bool,
    pub override_used: bool,
    #[serde(default)]
    pub preflight_checks: Vec<CompatibilityPreflightCheck>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompatibilityDecisionStatus {
    Accepted,
    Refused,
    NativeFree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityPreflightCheck {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub reason_code: String,
}

impl TargetCompatibilityMatrix {
    pub fn new(entries: Vec<TargetCompatibilityMatrixEntry>) -> Result<Self> {
        let mut entries = entries;
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        validate_entries(&entries)?;
        Ok(Self { entries })
    }

    #[must_use]
    pub fn production_default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn for_testing(entries: Vec<TargetCompatibilityMatrixEntry>) -> Self {
        Self::new(entries).expect("test compatibility matrix must be valid")
    }

    #[cfg(test)]
    pub(crate) fn unchecked_for_ambiguous_test(
        entries: Vec<TargetCompatibilityMatrixEntry>,
    ) -> Self {
        Self { entries }
    }

    #[must_use]
    pub fn entries(&self) -> &[TargetCompatibilityMatrixEntry] {
        &self.entries
    }

    #[must_use]
    pub fn digest(&self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let json = serde_json::to_vec(&self.entries).expect("matrix entries serialize");
        Some(hash::sha256_prefixed(&json))
    }

    pub fn match_entry(
        &self,
        source: &ReplayTarget<'_>,
        target: &ReplayTarget<'_>,
    ) -> Result<Option<TargetCompatibilityMatch>> {
        let mut matches: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.source.matches(source) && entry.target.matches(target))
            .collect();
        if matches.is_empty() {
            return Ok(None);
        }
        matches.sort_by_key(|entry| Reverse(entry.specificity()));
        let top = matches[0].specificity();
        if matches
            .iter()
            .filter(|entry| entry.specificity() == top)
            .count()
            > 1
        {
            return Err(anyhow!(
                "compatibility-matrix-entry-ambiguous: {} -> {}",
                replay_target_id(source),
                replay_target_id(target)
            ));
        }
        let entry = matches[0];
        Ok(Some(TargetCompatibilityMatch {
            entry_id: entry.id.clone(),
            matrix_digest: self.digest(),
            entry_digest: entry.digest.clone(),
        }))
    }

    #[must_use]
    pub fn preflight_entry(
        &self,
        matched: &TargetCompatibilityMatch,
        env: &CompatibilityPreflightEnvironment,
    ) -> TargetCompatibilityDecision {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.id == matched.entry_id)
        else {
            return TargetCompatibilityDecision::refused(
                "compatibility-matrix-entry-missing",
                Some(matched.entry_id.clone()),
                matched.matrix_digest.clone(),
                Vec::new(),
            );
        };
        preflight_requirements(entry, matched, env)
    }
}

impl TargetCompatibilityMatrixEntry {
    fn specificity(&self) -> usize {
        self.source.specificity() + self.target.specificity()
    }
}

impl TargetSelector {
    fn matches(&self, target: &ReplayTarget<'_>) -> bool {
        self.format == target.format
            && self.distro == target.distro
            && self.release.matches(target.release)
            && self.arch.matches(target.arch)
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.format == other.format
            && self.distro == other.distro
            && self.release.overlaps(&other.release)
            && self.arch.overlaps(&other.arch)
    }

    fn specificity(&self) -> usize {
        self.release.specificity() + self.arch.specificity()
    }
}

impl TargetSelectorRelease {
    fn matches(&self, value: &str) -> bool {
        matches!(self, Self::Any) || matches!(self, Self::Exact(exact) if exact == value)
    }

    fn overlaps(&self, other: &Self) -> bool {
        matches!(self, Self::Any)
            || matches!(other, Self::Any)
            || matches!((self, other), (Self::Exact(left), Self::Exact(right)) if left == right)
    }

    fn specificity(&self) -> usize {
        usize::from(matches!(self, Self::Exact(_)))
    }
}

impl TargetSelectorArch {
    fn matches(&self, value: &str) -> bool {
        matches!(self, Self::Any) || matches!(self, Self::Exact(exact) if exact == value)
    }

    fn overlaps(&self, other: &Self) -> bool {
        matches!(self, Self::Any)
            || matches!(other, Self::Any)
            || matches!((self, other), (Self::Exact(left), Self::Exact(right)) if left == right)
    }

    fn specificity(&self) -> usize {
        usize::from(matches!(self, Self::Exact(_)))
    }
}

fn validate_entries(entries: &[TargetCompatibilityMatrixEntry]) -> Result<()> {
    let mut ids = BTreeSet::new();
    for entry in entries {
        if entry.id.trim().is_empty() {
            return Err(anyhow!("matrix entry id must not be empty"));
        }
        if !ids.insert(entry.id.clone()) {
            return Err(anyhow!("duplicate matrix entry id: {}", entry.id));
        }
    }

    for (left_index, left) in entries.iter().enumerate() {
        for right in entries.iter().skip(left_index + 1) {
            if left.source.overlaps(&right.source)
                && left.target.overlaps(&right.target)
                && left.specificity() == right.specificity()
            {
                return Err(anyhow!(
                    "ambiguous matrix entries {} and {} have the same specificity",
                    left.id,
                    right.id
                ));
            }
        }
    }
    Ok(())
}

impl TargetCompatibilityDecision {
    #[must_use]
    pub fn accepted(
        reason_code: impl Into<String>,
        matrix_entry_id: Option<String>,
        matrix_digest: Option<String>,
        override_required: bool,
        override_used: bool,
        preflight_checks: Vec<CompatibilityPreflightCheck>,
    ) -> Self {
        Self {
            decision: CompatibilityDecisionStatus::Accepted,
            reason_code: reason_code.into(),
            matrix_entry_id,
            matrix_digest,
            override_required,
            override_used,
            preflight_checks,
        }
    }

    #[must_use]
    pub fn refused(
        reason_code: impl Into<String>,
        matrix_entry_id: Option<String>,
        matrix_digest: Option<String>,
        preflight_checks: Vec<CompatibilityPreflightCheck>,
    ) -> Self {
        Self {
            decision: CompatibilityDecisionStatus::Refused,
            reason_code: reason_code.into(),
            matrix_entry_id,
            matrix_digest,
            override_required: false,
            override_used: false,
            preflight_checks,
        }
    }

    #[must_use]
    pub fn native_free(reason_code: impl Into<String>) -> Self {
        Self {
            decision: CompatibilityDecisionStatus::NativeFree,
            reason_code: reason_code.into(),
            matrix_entry_id: None,
            matrix_digest: None,
            override_required: false,
            override_used: false,
            preflight_checks: Vec::new(),
        }
    }
}

fn preflight_requirements(
    entry: &TargetCompatibilityMatrixEntry,
    matched: &TargetCompatibilityMatch,
    env: &CompatibilityPreflightEnvironment,
) -> TargetCompatibilityDecision {
    let mut checks = Vec::new();

    for helper in &entry.requirements.required_helpers {
        let observed = env.helpers.iter().find(|item| item.name == helper.name);
        let Some(observed) = observed else {
            checks.push(failed_check(
                &helper.id,
                "helper",
                "compatibility-helper-missing",
            ));
            return TargetCompatibilityDecision::refused(
                "compatibility-helper-missing",
                Some(entry.id.clone()),
                matched.matrix_digest.clone(),
                checks,
            );
        };
        if let Some(expected) = &helper.exact_version {
            let Some(actual) = observed.version.as_ref() else {
                checks.push(failed_check(
                    &helper.id,
                    "helper",
                    "compatibility-helper-version-missing",
                ));
                return TargetCompatibilityDecision::refused(
                    "compatibility-helper-version-missing",
                    Some(entry.id.clone()),
                    matched.matrix_digest.clone(),
                    checks,
                );
            };
            if actual != expected {
                checks.push(failed_check(
                    &helper.id,
                    "helper",
                    "compatibility-helper-version-unsupported",
                ));
                return TargetCompatibilityDecision::refused(
                    "compatibility-helper-version-unsupported",
                    Some(entry.id.clone()),
                    matched.matrix_digest.clone(),
                    checks,
                );
            }
        }
        checks.push(passed_check(
            &helper.id,
            "helper",
            "compatibility-helper-present",
        ));
    }

    for required_path in &entry.requirements.required_paths {
        let present = env
            .paths
            .iter()
            .any(|path| path.path == required_path.path && path.present);
        if !present {
            checks.push(failed_check(
                &required_path.id,
                "path",
                "compatibility-path-missing",
            ));
            return TargetCompatibilityDecision::refused(
                "compatibility-path-missing",
                Some(entry.id.clone()),
                matched.matrix_digest.clone(),
                checks,
            );
        }
        checks.push(passed_check(
            &required_path.id,
            "path",
            "compatibility-path-present",
        ));
    }

    if let Some(required) = &entry.requirements.service_manager
        && env.service_manager.as_deref() != Some(required.name.as_str())
    {
        checks.push(failed_check(
            &required.id,
            "service-manager",
            "compatibility-service-manager-mismatch",
        ));
        return TargetCompatibilityDecision::refused(
            "compatibility-service-manager-mismatch",
            Some(entry.id.clone()),
            matched.matrix_digest.clone(),
            checks,
        );
    }

    if let Some(required) = &entry.requirements.security_policy
        && !env
            .security_policies
            .iter()
            .any(|policy| policy == &required.policy)
    {
        checks.push(failed_check(
            &required.id,
            "security-policy",
            "compatibility-security-policy-unsupported",
        ));
        return TargetCompatibilityDecision::refused(
            "compatibility-security-policy-unsupported",
            Some(entry.id.clone()),
            matched.matrix_digest.clone(),
            checks,
        );
    }

    if let Some(floor) = entry.requirements.sandbox_floor
        && !sandbox_satisfies(env.effective_sandbox, floor)
    {
        checks.push(failed_check(
            "sandbox-floor",
            "sandbox",
            "compatibility-sandbox-floor-unsupported",
        ));
        return TargetCompatibilityDecision::refused(
            "compatibility-sandbox-floor-unsupported",
            Some(entry.id.clone()),
            matched.matrix_digest.clone(),
            checks,
        );
    }

    TargetCompatibilityDecision::accepted(
        "compatibility-matrix-entry-accepted",
        Some(entry.id.clone()),
        matched.matrix_digest.clone(),
        false,
        false,
        checks,
    )
}

fn passed_check(id: &str, kind: &str, reason_code: &str) -> CompatibilityPreflightCheck {
    CompatibilityPreflightCheck {
        id: id.to_string(),
        kind: kind.to_string(),
        status: "passed".to_string(),
        reason_code: reason_code.to_string(),
    }
}

fn failed_check(id: &str, kind: &str, reason_code: &str) -> CompatibilityPreflightCheck {
    CompatibilityPreflightCheck {
        id: id.to_string(),
        kind: kind.to_string(),
        status: "failed".to_string(),
        reason_code: reason_code.to_string(),
    }
}

fn sandbox_satisfies(actual: SandboxMode, required: SandboxMode) -> bool {
    fn rank(mode: SandboxMode) -> u8 {
        match mode {
            SandboxMode::None => 0,
            SandboxMode::Auto => 1,
            SandboxMode::Always => 2,
        }
    }

    rank(actual) >= rank(required)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::target_compatibility::{
        ObservedHelper, ObservedPath, RequiredPath, SecurityPolicyRequirement,
        ServiceManagerRequirement,
    };
    use crate::repository::distro::ReplayTarget;
    use crate::scriptlet::SandboxMode;

    fn fedora45() -> ReplayTarget<'static> {
        ReplayTarget {
            format: "rpm",
            distro: "fedora",
            release: "45",
            arch: "x86_64",
        }
    }

    fn fedora44() -> ReplayTarget<'static> {
        ReplayTarget {
            format: "rpm",
            distro: "fedora",
            release: "44",
            arch: "x86_64",
        }
    }

    fn fedora_entry(id: &str) -> TargetCompatibilityMatrixEntry {
        TargetCompatibilityMatrixEntry {
            id: id.to_string(),
            source: TargetSelector {
                format: "rpm".to_string(),
                distro: "fedora".to_string(),
                release: TargetSelectorRelease::Exact("45".to_string()),
                arch: TargetSelectorArch::Exact("x86_64".to_string()),
            },
            target: TargetSelector {
                format: "rpm".to_string(),
                distro: "fedora".to_string(),
                release: TargetSelectorRelease::Exact("44".to_string()),
                arch: TargetSelectorArch::Exact("x86_64".to_string()),
            },
            requirements: MatrixPreflightRequirements::default(),
            digest: Some("sha256:test-entry".to_string()),
            rationale: "synthetic test entry".to_string(),
        }
    }

    #[test]
    fn production_default_matrix_is_empty() {
        let matrix = TargetCompatibilityMatrix::production_default();
        assert!(matrix.entries().is_empty());
    }

    #[test]
    fn exact_matrix_entry_matches_source_and_target() {
        let matrix = TargetCompatibilityMatrix::for_testing(vec![fedora_entry("fedora45-to-44")]);
        let matched = matrix
            .match_entry(&fedora45(), &fedora44())
            .expect("lookup succeeds")
            .expect("entry matches");

        assert_eq!(matched.entry_id, "fedora45-to-44");
        assert_eq!(matched.matrix_digest, matrix.digest());
    }

    #[test]
    fn missing_entry_returns_none() {
        let matrix = TargetCompatibilityMatrix::production_default();
        let matched = matrix
            .match_entry(&fedora45(), &fedora44())
            .expect("lookup succeeds");

        assert!(matched.is_none());
    }

    #[test]
    fn constructor_rejects_duplicate_ids() {
        let first = fedora_entry("duplicate");
        let second = fedora_entry("duplicate");
        let error = TargetCompatibilityMatrix::new(vec![first, second]).expect_err("rejects");

        assert!(error.to_string().contains("duplicate matrix entry id"));
    }

    #[test]
    fn constructor_rejects_same_specificity_overlaps() {
        let mut first = fedora_entry("first");
        first.source.arch = TargetSelectorArch::Any;
        let mut second = fedora_entry("second");
        second.source.arch = TargetSelectorArch::Any;

        let error =
            TargetCompatibilityMatrix::new(vec![first, second]).expect_err("rejects overlap");

        assert!(error.to_string().contains("ambiguous matrix entries"));
    }

    #[test]
    fn unchecked_runtime_ambiguity_returns_error() {
        let first = fedora_entry("first");
        let second = fedora_entry("second");
        let matrix = TargetCompatibilityMatrix::unchecked_for_ambiguous_test(vec![first, second]);

        let error = matrix
            .match_entry(&fedora45(), &fedora44())
            .expect_err("unchecked ambiguous matrix refuses");

        assert!(
            error
                .to_string()
                .contains("compatibility-matrix-entry-ambiguous")
        );
    }

    #[test]
    fn missing_helper_requirement_fails_preflight() {
        let mut entry = fedora_entry("needs-helper");
        entry.requirements.required_helpers.push(RequiredHelper {
            id: "helper-systemctl".to_string(),
            name: "systemctl".to_string(),
            exact_version: None,
        });
        let matrix = TargetCompatibilityMatrix::for_testing(vec![entry]);
        let matched = matrix
            .match_entry(&fedora45(), &fedora44())
            .expect("lookup succeeds")
            .expect("entry matches");
        let env = CompatibilityPreflightEnvironment {
            helpers: Vec::new(),
            paths: Vec::new(),
            service_manager: None,
            security_policies: Vec::new(),
            effective_sandbox: SandboxMode::Always,
        };

        let decision = matrix.preflight_entry(&matched, &env);

        assert_eq!(decision.decision, CompatibilityDecisionStatus::Refused);
        assert_eq!(decision.reason_code, "compatibility-helper-missing");
        assert_eq!(decision.preflight_checks[0].id, "helper-systemctl");
    }

    #[test]
    fn missing_helper_version_fails_preflight() {
        let mut entry = fedora_entry("needs-helper-version");
        entry.requirements.required_helpers.push(RequiredHelper {
            id: "helper-rpm".to_string(),
            name: "rpm".to_string(),
            exact_version: Some("4.20.0".to_string()),
        });
        let matrix = TargetCompatibilityMatrix::for_testing(vec![entry]);
        let matched = matrix
            .match_entry(&fedora45(), &fedora44())
            .expect("lookup succeeds")
            .expect("entry matches");
        let env = CompatibilityPreflightEnvironment {
            helpers: vec![ObservedHelper {
                name: "rpm".to_string(),
                version: None,
            }],
            paths: Vec::new(),
            service_manager: None,
            security_policies: Vec::new(),
            effective_sandbox: SandboxMode::Always,
        };

        let decision = matrix.preflight_entry(&matched, &env);

        assert_eq!(decision.reason_code, "compatibility-helper-version-missing");
    }

    #[test]
    fn unsupported_helper_version_fails_preflight() {
        let mut entry = fedora_entry("needs-helper-version");
        entry.requirements.required_helpers.push(RequiredHelper {
            id: "helper-rpm".to_string(),
            name: "rpm".to_string(),
            exact_version: Some("4.20.0".to_string()),
        });
        let matrix = TargetCompatibilityMatrix::for_testing(vec![entry]);
        let matched = matrix
            .match_entry(&fedora45(), &fedora44())
            .expect("lookup succeeds")
            .expect("entry matches");
        let env = CompatibilityPreflightEnvironment {
            helpers: vec![ObservedHelper {
                name: "rpm".to_string(),
                version: Some("4.19.0".to_string()),
            }],
            paths: Vec::new(),
            service_manager: None,
            security_policies: Vec::new(),
            effective_sandbox: SandboxMode::Always,
        };

        let decision = matrix.preflight_entry(&matched, &env);

        assert_eq!(
            decision.reason_code,
            "compatibility-helper-version-unsupported"
        );
    }

    #[test]
    fn missing_path_fails_preflight() {
        let mut entry = fedora_entry("needs-path");
        entry.requirements.required_paths.push(RequiredPath {
            id: "path-systemctl".to_string(),
            path: "/usr/bin/systemctl".to_string(),
        });
        let matrix = TargetCompatibilityMatrix::for_testing(vec![entry]);
        let matched = matrix
            .match_entry(&fedora45(), &fedora44())
            .expect("lookup succeeds")
            .expect("entry matches");
        let env = CompatibilityPreflightEnvironment {
            helpers: Vec::new(),
            paths: vec![ObservedPath {
                path: "/usr/bin/systemctl".to_string(),
                present: false,
            }],
            service_manager: None,
            security_policies: Vec::new(),
            effective_sandbox: SandboxMode::Always,
        };

        let decision = matrix.preflight_entry(&matched, &env);

        assert_eq!(decision.reason_code, "compatibility-path-missing");
    }

    #[test]
    fn service_manager_mismatch_fails_preflight() {
        let mut entry = fedora_entry("needs-systemd");
        entry.requirements.service_manager = Some(ServiceManagerRequirement {
            id: "service-manager-systemd".to_string(),
            name: "systemd".to_string(),
        });
        let matrix = TargetCompatibilityMatrix::for_testing(vec![entry]);
        let matched = matrix
            .match_entry(&fedora45(), &fedora44())
            .expect("lookup succeeds")
            .expect("entry matches");
        let env = CompatibilityPreflightEnvironment {
            helpers: Vec::new(),
            paths: Vec::new(),
            service_manager: Some("none".to_string()),
            security_policies: Vec::new(),
            effective_sandbox: SandboxMode::Always,
        };

        let decision = matrix.preflight_entry(&matched, &env);

        assert_eq!(
            decision.reason_code,
            "compatibility-service-manager-mismatch"
        );
    }

    #[test]
    fn security_policy_requirement_fails_closed() {
        let mut entry = fedora_entry("needs-selinux");
        entry.requirements.security_policy = Some(SecurityPolicyRequirement {
            id: "security-policy-selinux".to_string(),
            policy: "selinux".to_string(),
        });
        let matrix = TargetCompatibilityMatrix::for_testing(vec![entry]);
        let matched = matrix
            .match_entry(&fedora45(), &fedora44())
            .expect("lookup succeeds")
            .expect("entry matches");
        let env = CompatibilityPreflightEnvironment {
            helpers: Vec::new(),
            paths: Vec::new(),
            service_manager: None,
            security_policies: Vec::new(),
            effective_sandbox: SandboxMode::Always,
        };

        let decision = matrix.preflight_entry(&matched, &env);

        assert_eq!(
            decision.reason_code,
            "compatibility-security-policy-unsupported"
        );
    }

    #[test]
    fn sandbox_floor_mismatch_fails_preflight() {
        let mut entry = fedora_entry("needs-sandbox");
        entry.requirements.sandbox_floor = Some(SandboxMode::Always);
        let matrix = TargetCompatibilityMatrix::for_testing(vec![entry]);
        let matched = matrix
            .match_entry(&fedora45(), &fedora44())
            .expect("lookup succeeds")
            .expect("entry matches");
        let env = CompatibilityPreflightEnvironment {
            helpers: Vec::new(),
            paths: Vec::new(),
            service_manager: None,
            security_policies: Vec::new(),
            effective_sandbox: SandboxMode::None,
        };

        let decision = matrix.preflight_entry(&matched, &env);

        assert_eq!(
            decision.reason_code,
            "compatibility-sandbox-floor-unsupported"
        );
    }
}
