# Legacy Scriptlet Compatibility Matrix And Override Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Goal 7 by requiring explicit compatibility matrix authorization for `family-compatible` raw legacy scriptlet replay and recording accepted compatibility decisions in replay audit metadata.

**Architecture:** Add a pure target-compatibility matrix model in `conary-core`, thread it through the existing Goal 6 legacy replay planner, and keep production behavior conservative with an empty matrix. CLI integration resolves the real host replay target from the distro pin instead of reusing the bundle source target, then passes that host target plus the production matrix into install, remove, update, restore, batch, autoremove, and rollback preflight points. Accepted operations extend the existing changeset metadata object; refused operations still fail before mutation.

**Tech Stack:** Rust workspace, `conary-core`, `apps/conary`, `rusqlite`, `serde`, `serde_json`, existing CCS legacy scriptlet bundle and changeset metadata models.

---

## Source Documents

- `AGENTS.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `docs/superpowers/specs/2026-06-02-legacy-scriptlet-compatibility-matrix-override-audit-design.md`
- `docs/superpowers/specs/archive/2026-05-31-legacy-scriptlet-safe-replay-engine-design.md`
- `docs/superpowers/plans/archive/2026-05-31-legacy-scriptlet-safe-replay-engine-plan.md`

## File Map

- Create: `crates/conary-core/src/ccs/target_compatibility.rs`
  - Pure matrix model, selectors, shallow preflight requirements, preflight environment, match decisions, validation, deterministic digest.
- Modify: `crates/conary-core/src/ccs/mod.rs`
  - Register and re-export the new matrix module.
- Modify: `crates/conary-core/src/scriptlet/mod.rs`
  - Add serde support for `SandboxMode` so matrix requirements can serialize.
- Modify: `crates/conary-core/src/ccs/legacy_replay.rs`
  - Add compatibility fields to `LegacyReplayPolicyInput` and `LegacyReplayPlan`, add matrix-specific refusal kinds, and wire matrix evaluation into `plan_legacy_replay()`.
- Create: `apps/conary/src/commands/legacy_replay_policy.rs`
  - CLI-side host target resolution, host policy resolution, production matrix/environment construction, and debug-test matrix injection.
- Modify: `apps/conary/src/commands/mod.rs`
  - Register the new commands-level helper module and re-export audit types as needed.
- Modify: `apps/conary/src/commands/install/mod.rs`
  - Use resolved host target policy for fresh install and old-upgrade bundle planning, thread compatibility audit decisions into install metadata.
- Modify: `apps/conary/src/commands/remove.rs`
  - Use resolved host target policy for installed-bundle remove planning, thread compatibility audit decisions into remove metadata.
- Modify: `apps/conary/src/commands/system.rs`
  - Use resolved host target policy for rollback fail-closed checks.
- Modify: `apps/conary/src/commands/install/batch.rs`
  - Repair `LegacyReplayPlan` test literals and batch planning helpers after the new plan field lands.
- Modify: `apps/conary/src/commands/install/restore.rs`
  - Update policy input and plan constructor call sites.
- Modify: `apps/conary/src/commands/update.rs`
  - Update `plan_ccs_fresh_install_legacy_replay(...)` call sites after the helper gains `conn`.
- Modify: `apps/conary/src/commands/changeset_metadata.rs`
  - Add compatibility audit metadata with serde defaults and backward-compat tests.
- Modify: `apps/conary/tests/foreign_replay.rs`
  - Inject synthetic same-family matrix entries when tests intend to exercise host foreign replay policy.
- Modify: `apps/conary/tests/bundle_replay.rs`
  - Add missing-matrix and accepted-matrix end-to-end coverage through a test-only matrix injection seam.
- Modify: `docs/modules/ccs.md` or `docs/modules/source-selection.md`
  - State that converted CCS packages are not automatically raw-scriptlet portable.

## Invariants

- Production matrix is empty in Goal 7.
- `LegacyReplayPolicyInput.target` is the host target, never the bundle source target.
- No public CLI flag or release-build policy path injects matrix entries.
- Core planner performs no host filesystem, PATH, service manager, SELinux, or AppArmor discovery.
- `--no-scripts` and disabled `--allow-legacy-replay` continue to refuse before matrix/foreign override acceptance for selected raw replay entries.
- `family-compatible` raw replay with both replay flags enabled refuses with `compatibility-matrix-entry-missing` unless a matrix entry matches.
- Refused operations do not create changesets solely for audit.

---

### Task 1: Core Target Compatibility Matrix

**Files:**
- Create: `crates/conary-core/src/ccs/target_compatibility.rs`
- Modify: `crates/conary-core/src/ccs/mod.rs`
- Modify: `crates/conary-core/src/scriptlet/mod.rs`

- [ ] **Step 1: Add failing matrix validation and preflight tests**

Create `crates/conary-core/src/ccs/target_compatibility.rs` with the path comment and a test module. Start with tests that describe the public behavior before adding the full implementation.

```rust
// conary-core/src/ccs/target_compatibility.rs
//! Target compatibility matrix for legacy scriptlet replay.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::distro::ReplayTarget;
    use crate::scriptlet::SandboxMode;
    use crate::ccs::target_compatibility::{
        ObservedHelper, ObservedPath, RequiredPath, SecurityPolicyRequirement,
        ServiceManagerRequirement,
    };

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

        assert!(error.to_string().contains("compatibility-matrix-entry-ambiguous"));
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
```

- [ ] **Step 2: Run the new tests and confirm they fail to compile**

Run:

```bash
cargo test -p conary-core target_compatibility
```

Expected: compile fails because `TargetCompatibilityMatrix`, selectors, requirements, and environment types do not exist yet.

- [ ] **Step 3: Add serde support for `SandboxMode`**

Modify `crates/conary-core/src/scriptlet/mod.rs`:

```rust
use serde::{Deserialize, Serialize};
```

Update the enum derive:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    #[serde(rename = "never", alias = "none")]
    None,
    Auto,
    #[default]
    Always,
}
```

This supports matrix JSON for `sandbox_floor` while keeping the existing
`SandboxMode::parse(...)` and `SandboxMode::as_str(...)` behavior unchanged.

Add a serde round-trip test in the existing `#[cfg(test)]` module in
`crates/conary-core/src/scriptlet/mod.rs`:

```rust
#[test]
fn sandbox_mode_serde_round_trips_goal7_matrix_spellings() {
    assert_eq!(
        serde_json::from_str::<SandboxMode>("\"never\"").expect("never deserializes"),
        SandboxMode::None
    );
    assert_eq!(
        serde_json::from_str::<SandboxMode>("\"none\"").expect("none alias deserializes"),
        SandboxMode::None
    );
    assert_eq!(
        serde_json::from_str::<SandboxMode>("\"auto\"").expect("auto deserializes"),
        SandboxMode::Auto
    );
    assert_eq!(
        serde_json::from_str::<SandboxMode>("\"always\"").expect("always deserializes"),
        SandboxMode::Always
    );
    assert_eq!(
        serde_json::to_string(&SandboxMode::None).expect("serialize none"),
        "\"never\""
    );
}
```

- [ ] **Step 4: Implement the matrix types and deterministic validation**

Replace the skeleton in `crates/conary-core/src/ccs/target_compatibility.rs` with the concrete model. Keep the module pure and deterministic.

```rust
// conary-core/src/ccs/target_compatibility.rs
//! Target compatibility matrix for legacy scriptlet replay.

use crate::hash;
use crate::repository::distro::{ReplayTarget, replay_target_id};
use crate::scriptlet::SandboxMode;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

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
```

Implement methods in the same file:

```rust
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
        matches.sort_by_key(|entry| std::cmp::Reverse(entry.specificity()));
        let top = matches[0].specificity();
        if matches.iter().filter(|entry| entry.specificity() == top).count() > 1 {
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
        let Some(entry) = self.entries.iter().find(|entry| entry.id == matched.entry_id) else {
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
```

Implement selector helpers and validation:

```rust
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
    let mut ids = std::collections::BTreeSet::new();
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
```

Implement preflight helpers with stable reason codes:

```rust
impl TargetCompatibilityDecision {
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
        if observed.is_none() {
            checks.push(failed_check(&helper.id, "helper", "compatibility-helper-missing"));
            return TargetCompatibilityDecision::refused(
                "compatibility-helper-missing",
                Some(entry.id.clone()),
                matched.matrix_digest.clone(),
                checks,
            );
        }
        if let Some(expected) = &helper.exact_version {
            let observed = observed.expect("checked above");
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
        checks.push(passed_check(&helper.id, "helper", "compatibility-helper-present"));
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
        && !env.security_policies.iter().any(|policy| policy == &required.policy)
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
```

- [ ] **Step 5: Register and re-export the module**

Modify `crates/conary-core/src/ccs/mod.rs`:

```rust
pub mod target_compatibility;
```

Add re-exports near the other CCS exports:

```rust
pub use target_compatibility::{
    CompatibilityDecisionStatus, CompatibilityPreflightCheck, CompatibilityPreflightEnvironment,
    MatrixPreflightRequirements, ObservedHelper, ObservedPath, RequiredHelper, RequiredPath,
    SecurityPolicyRequirement, ServiceManagerRequirement, TargetCompatibilityDecision,
    TargetCompatibilityMatch, TargetCompatibilityMatrix, TargetCompatibilityMatrixEntry,
    TargetSelector, TargetSelectorArch, TargetSelectorRelease,
};
```

- [ ] **Step 6: Run core matrix tests**

Run:

```bash
cargo test -p conary-core target_compatibility
```

Expected: all new target compatibility matrix tests pass.

- [ ] **Step 7: Commit Task 1**

```bash
git add crates/conary-core/src/ccs/mod.rs crates/conary-core/src/ccs/target_compatibility.rs crates/conary-core/src/scriptlet/mod.rs
git commit -m "feat(core): add legacy replay compatibility matrix"
```

---

### Task 2: Legacy Replay Planner Matrix Integration

**Files:**
- Modify: `crates/conary-core/src/ccs/legacy_replay.rs`

- [ ] **Step 1: Add failing planner tests for matrix-gated family compatibility**

In the `#[cfg(test)] mod tests` block of `crates/conary-core/src/ccs/legacy_replay.rs`, import the matrix types:

```rust
use crate::ccs::target_compatibility::{
    CompatibilityPreflightEnvironment, MatrixPreflightRequirements, RequiredHelper,
    TargetCompatibilityMatrix, TargetCompatibilityMatrixEntry, TargetSelector,
    TargetSelectorArch, TargetSelectorRelease,
};
```

Replace the existing `policy_input()` helper with an owned matrix/environment version:

```rust
fn policy_input() -> LegacyReplayPolicyInput<'static> {
    LegacyReplayPolicyInput {
        replay_enabled: false,
        foreign_replay_override: false,
        no_scripts: false,
        requested_sandbox_mode: SandboxMode::Always,
        host_policy: HostForeignReplayPolicy::Strict,
        target: target(),
        compatibility_matrix: TargetCompatibilityMatrix::production_default(),
        compatibility_environment: CompatibilityPreflightEnvironment::default(),
    }
}
```

Add helper functions for synthetic matrix entries:

```rust
fn fedora45_to_fedora44_entry(id: &str) -> TargetCompatibilityMatrixEntry {
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
        digest: Some("sha256:test-fedora45-to-44".to_string()),
        rationale: "synthetic planner fixture".to_string(),
    }
}

fn policy_with_fedora_matrix() -> LegacyReplayPolicyInput<'static> {
    let mut input = policy_input();
    input.compatibility_matrix =
        TargetCompatibilityMatrix::for_testing(vec![fedora45_to_fedora44_entry(
            "test-fedora45-to-fedora44",
        )]);
    input
}
```

Add these tests:

```rust
#[test]
fn family_compatible_without_matrix_entry_refuses() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.source_release = Some("45".to_string());
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;

    let mut input = policy_input();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Guarded;
    input.target = ReplayTarget {
        format: "rpm",
        distro: "fedora",
        release: "44",
        arch: "x86_64",
    };

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::CompatibilityMatrixEntryMissing,
    );
}

#[test]
fn family_compatible_with_matrix_entry_records_decision() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.source_release = Some("45".to_string());
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;

    let mut input = policy_with_fedora_matrix();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Guarded;

    let LegacyReplayPreflight::RequiresReplay(plan) = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan")
    else {
        panic!("expected accepted replay plan");
    };

    assert_eq!(plan.compatibility_decision.decision, "accepted");
    assert_eq!(
        plan.compatibility_decision.reason_code,
        "compatibility-matrix-entry-accepted"
    );
    assert_eq!(
        plan.compatibility_decision.matrix_entry_id.as_deref(),
        Some("test-fedora45-to-fedora44")
    );
    assert!(plan.compatibility_decision.override_required);
    assert!(plan.compatibility_decision.override_used);
}

#[test]
fn allowed_targets_do_not_substitute_for_family_compatible_matrix_entry() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.source_release = Some("45".to_string());
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;
    bundle
        .allowed_targets
        .push("rpm/fedora/44/x86_64".to_string());

    let mut input = policy_input();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Guarded;
    input.target = ReplayTarget {
        format: "rpm",
        distro: "fedora",
        release: "44",
        arch: "x86_64",
    };

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::CompatibilityMatrixEntryMissing,
    );
}

#[test]
fn no_scripts_refusal_still_precedes_matrix_lookup() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.source_release = Some("45".to_string());
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;

    let mut input = policy_input();
    input.replay_enabled = true;
    input.no_scripts = true;
    input.target = ReplayTarget {
        format: "rpm",
        distro: "fedora",
        release: "44",
        arch: "x86_64",
    };

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::NoScriptsWouldSkipRequiredReplay,
    );
}
```

- [ ] **Step 2: Run planner tests and confirm expected failures**

Run:

```bash
cargo test -p conary-core legacy_replay
```

Expected: compile fails for missing `compatibility_matrix`, `compatibility_environment`, `compatibility_decision`, and matrix refusal kinds.

- [ ] **Step 3: Extend planner input, plan, and refusal kinds**

Modify imports in `legacy_replay.rs`:

```rust
use crate::ccs::target_compatibility::{
    CompatibilityDecisionStatus, CompatibilityPreflightCheck, CompatibilityPreflightEnvironment,
    TargetCompatibilityDecision, TargetCompatibilityMatrix,
};
```

Extend `LegacyReplayPolicyInput`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyReplayPolicyInput<'a> {
    pub replay_enabled: bool,
    pub foreign_replay_override: bool,
    pub no_scripts: bool,
    pub requested_sandbox_mode: SandboxMode,
    pub host_policy: HostForeignReplayPolicy,
    pub target: ReplayTarget<'a>,
    pub compatibility_matrix: TargetCompatibilityMatrix,
    pub compatibility_environment: CompatibilityPreflightEnvironment,
}
```

These compatibility fields are owned intentionally. `CompatibilityPreflightEnvironment`
does not borrow host data, and owning the matrix/environment keeps the struct's
`'a` lifetime scoped to `ReplayTarget<'a>` instead of spreading borrowed matrix
lifetimes through planner and CLI helper code.

Add the plan compatibility decision type:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyReplayCompatibilityDecision {
    pub decision: String,
    pub reason_code: String,
    pub matrix_entry_id: Option<String>,
    pub matrix_digest: Option<String>,
    pub preflight_checks: Vec<CompatibilityPreflightCheck>,
    pub override_required: bool,
    pub override_used: bool,
}
```

Extend `LegacyReplayPlan`:

```rust
pub struct LegacyReplayPlan {
    pub target_id: String,
    pub source_target_id: String,
    pub bundle_evidence_digest: Option<String>,
    pub lifecycle_entries: Vec<PlannedLegacyEntry>,
    pub sandbox_floor: SandboxMode,
    pub ccs_hooks_allowed: bool,
    pub raw_replay_required: bool,
    pub compatibility_decision: LegacyReplayCompatibilityDecision,
}
```

Add refusal variants:

```rust
CompatibilityMatrixEntryMissing,
CompatibilityMatrixEntryAmbiguous,
CompatibilityHelperMissing,
CompatibilityHelperVersionMissing,
CompatibilityHelperVersionUnsupported,
CompatibilityPathMissing,
CompatibilityServiceManagerMismatch,
CompatibilitySecurityPolicyUnsupported,
CompatibilitySandboxFloorUnsupported,
```

Add a stable reason-code method:

```rust
impl LegacyReplayRefusalKind {
    #[must_use]
    pub fn reason_code(self) -> &'static str {
        match self {
            Self::ReviewEntry => "legacy-review-entry",
            Self::BlockedEntry => "legacy-blocked-entry",
            Self::UnknownDecision => "legacy-unknown-decision",
            Self::LegacyReplayFeatureDisabled => "legacy-replay-feature-disabled",
            Self::NoScriptsWouldSkipRequiredReplay => "no-scripts-would-skip-required-replay",
            Self::TargetCompatibilityReviewRequired => "target-compatibility-review-required",
            Self::TargetCompatibilityBlocked => "target-compatibility-blocked",
            Self::TargetMismatch => "target-mismatch",
            Self::ForeignReplayDeniedByBundle => "foreign-replay-denied-by-bundle",
            Self::ForeignReplayDeniedByHostPolicy => "foreign-replay-denied-by-host-policy",
            Self::ForeignReplayOverrideRequired => "foreign-replay-override-required",
            Self::SandboxRequirementUnsupported => "sandbox-requirement-unsupported",
            Self::TriggerReplayUnsupported => "trigger-replay-unsupported",
            Self::NativeArgsContractUnsupported => "native-args-contract-unsupported",
            Self::UnsatisfiedTransactionOrder => "unsatisfied-transaction-order",
            Self::RollbackReplayUnavailable => "rollback-replay-unavailable",
            Self::ReplayExecutionUnavailable => "replay-execution-unavailable",
            Self::TimeoutOutOfRange => "timeout-out-of-range",
            Self::MalformedBundle => "malformed-bundle",
            Self::CompatibilityMatrixEntryMissing => "compatibility-matrix-entry-missing",
            Self::CompatibilityMatrixEntryAmbiguous => "compatibility-matrix-entry-ambiguous",
            Self::CompatibilityHelperMissing => "compatibility-helper-missing",
            Self::CompatibilityHelperVersionMissing => "compatibility-helper-version-missing",
            Self::CompatibilityHelperVersionUnsupported => {
                "compatibility-helper-version-unsupported"
            }
            Self::CompatibilityPathMissing => "compatibility-path-missing",
            Self::CompatibilityServiceManagerMismatch => {
                "compatibility-service-manager-mismatch"
            }
            Self::CompatibilitySecurityPolicyUnsupported => {
                "compatibility-security-policy-unsupported"
            }
            Self::CompatibilitySandboxFloorUnsupported => {
                "compatibility-sandbox-floor-unsupported"
            }
        }
    }
}
```

- [ ] **Step 4: Replace target compatibility logic with decision-producing logic**

Add helper functions in `legacy_replay.rs`:

```rust
fn compatibility_decision_for_no_raw_replay() -> LegacyReplayCompatibilityDecision {
    LegacyReplayCompatibilityDecision {
        decision: "native-free".to_string(),
        reason_code: "compatibility-native-free".to_string(),
        matrix_entry_id: None,
        matrix_digest: None,
        preflight_checks: Vec::new(),
        override_required: false,
        override_used: false,
    }
}

fn compatibility_decision_from_target(
    bundle: &LegacyScriptletBundle,
    input: &LegacyReplayPolicyInput<'_>,
    target_id: &str,
    source_target_id: &str,
) -> Result<LegacyReplayCompatibilityDecision, LegacyReplayPreflight> {
    match &bundle.target_compatibility {
        TargetCompatibility::SourceNative => {
            if target_id == source_target_id
                || bundle.allowed_targets.iter().any(|allowed| allowed == target_id)
            {
                Ok(LegacyReplayCompatibilityDecision {
                    decision: "accepted".to_string(),
                    reason_code: "compatibility-source-native".to_string(),
                    matrix_entry_id: None,
                    matrix_digest: None,
                    preflight_checks: Vec::new(),
                    override_required: target_id != source_target_id,
                    override_used: input.foreign_replay_override,
                })
            } else {
                Err(refused(
                    LegacyReplayRefusalKind::TargetMismatch,
                    None,
                    format!("target {target_id} does not match source {source_target_id}"),
                ))
            }
        }
        TargetCompatibility::ConaryPortable => Ok(LegacyReplayCompatibilityDecision {
            decision: "accepted".to_string(),
            reason_code: "compatibility-conary-portable".to_string(),
            matrix_entry_id: None,
            matrix_digest: None,
            preflight_checks: Vec::new(),
            override_required: target_id != source_target_id,
            override_used: input.foreign_replay_override,
        }),
        TargetCompatibility::FamilyCompatible => {
            let source_target = source_target_from_bundle(bundle);
            let matched = input
                .compatibility_matrix
                .match_entry(&source_target.as_target(), &input.target)
                .map_err(|error| {
                    refused(
                        LegacyReplayRefusalKind::CompatibilityMatrixEntryAmbiguous,
                        None,
                        error.to_string(),
                    )
                })?;
            let Some(matched) = matched else {
                return Err(refused(
                    LegacyReplayRefusalKind::CompatibilityMatrixEntryMissing,
                    None,
                    format!(
                        "no compatibility matrix entry authorizes {source_target_id} on {target_id}"
                    ),
                ));
            };
            let decision = input
                .compatibility_matrix
                .preflight_entry(&matched, &input.compatibility_environment);
            if decision.decision == CompatibilityDecisionStatus::Accepted {
                Ok(LegacyReplayCompatibilityDecision {
                    decision: "accepted".to_string(),
                    reason_code: decision.reason_code,
                    matrix_entry_id: decision.matrix_entry_id,
                    matrix_digest: decision.matrix_digest,
                    preflight_checks: decision.preflight_checks,
                    override_required: target_id != source_target_id,
                    override_used: input.foreign_replay_override,
                })
            } else {
                Err(refusal_from_compatibility_decision(decision))
            }
        }
        TargetCompatibility::ReviewRequired => Err(refused(
            LegacyReplayRefusalKind::TargetCompatibilityReviewRequired,
            None,
            "target compatibility requires review",
        )),
        TargetCompatibility::Blocked => Err(refused(
            LegacyReplayRefusalKind::TargetCompatibilityBlocked,
            None,
            "target compatibility is blocked",
        )),
        TargetCompatibility::Unknown(value) => Err(refused(
            LegacyReplayRefusalKind::TargetCompatibilityReviewRequired,
            None,
            format!("unknown target compatibility {value}"),
        )),
    }
}
```

Add the decision-to-refusal mapper:

```rust
fn refusal_from_compatibility_decision(
    decision: TargetCompatibilityDecision,
) -> LegacyReplayPreflight {
    let kind = match decision.reason_code.as_str() {
        "compatibility-helper-missing" => LegacyReplayRefusalKind::CompatibilityHelperMissing,
        "compatibility-helper-version-missing" => {
            LegacyReplayRefusalKind::CompatibilityHelperVersionMissing
        }
        "compatibility-helper-version-unsupported" => {
            LegacyReplayRefusalKind::CompatibilityHelperVersionUnsupported
        }
        "compatibility-path-missing" => LegacyReplayRefusalKind::CompatibilityPathMissing,
        "compatibility-service-manager-mismatch" => {
            LegacyReplayRefusalKind::CompatibilityServiceManagerMismatch
        }
        "compatibility-security-policy-unsupported" => {
            LegacyReplayRefusalKind::CompatibilitySecurityPolicyUnsupported
        }
        "compatibility-sandbox-floor-unsupported" => {
            LegacyReplayRefusalKind::CompatibilitySandboxFloorUnsupported
        }
        _ => LegacyReplayRefusalKind::CompatibilityMatrixEntryMissing,
    };

    refused(
        kind,
        None,
        format!(
            "{} for matrix entry {}",
            decision.reason_code,
            decision.matrix_entry_id.as_deref().unwrap_or("unknown")
        ),
    )
}
```

Change `build_plan(...)` to accept a `compatibility_decision` argument and populate the new field:

```rust
fn build_plan(
    bundle: &LegacyScriptletBundle,
    input: &LegacyReplayPolicyInput<'_>,
    target_id: String,
    source_target_id: String,
    entries: Vec<&LegacyScriptletEntry>,
    raw_replay_required: bool,
    compatibility_decision: LegacyReplayCompatibilityDecision,
) -> LegacyReplayPlan {
    LegacyReplayPlan {
        target_id,
        source_target_id,
        bundle_evidence_digest: bundle.evidence_digest.clone(),
        lifecycle_entries: entries
            .into_iter()
            .map(|entry| PlannedLegacyEntry {
                entry_id: entry.id.clone(),
                native_slot: entry.native_slot.clone(),
                phase: entry.phase.clone(),
                timeout_ms: entry.timeout_ms,
            })
            .collect(),
        sandbox_floor: input.requested_sandbox_mode,
        ccs_hooks_allowed: !input.no_scripts,
        raw_replay_required,
        compatibility_decision,
    }
}
```

Update the `selected_legacy.is_empty()` branch to pass `compatibility_decision_for_no_raw_replay()`.

Update the raw replay branch after replay feature checks and before `foreign_replay_refusal(...)`:

```rust
let compatibility_decision =
    match compatibility_decision_from_target(bundle, input, &target_id, &source_target_id) {
        Ok(decision) => decision,
        Err(refusal) => return Ok(refusal),
    };

if let Some(refusal) = foreign_replay_refusal(bundle, input, &target_id, &source_target_id) {
    return Ok(refusal);
}

Ok(LegacyReplayPreflight::RequiresReplay(build_plan(
    bundle,
    input,
    target_id,
    source_target_id,
    selected_legacy,
    true,
    compatibility_decision,
)))
```

Remove the old `target_compatibility_refusal(...)` helper once all call sites are migrated.

- [ ] **Step 5: Update existing foreign policy tests to inject synthetic matrix entries**

In existing tests that set `bundle.target_compatibility = TargetCompatibility::FamilyCompatible` and expect `ForeignReplayDeniedByBundle`, `ForeignReplayDeniedByHostPolicy`, `ForeignReplayOverrideRequired`, or success, use `policy_with_fedora_matrix()` for the Fedora 45 to Fedora 44 target pair. For tests that change the target to CentOS 10, use the `fedora44_to_centos10_entry(...)` helper below.

For tests with a CentOS target, add:

```rust
fn fedora44_to_centos10_entry(id: &str) -> TargetCompatibilityMatrixEntry {
    let mut entry = fedora45_to_fedora44_entry(id);
    entry.source.release = TargetSelectorRelease::Exact("44".to_string());
    entry.target.distro = "centos".to_string();
    entry.target.release = TargetSelectorRelease::Exact("10".to_string());
    entry
}
```

Then set:

```rust
input.compatibility_matrix =
    TargetCompatibilityMatrix::for_testing(vec![fedora44_to_centos10_entry(
        "test-fedora44-to-centos10",
    )]);
```

- [ ] **Step 6: Run planner tests**

Run:

```bash
cargo test -p conary-core legacy_replay
cargo test -p conary-core target_compatibility
```

Expected: both suites pass.

- [ ] **Step 7: Checkpoint Task 2 without committing**

Do not commit after Task 2 by itself. Extending `LegacyReplayPolicyInput` and
`LegacyReplayPlan` intentionally breaks app call sites until Task 3 wires the
CLI policy input helper and repairs app test literals. Continue directly to
Task 3, then commit the core and CLI compile-boundary changes together.

---

### Task 3: CLI Host Target Resolution And Policy Input Builder

**Files:**
- Create: `apps/conary/src/commands/legacy_replay_policy.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/system.rs`
- Modify: `apps/conary/src/commands/install/restore.rs`
- Modify: `apps/conary/src/commands/update.rs`

- [ ] **Step 1: Add failing host-target resolution tests**

Create `apps/conary/src/commands/legacy_replay_policy.rs`:

```rust
// src/commands/legacy_replay_policy.rs
//! Legacy replay policy input construction for CLI mutation paths.

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::target_compatibility::{
        MatrixPreflightRequirements, TargetCompatibilityMatrix, TargetCompatibilityMatrixEntry,
        TargetSelector, TargetSelectorArch, TargetSelectorRelease,
    };
    use conary_core::db;
    use conary_core::db::models::DistroPin;
    use conary_core::scriptlet::SandboxMode;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn host_context_uses_current_distro_pin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("conary.db");
        db::init(&db_path).expect("init db");
        let conn = db::open(&db_path).expect("open db");
        DistroPin::set(&conn, "fedora-44", "guarded").expect("set pin");

        let context = resolve_legacy_replay_host_context(&conn).expect("resolve context");

        assert_eq!(context.target.to_id(), "rpm/fedora/44/x86_64");
        assert_eq!(context.host_policy, HostForeignReplayPolicy::Guarded);
    }

    #[test]
    fn host_context_falls_back_to_unknown_and_strict_without_pin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("conary.db");
        db::init(&db_path).expect("init db");
        let conn = db::open(&db_path).expect("open db");

        let context = resolve_legacy_replay_host_context(&conn).expect("resolve context");

        assert!(context.target.to_id().starts_with("unknown/unknown/unknown/"));
        assert_eq!(context.host_policy, HostForeignReplayPolicy::Strict);
    }

    #[test]
    fn policy_input_uses_host_target_not_source_target() {
        let context = LegacyReplayHostContext {
            target: ReplayTargetOwned {
                format: "rpm".to_string(),
                distro: "fedora".to_string(),
                release: "44".to_string(),
                arch: "x86_64".to_string(),
            },
            host_policy: HostForeignReplayPolicy::Guarded,
        };
        let options = LegacyReplayPolicyOptions {
            replay_enabled: true,
            foreign_replay_override: true,
            no_scripts: false,
            requested_sandbox_mode: SandboxMode::Always,
        };

        let input = legacy_replay_policy_input(&context, options).expect("input");

        assert_eq!(input.target.to_id(), "rpm/fedora/44/x86_64");
        assert_eq!(input.host_policy, HostForeignReplayPolicy::Guarded);
        assert!(input.compatibility_matrix.entries().is_empty());
    }

    #[test]
    fn test_matrix_env_is_ignored_without_test_marker() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let matrix_json = serde_json::to_string(&synthetic_matrix()).expect("matrix json");
        unsafe {
            std::env::remove_var("CONARY_TEST_SKIP_GENERATION_MOUNT");
            std::env::set_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON", matrix_json);
        }

        let matrix = compatibility_matrix_for_process().expect("matrix");

        unsafe {
            std::env::remove_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON");
        }
        assert!(matrix.entries().is_empty());
    }

    #[test]
    fn invalid_test_matrix_json_fails_closed() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::set_var("CONARY_TEST_SKIP_GENERATION_MOUNT", "1");
            std::env::set_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON", "{not-json");
        }

        let error = compatibility_matrix_for_process().expect_err("invalid JSON refuses");

        unsafe {
            std::env::remove_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON");
            std::env::remove_var("CONARY_TEST_SKIP_GENERATION_MOUNT");
        }
        assert!(error.to_string().contains("parse CONARY_TEST_COMPATIBILITY_MATRIX_JSON"));
    }

    #[test]
    fn invalid_test_matrix_entries_fail_closed() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let duplicate_json = r#"{
            "entries": [
                {
                    "id": "duplicate",
                    "source": {
                        "format": "rpm",
                        "distro": "fedora",
                        "release": {"exact": "44"},
                        "arch": {"exact": "x86_64"}
                    },
                    "target": {
                        "format": "rpm",
                        "distro": "fedora",
                        "release": {"exact": "44"},
                        "arch": {"exact": "x86_64"}
                    },
                    "requirements": {},
                    "digest": null,
                    "rationale": "first"
                },
                {
                    "id": "duplicate",
                    "source": {
                        "format": "rpm",
                        "distro": "fedora",
                        "release": {"exact": "44"},
                        "arch": {"exact": "x86_64"}
                    },
                    "target": {
                        "format": "rpm",
                        "distro": "fedora",
                        "release": {"exact": "44"},
                        "arch": {"exact": "x86_64"}
                    },
                    "requirements": {},
                    "digest": null,
                    "rationale": "second"
                }
            ]
        }"#;
        unsafe {
            std::env::set_var("CONARY_TEST_SKIP_GENERATION_MOUNT", "1");
            std::env::set_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON", duplicate_json);
        }

        let error = compatibility_matrix_for_process().expect_err("invalid matrix refuses");

        unsafe {
            std::env::remove_var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON");
            std::env::remove_var("CONARY_TEST_SKIP_GENERATION_MOUNT");
        }
        assert!(error.to_string().contains("validate CONARY_TEST_COMPATIBILITY_MATRIX_JSON"));
    }

    fn synthetic_matrix() -> TargetCompatibilityMatrix {
        TargetCompatibilityMatrix::for_testing(vec![TargetCompatibilityMatrixEntry {
            id: "test-fedora44-to-fedora44".to_string(),
            source: TargetSelector {
                format: "rpm".to_string(),
                distro: "fedora".to_string(),
                release: TargetSelectorRelease::Exact("44".to_string()),
                arch: TargetSelectorArch::Exact("x86_64".to_string()),
            },
            target: TargetSelector {
                format: "rpm".to_string(),
                distro: "fedora".to_string(),
                release: TargetSelectorRelease::Exact("44".to_string()),
                arch: TargetSelectorArch::Exact("x86_64".to_string()),
            },
            requirements: MatrixPreflightRequirements::default(),
            digest: Some("sha256:test".to_string()),
            rationale: "synthetic env fixture".to_string(),
        }])
    }
}
```

- [ ] **Step 2: Run the host policy tests and confirm they fail**

Run:

```bash
cargo test -p conary --bin conary legacy_replay_policy
```

Expected: compile fails because the new module is not registered and helper types do not exist yet.

- [ ] **Step 3: Implement the CLI helper module**

Implement `apps/conary/src/commands/legacy_replay_policy.rs`:

```rust
// src/commands/legacy_replay_policy.rs
//! Legacy replay policy input construction for CLI mutation paths.

use anyhow::{Context, Result};
use conary_core::ccs::legacy_replay::{HostForeignReplayPolicy, LegacyReplayPolicyInput};
use conary_core::ccs::target_compatibility::{
    CompatibilityPreflightEnvironment, TargetCompatibilityMatrix,
};
use conary_core::db::models::DistroPin;
use conary_core::repository::distro::{ReplayTargetOwned, replay_target_from_distro_id};
use conary_core::scriptlet::SandboxMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegacyReplayHostContext {
    pub target: ReplayTargetOwned,
    pub host_policy: HostForeignReplayPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LegacyReplayPolicyOptions {
    pub replay_enabled: bool,
    pub foreign_replay_override: bool,
    pub no_scripts: bool,
    pub requested_sandbox_mode: SandboxMode,
}

pub(crate) fn resolve_legacy_replay_host_context(
    conn: &rusqlite::Connection,
) -> Result<LegacyReplayHostContext> {
    let pin = DistroPin::get_current(conn).context("load current distro pin")?;
    let arch = std::env::consts::ARCH;
    let target = pin
        .as_ref()
        .and_then(|pin| replay_target_from_distro_id(&pin.distro, arch))
        .unwrap_or_else(|| ReplayTargetOwned {
            format: "unknown".to_string(),
            distro: "unknown".to_string(),
            release: "unknown".to_string(),
            arch: arch.to_string(),
        });
    let host_policy = host_foreign_replay_policy_from_pin(pin.as_ref());

    Ok(LegacyReplayHostContext {
        target,
        host_policy,
    })
}

pub(crate) fn host_foreign_replay_policy_from_pin(
    pin: Option<&DistroPin>,
) -> HostForeignReplayPolicy {
    match pin.map(|pin| pin.mixing_policy.as_str()) {
        Some("guarded") => HostForeignReplayPolicy::Guarded,
        Some("permissive") => HostForeignReplayPolicy::Permissive,
        _ => HostForeignReplayPolicy::Strict,
    }
}

pub(crate) fn legacy_replay_policy_input<'a>(
    context: &'a LegacyReplayHostContext,
    options: LegacyReplayPolicyOptions,
) -> Result<LegacyReplayPolicyInput<'a>> {
    Ok(LegacyReplayPolicyInput {
        replay_enabled: options.replay_enabled,
        foreign_replay_override: options.foreign_replay_override,
        no_scripts: options.no_scripts,
        requested_sandbox_mode: options.requested_sandbox_mode,
        host_policy: context.host_policy,
        target: context.target.as_target(),
        compatibility_matrix: compatibility_matrix_for_process()?,
        compatibility_environment: CompatibilityPreflightEnvironment {
            effective_sandbox: options.requested_sandbox_mode,
            ..CompatibilityPreflightEnvironment::default()
        },
    })
}

fn compatibility_matrix_for_process() -> Result<TargetCompatibilityMatrix> {
    #[cfg(debug_assertions)]
    {
        if std::env::var("CONARY_TEST_SKIP_GENERATION_MOUNT").as_deref() == Ok("1") {
            if let Ok(raw) = std::env::var("CONARY_TEST_COMPATIBILITY_MATRIX_JSON") {
                let matrix: TargetCompatibilityMatrix =
                    serde_json::from_str(&raw).context("parse CONARY_TEST_COMPATIBILITY_MATRIX_JSON")?;
                return TargetCompatibilityMatrix::new(matrix.entries().to_vec())
                    .context("validate CONARY_TEST_COMPATIBILITY_MATRIX_JSON");
            }
        }
    }

    Ok(TargetCompatibilityMatrix::production_default())
}
```

If `TargetCompatibilityMatrix.entries()` returns a slice over private entries, derive `Clone` for `TargetCompatibilityMatrixEntry` and keep the `entries().to_vec()` validation path. Invalid test JSON must return an error before mutation.

The test matrix injection is intentionally unavailable in release-profile test
runs because the parsing block is compiled only with `cfg(debug_assertions)`.
Do not use `cargo test --release` for the process-injection coverage in Goal 7;
release builds must always use `TargetCompatibilityMatrix::production_default()`.

- [ ] **Step 4: Register the module and remove duplicate policy helpers**

Modify `apps/conary/src/commands/mod.rs`:

```rust
mod legacy_replay_policy;
```

Remove the install-local `host_foreign_replay_policy_from_pin(...)` helper from `apps/conary/src/commands/install/mod.rs` after all tests import the commands-level helper.

- [ ] **Step 5: Update install planning signatures and call sites**

Change the signature of `plan_ccs_fresh_install_legacy_replay(...)` in `apps/conary/src/commands/install/mod.rs`:

```rust
pub(super) fn plan_ccs_fresh_install_legacy_replay(
    conn: &rusqlite::Connection,
    bundle: Option<&conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle>,
    opts: &CcsTransactionInstallOptions<'_>,
    is_upgrade: bool,
) -> Result<LegacyReplayInstallState> {
```

Inside that helper, replace `source_target_from_bundle(bundle)` as the policy target with:

```rust
let host_context = crate::commands::legacy_replay_policy::resolve_legacy_replay_host_context(conn)?;
let input = crate::commands::legacy_replay_policy::legacy_replay_policy_input(
    &host_context,
    crate::commands::legacy_replay_policy::LegacyReplayPolicyOptions {
        replay_enabled: opts.legacy_replay.allow_legacy_replay,
        foreign_replay_override: opts.legacy_replay.allow_foreign_legacy_replay,
        no_scripts: opts.no_scripts,
        requested_sandbox_mode: opts.sandbox_mode,
    },
)?;
let source_target_id = conary_core::repository::distro::source_target_from_bundle(bundle).to_id();
let target_id = host_context.target.to_id();
```

Update the existing audit context in the same helper so its target values match
the resolved host target and bundle source target:

```rust
audit: Some(LegacyReplayAuditContext {
    target_id: target_id.clone(),
    source_target_id,
    target_compatibility: bundle.target_compatibility.as_str().to_string(),
    foreign_replay_policy: bundle.foreign_replay_policy.as_str().to_string(),
    host_policy: host_context.host_policy,
    feature_gate_enabled: opts.legacy_replay.allow_legacy_replay,
    foreign_override: opts.legacy_replay.allow_foreign_legacy_replay,
    evidence_digest: bundle.evidence_digest.clone(),
}),
```

Update call sites:

```rust
plan_ccs_fresh_install_legacy_replay(conn, legacy_bundle, &opts, old_trove.is_some())?;
```

Update the `plan_ccs_fresh_install_legacy_replay(...)` call sites in
`apps/conary/src/commands/update.rs` and
`apps/conary/src/commands/install/restore.rs` by passing their available
`conn` as the first argument.

- [ ] **Step 6: Update old installed upgrade planning**

In `plan_ccs_old_installed_upgrade_legacy_replay(...)`, replace `source_target_from_bundle(&bundle)` as the input target with the same host context and policy input helper:

```rust
let host_context = crate::commands::legacy_replay_policy::resolve_legacy_replay_host_context(conn)?;
let input = crate::commands::legacy_replay_policy::legacy_replay_policy_input(
    &host_context,
    crate::commands::legacy_replay_policy::LegacyReplayPolicyOptions {
        replay_enabled: opts.legacy_replay.allow_legacy_replay,
        foreign_replay_override: opts.legacy_replay.allow_foreign_legacy_replay,
        no_scripts: opts.no_scripts,
        requested_sandbox_mode: opts.sandbox_mode,
    },
)?;
let source_target_id = conary_core::repository::distro::source_target_from_bundle(&bundle).to_id();
let target_id = host_context.target.to_id();
```

- [ ] **Step 7: Update remove planning**

Change `plan_installed_legacy_remove_replay(...)` in `apps/conary/src/commands/remove.rs` to accept `conn`:

```rust
fn plan_installed_legacy_remove_replay(
    conn: &rusqlite::Connection,
    bundle: &LegacyScriptletBundle,
    scriptlet_options: RemoveScriptletOptions,
) -> Result<PreparedLegacyRemoveReplay> {
```

Update `load_installed_legacy_remove_plan(...)`:

```rust
plan_installed_legacy_remove_replay(conn, &bundle, scriptlet_options)
```

Build input with the shared helper:

```rust
let host_context = crate::commands::legacy_replay_policy::resolve_legacy_replay_host_context(conn)?;
let input = crate::commands::legacy_replay_policy::legacy_replay_policy_input(
    &host_context,
    crate::commands::legacy_replay_policy::LegacyReplayPolicyOptions {
        replay_enabled: scriptlet_options.legacy_replay.allow_legacy_replay,
        foreign_replay_override: scriptlet_options.legacy_replay.allow_foreign_legacy_replay,
        no_scripts: scriptlet_options.no_scripts,
        requested_sandbox_mode: scriptlet_options.sandbox_mode,
    },
)?;
let source_target_id = conary_core::repository::distro::source_target_from_bundle(bundle).to_id();
let target_id = host_context.target.to_id();
```

- [ ] **Step 8: Update rollback preflight**

In `apps/conary/src/commands/system.rs`, replace the source-target input in `preflight_rollback_installed_bundle(...)`:

```rust
let host_context = crate::commands::legacy_replay_policy::resolve_legacy_replay_host_context(conn)?;
let input = crate::commands::legacy_replay_policy::legacy_replay_policy_input(
    &host_context,
    crate::commands::legacy_replay_policy::LegacyReplayPolicyOptions {
        replay_enabled: false,
        foreign_replay_override: false,
        no_scripts: false,
        requested_sandbox_mode: SandboxMode::Always,
    },
)?;
```

Remove the `source_target_from_bundle` import from `system.rs` once unused.

- [ ] **Step 9: Repair app `LegacyReplayPlan` test literals**

The new `LegacyReplayPlan.compatibility_decision` field breaks direct test
constructors before `cargo test -p conary ...` can compile. Update direct plan
constructors in:

- `apps/conary/src/commands/install/mod.rs::test_legacy_plan(entries: Vec<(&str, &str, LifecyclePath)>)`
- `apps/conary/src/commands/install/batch.rs`
- `apps/conary/src/commands/install/restore.rs`
- `apps/conary/src/commands/remove.rs`

Add this helper inside each affected `#[cfg(test)]` module:

```rust
fn accepted_compatibility_decision() -> conary_core::ccs::legacy_replay::LegacyReplayCompatibilityDecision {
    conary_core::ccs::legacy_replay::LegacyReplayCompatibilityDecision {
        decision: "accepted".to_string(),
        reason_code: "compatibility-source-native".to_string(),
        matrix_entry_id: None,
        matrix_digest: None,
        preflight_checks: Vec::new(),
        override_required: false,
        override_used: false,
    }
}
```

Set this field on every direct `LegacyReplayPlan { ... }` literal:

```rust
compatibility_decision: accepted_compatibility_decision(),
```

Also update any `apps/conary/src/commands/remove.rs` or
`apps/conary/src/commands/system.rs` unit tests that exercise source-native
installed bundles through `plan_installed_legacy_remove_replay(...)`,
`load_installed_legacy_remove_plan(...)`, or rollback preflight. Those tests
must initialize a test database and set a host pin that matches the fixture
source target before calling the host-context path:

```rust
let conn = db::open(&db_path).expect("open db");
conary_core::db::models::DistroPin::set(&conn, "fedora-44", "strict")
    .expect("set test distro pin");
```

The source-native planner may still be tested directly in `conary-core` with an
explicit `ReplayTarget`, but app-level host-context tests should not rely on the
`unknown/unknown/unknown/<arch>` fallback.

- [ ] **Step 10: Run focused compile/test checks**

Run:

```bash
cargo test -p conary --bin conary legacy_replay_policy
cargo test -p conary-core legacy_replay
cargo check -p conary
```

Expected: `legacy_replay_policy` and core `legacy_replay` pass, and
`cargo check -p conary` exits successfully. The `foreign_replay` integration
suite is updated and run in Task 5.

- [ ] **Step 11: Commit Tasks 2 and 3 together**

```bash
git add crates/conary-core/src/ccs/legacy_replay.rs crates/conary-core/src/ccs/target_compatibility.rs apps/conary/src/commands/legacy_replay_policy.rs apps/conary/src/commands/mod.rs apps/conary/src/commands/install/mod.rs apps/conary/src/commands/remove.rs apps/conary/src/commands/system.rs apps/conary/src/commands/install/restore.rs apps/conary/src/commands/update.rs apps/conary/src/commands/install/batch.rs
git commit -m "feat(cli): gate legacy replay with host compatibility policy"
```

---

### Task 4: Compatibility Audit Metadata

**Files:**
- Modify: `apps/conary/src/commands/changeset_metadata.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/install/batch.rs`
- Modify: `apps/conary/src/commands/install/restore.rs`

- [ ] **Step 1: Add failing changeset metadata tests**

In `apps/conary/src/commands/changeset_metadata.rs`, add tests near existing metadata tests:

```rust
#[test]
fn legacy_replay_audit_deserializes_goal6_json_without_compatibility() {
    let json = r#"{
      "bundle_present": true,
      "target_id": "rpm/fedora/44/x86_64",
      "source_target_id": "rpm/fedora/44/x86_64",
      "target_compatibility": "source-native",
      "foreign_replay_policy": "deny",
      "host_policy": "strict",
      "feature_gate": "enabled",
      "foreign_override": false,
      "evidence_digest": "sha256:test",
      "planned_entries": []
    }"#;

    let audit: LegacyReplayAudit = serde_json::from_str(json).expect("old audit JSON reads");

    assert_eq!(audit.compatibility.decision, "unknown");
    assert_eq!(
        audit.compatibility.reason_code,
        "compatibility-audit-unavailable"
    );
}

#[test]
fn legacy_replay_audit_serializes_compatibility_block() {
    let audit = LegacyReplayAudit {
        bundle_present: true,
        target_id: "rpm/fedora/44/x86_64".to_string(),
        source_target_id: "rpm/fedora/45/x86_64".to_string(),
        target_compatibility: "family-compatible".to_string(),
        foreign_replay_policy: "guarded".to_string(),
        host_policy: "guarded".to_string(),
        feature_gate: "enabled".to_string(),
        foreign_override: true,
        evidence_digest: Some("sha256:test".to_string()),
        compatibility: LegacyReplayCompatibilityAudit {
            decision: "accepted".to_string(),
            reason_code: "compatibility-matrix-entry-accepted".to_string(),
            matrix_entry_id: Some("test-fedora45-to-fedora44".to_string()),
            matrix_digest: Some("sha256:matrix".to_string()),
            override_required: true,
            override_used: true,
            preflight_checks: vec![LegacyReplayPreflightCheckAudit {
                id: "helper-systemctl".to_string(),
                kind: "helper".to_string(),
                status: "passed".to_string(),
                reason_code: "compatibility-helper-present".to_string(),
            }],
        },
        planned_entries: Vec::new(),
    };

    let value = serde_json::to_value(&audit).expect("serialize audit");

    assert_eq!(value["compatibility"]["decision"], "accepted");
    assert_eq!(
        value["compatibility"]["matrix_entry_id"],
        "test-fedora45-to-fedora44"
    );
}

#[test]
fn legacy_replay_audit_preflight_checks_exclude_local_paths() {
    let audit = LegacyReplayAudit {
        bundle_present: true,
        target_id: "rpm/fedora/44/x86_64".to_string(),
        source_target_id: "rpm/fedora/45/x86_64".to_string(),
        target_compatibility: "family-compatible".to_string(),
        foreign_replay_policy: "guarded".to_string(),
        host_policy: "guarded".to_string(),
        feature_gate: "enabled".to_string(),
        foreign_override: true,
        evidence_digest: Some("sha256:test".to_string()),
        compatibility: LegacyReplayCompatibilityAudit {
            decision: "accepted".to_string(),
            reason_code: "compatibility-matrix-entry-accepted".to_string(),
            matrix_entry_id: Some("test-fedora45-to-fedora44".to_string()),
            matrix_digest: Some("sha256:matrix".to_string()),
            override_required: true,
            override_used: true,
            preflight_checks: vec![LegacyReplayPreflightCheckAudit {
                id: "path-systemctl".to_string(),
                kind: "path".to_string(),
                status: "passed".to_string(),
                reason_code: "compatibility-path-present".to_string(),
            }],
        },
        planned_entries: Vec::new(),
    };

    let serialized = serde_json::to_string(&audit).expect("serialize audit");

    assert!(!serialized.contains("/tmp/conary"));
    assert!(!serialized.contains("/var/cache/conary"));
    assert!(!serialized.contains("review_artifact_path"));
}
```

- [ ] **Step 2: Run metadata tests and confirm they fail**

Run:

```bash
cargo test -p conary --bin conary changeset_metadata
```

Expected: compile fails because `LegacyReplayCompatibilityAudit`, `LegacyReplayPreflightCheckAudit`, and `LegacyReplayAudit.compatibility` do not exist.

- [ ] **Step 3: Add compatibility audit structs with serde defaults**

In `apps/conary/src/commands/changeset_metadata.rs`, insert before `LegacyReplayAudit`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LegacyReplayCompatibilityAudit {
    pub decision: String,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_digest: Option<String>,
    pub override_required: bool,
    pub override_used: bool,
    #[serde(default)]
    pub preflight_checks: Vec<LegacyReplayPreflightCheckAudit>,
}

impl Default for LegacyReplayCompatibilityAudit {
    fn default() -> Self {
        Self {
            decision: "unknown".to_string(),
            reason_code: "compatibility-audit-unavailable".to_string(),
            matrix_entry_id: None,
            matrix_digest: None,
            override_required: false,
            override_used: false,
            preflight_checks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LegacyReplayPreflightCheckAudit {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub reason_code: String,
}
```

Add the field to `LegacyReplayAudit`:

```rust
#[serde(default)]
pub compatibility: LegacyReplayCompatibilityAudit,
```

Update the existing `LegacyReplayAudit { ... }` literal in
`apps/conary/src/commands/changeset_metadata.rs` tests by adding:

```rust
compatibility: LegacyReplayCompatibilityAudit::default(),
```

Update the `pub(crate) use changeset_metadata::{ ... }` list in `apps/conary/src/commands/mod.rs` to include the two new audit types:

```rust
LegacyReplayCompatibilityAudit, LegacyReplayPreflightCheckAudit,
```

- [ ] **Step 4: Add compatibility carriers to install/remove audit contexts**

In `apps/conary/src/commands/install/mod.rs`, extend `LegacyReplayAuditContext`:

```rust
pub(crate) struct LegacyReplayAuditContext {
    pub target_id: String,
    pub source_target_id: String,
    pub target_compatibility: String,
    pub foreign_replay_policy: String,
    pub host_policy: conary_core::ccs::legacy_replay::HostForeignReplayPolicy,
    pub feature_gate_enabled: bool,
    pub foreign_override: bool,
    pub evidence_digest: Option<String>,
    pub compatibility: crate::commands::LegacyReplayCompatibilityAudit,
}
```

In `apps/conary/src/commands/remove.rs`, extend `LegacyRemoveReplayAuditContext` with the same field:

```rust
compatibility: crate::commands::LegacyReplayCompatibilityAudit,
```

Add this conversion helper in both `apps/conary/src/commands/install/mod.rs`
and `apps/conary/src/commands/remove.rs`:

```rust
fn compatibility_audit_from_plan(
    plan: Option<&conary_core::ccs::legacy_replay::LegacyReplayPlan>,
) -> crate::commands::LegacyReplayCompatibilityAudit {
    let Some(plan) = plan else {
        return crate::commands::LegacyReplayCompatibilityAudit::default();
    };
    let decision = &plan.compatibility_decision;
    crate::commands::LegacyReplayCompatibilityAudit {
        decision: decision.decision.clone(),
        reason_code: decision.reason_code.clone(),
        matrix_entry_id: decision.matrix_entry_id.clone(),
        matrix_digest: decision.matrix_digest.clone(),
        override_required: decision.override_required,
        override_used: decision.override_used,
        preflight_checks: decision
            .preflight_checks
            .iter()
            .map(|check| crate::commands::LegacyReplayPreflightCheckAudit {
                id: check.id.clone(),
                kind: check.kind.clone(),
                status: check.status.clone(),
                reason_code: check.reason_code.clone(),
            })
            .collect(),
    }
}
```

When constructing `LegacyReplayInstallState`, compute the plan options before
the struct literal so the context can carry a concrete compatibility audit:

```rust
let new_bundle_pre_plan = plan_from_preflight(pre)?;
let new_bundle_post_plan = plan_from_preflight(post)?;
let compatibility = compatibility_audit_from_plan(
    new_bundle_pre_plan
        .as_ref()
        .or(new_bundle_post_plan.as_ref()),
);

Ok(LegacyReplayInstallState {
    new_bundle_pre_plan,
    new_bundle_post_plan,
    accepted_bundle_to_persist: Some(AcceptedLegacyBundleInstall {
        bundle: bundle.clone(),
        target_id: target_id.clone(),
        replay_policy: LEGACY_REPLAY_POLICY.to_string(),
        replay_enabled: opts.legacy_replay.allow_legacy_replay,
    }),
    audit: Some(LegacyReplayAuditContext {
        target_id: target_id.clone(),
        source_target_id,
        target_compatibility: bundle.target_compatibility.as_str().to_string(),
        foreign_replay_policy: bundle.foreign_replay_policy.as_str().to_string(),
        host_policy: host_context.host_policy,
        feature_gate_enabled: opts.legacy_replay.allow_legacy_replay,
        foreign_override: opts.legacy_replay.allow_foreign_legacy_replay,
        evidence_digest: bundle.evidence_digest.clone(),
        compatibility,
    }),
    ..LegacyReplayInstallState::default()
})
```

In `plan_ccs_old_installed_upgrade_legacy_replay(...)`, compute
`old_bundle_pre_remove_plan` and `old_bundle_post_remove_plan` before the state
literal and use the same `compatibility_audit_from_plan(...)` helper. In
`merge_old_upgrade_legacy_replay_state(...)`, keep the existing rule that the
new-bundle audit context wins when present; this means upgrade metadata records
the compatibility decision for the package being installed, while planned entry
audit rows still include old pre/post-remove entries.

Populate `LegacyReplayAudit.compatibility` in `build_legacy_replay_audit_for_install(...)` and `build_legacy_replay_audit_for_remove(...)`:

```rust
compatibility: context.compatibility.clone(),
```

- [ ] **Step 5: Run metadata and app compile tests**

Run:

```bash
cargo test -p conary --bin conary changeset_metadata
cargo test -p conary --bin conary legacy_replay
cargo test -p conary-core legacy_replay
```

Expected: metadata tests pass and all `LegacyReplayPlan`/`LegacyReplayAudit` literal compile errors are resolved.

- [ ] **Step 6: Commit Task 4**

```bash
git add apps/conary/src/commands/changeset_metadata.rs apps/conary/src/commands/mod.rs apps/conary/src/commands/install/mod.rs apps/conary/src/commands/remove.rs apps/conary/src/commands/install/batch.rs apps/conary/src/commands/install/restore.rs
git commit -m "feat(cli): audit legacy replay compatibility decisions"
```

---

### Task 5: Update Foreign Replay And Bundle Replay Tests

**Files:**
- Modify: `apps/conary/tests/foreign_replay.rs`
- Modify: `apps/conary/tests/bundle_replay.rs`
- Modify: `apps/conary/tests/common/legacy_scriptlet_fixtures.rs`

- [ ] **Step 1: Update direct foreign replay tests with synthetic matrices**

In `apps/conary/tests/foreign_replay.rs`, import matrix types:

```rust
use conary_core::ccs::target_compatibility::{
    CompatibilityPreflightEnvironment, MatrixPreflightRequirements,
    TargetCompatibilityMatrix, TargetCompatibilityMatrixEntry, TargetSelector,
    TargetSelectorArch, TargetSelectorRelease,
};
```

Update `policy_input()`:

```rust
fn policy_input() -> LegacyReplayPolicyInput<'static> {
    LegacyReplayPolicyInput {
        replay_enabled: false,
        foreign_replay_override: false,
        no_scripts: false,
        requested_sandbox_mode: SandboxMode::Always,
        host_policy: HostForeignReplayPolicy::Strict,
        target: ReplayTarget {
            format: "rpm",
            distro: "fedora",
            release: "44",
            arch: "x86_64",
        },
        compatibility_matrix: synthetic_foreign_matrix(),
        compatibility_environment: CompatibilityPreflightEnvironment::default(),
    }
}

fn synthetic_foreign_matrix() -> TargetCompatibilityMatrix {
    TargetCompatibilityMatrix::for_testing(vec![TargetCompatibilityMatrixEntry {
        id: "test-fedora45-to-fedora44".to_string(),
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
        digest: Some("sha256:test-fedora45-to-fedora44".to_string()),
        rationale: "synthetic foreign replay test entry".to_string(),
    }])
}
```

Add a dedicated missing-matrix test:

```rust
#[test]
fn family_compatible_without_matrix_refuses_before_foreign_policy() {
    let bundle = foreign_legacy_bundle(ForeignReplayPolicy::Guarded);
    let mut input = policy_input();
    input.compatibility_matrix = TargetCompatibilityMatrix::production_default();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Guarded;

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan replay");

    assert_refused(
        preflight,
        LegacyReplayRefusalKind::CompatibilityMatrixEntryMissing,
    );
}
```

- [ ] **Step 2: Add process test matrix serialization helper**

In `apps/conary/tests/bundle_replay.rs`, import matrix types and add:

```rust
fn matrix_json_for_fedora44_source_native() -> String {
    let matrix = conary_core::ccs::target_compatibility::TargetCompatibilityMatrix::for_testing(
        vec![conary_core::ccs::target_compatibility::TargetCompatibilityMatrixEntry {
            id: "test-fedora44-to-fedora44".to_string(),
            source: conary_core::ccs::target_compatibility::TargetSelector {
                format: "rpm".to_string(),
                distro: "fedora".to_string(),
                release: conary_core::ccs::target_compatibility::TargetSelectorRelease::Exact(
                    "44".to_string(),
                ),
                arch: conary_core::ccs::target_compatibility::TargetSelectorArch::Exact(
                    "x86_64".to_string(),
                ),
            },
            target: conary_core::ccs::target_compatibility::TargetSelector {
                format: "rpm".to_string(),
                distro: "fedora".to_string(),
                release: conary_core::ccs::target_compatibility::TargetSelectorRelease::Exact(
                    "44".to_string(),
                ),
                arch: conary_core::ccs::target_compatibility::TargetSelectorArch::Exact(
                    "x86_64".to_string(),
                ),
            },
            requirements: conary_core::ccs::target_compatibility::MatrixPreflightRequirements::default(),
            digest: Some("sha256:test-fedora44-to-fedora44".to_string()),
            rationale: "synthetic process integration fixture".to_string(),
        }],
    );
    serde_json::to_string(&matrix).expect("serialize matrix")
}
```

Add a `run_conary_with_env(...)` helper:

```rust
fn run_conary_with_env(args: &[&str], envs: &[(&str, String)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_conary"));
    command.env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.args(args).output().expect("run conary")
}
```

Leave `run_conary(...)` as:

```rust
fn run_conary(args: &[&str]) -> Output {
    run_conary_with_env(args, &[])
}
```

- [ ] **Step 3: Add family-compatible fixture helper**

In `apps/conary/tests/common/legacy_scriptlet_fixtures.rs`, add a function that mutates an existing same-source legacy post-install bundle into a `family-compatible` fixture:

```rust
pub fn family_compatible_legacy_bundle() -> LegacyScriptletBundle {
    let mut bundle =
        synthetic_legacy_bundle(LegacyBundleFixture::SameSourceLegacyPostInstall)
            .expect("legacy bundle fixture");
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;
    bundle.allowed_targets.clear();
    bundle.validate().expect("family-compatible fixture validates");
    bundle
}
```

If the file already has a more specific fixture builder, extend that builder instead of duplicating package contents.

- [ ] **Step 4: Add bundle replay missing-matrix test**

In `apps/conary/tests/bundle_replay.rs`, add:

```rust
#[test]
fn ccs_install_family_compatible_without_matrix_refuses_before_db_mutation() {
    let bundle = common::legacy_scriptlet_fixtures::family_compatible_legacy_bundle();
    let (_package_temp, package_path) =
        build_ccs_package_fixture("legacy-fixture-family-compatible", "1.0.0", Some(bundle))
            .expect("build CCS fixture");
    let fixture = InstallFixture::from_package(_package_temp, package_path);

    let output = fixture.run_install(&["--allow-legacy-replay", "--allow-foreign-legacy-replay"]);

    assert_failure(&output);
    assert_contains(&output, "CompatibilityMatrixEntryMissing");
    fixture.assert_no_install_mutation();
}
```

- [ ] **Step 5: Add bundle replay accepted-matrix audit test**

Add a method to `InstallFixture`:

```rust
fn run_install_with_matrix(&self, extra_args: &[&str], matrix_json: String) -> Output {
    let mut args = vec![
        "--allow-live-system-mutation",
        "ccs",
        "install",
        self.package_path.to_str().expect("utf-8 package path"),
        "--allow-unsigned",
        "--sandbox",
        "never",
        "--db-path",
        self.db_path.to_str().expect("utf-8 db path"),
        "--root",
        self.root.to_str().expect("utf-8 root path"),
    ];
    args.extend_from_slice(extra_args);
    run_conary_with_env(&args, &[("CONARY_TEST_COMPATIBILITY_MATRIX_JSON", matrix_json)])
}
```

Add the test:

```rust
#[test]
fn ccs_install_family_compatible_with_test_matrix_records_compatibility_audit() {
    let bundle = common::legacy_scriptlet_fixtures::family_compatible_legacy_bundle();
    let (_package_temp, package_path) =
        build_ccs_package_fixture("legacy-fixture-family-compatible", "1.0.0", Some(bundle))
            .expect("build CCS fixture");
    let fixture = InstallFixture::from_package(_package_temp, package_path);

    let output = fixture.run_install_with_matrix(
        &["--allow-legacy-replay", "--allow-foreign-legacy-replay"],
        matrix_json_for_fedora44_source_native(),
    );

    assert_success(&output);
    let conn = db::open(&fixture.db_path).expect("open db");
    let metadata = single_changeset_metadata(&conn);
    let compatibility = &metadata["legacy_scriptlet_replay"]["compatibility"];
    assert_eq!(compatibility["decision"], "accepted");
    assert_eq!(
        compatibility["reason_code"],
        "compatibility-matrix-entry-accepted"
    );
    assert_eq!(
        compatibility["matrix_entry_id"],
        "test-fedora44-to-fedora44"
    );
    assert_eq!(compatibility["override_used"], true);
    assert_metadata_excludes_local_paths(&metadata, &fixture);
}
```

- [ ] **Step 6: Seed the host distro pin in bundle replay fixtures**

Goal 7 host-target resolution falls back to `unknown/unknown/unknown/<arch>`
when the test database has no `DistroPin`. The existing `bundle_replay`
process tests are Fedora-source fixtures, so seed a Fedora host pin in
`InstallFixture::from_package(...)` and `UpgradeFixture::new(...)` immediately
after `db::init(&db_path)`:

```rust
let conn = db::open(&db_path).expect("open db");
conary_core::db::models::DistroPin::set(&conn, "fedora-44", "strict")
    .expect("set test distro pin");
```

This pin setup is required before the integration test suite is run. Without
it, source-native fixture installs resolve the host target as unknown and fail
the target compatibility preflight for the wrong reason.

- [ ] **Step 7: Run integration test suites**

Run:

```bash
cargo test -p conary --test bundle_replay
cargo test -p conary --test foreign_replay
```

Expected: both suites pass.

- [ ] **Step 8: Commit Task 5**

```bash
git add apps/conary/tests/foreign_replay.rs apps/conary/tests/bundle_replay.rs apps/conary/tests/common/legacy_scriptlet_fixtures.rs
git commit -m "test(cli): cover legacy replay compatibility matrix gates"
```

---

### Task 6: Documentation, Verification, And Final Cleanup

**Files:**
- Modify: `docs/modules/ccs.md` or `docs/modules/source-selection.md`
- Modify: `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md` if verification command wording needs alignment

- [ ] **Step 1: Add user-facing documentation for the compatibility boundary**

Check `docs/modules/ccs.md` and `docs/modules/source-selection.md`:

```bash
rg -n "scriptlet|legacy|compatib|CCS|portable|source" docs/modules/ccs.md docs/modules/source-selection.md
```

Add this text to the module that already discusses CCS conversion or source selection:

```markdown
Converted CCS packages can carry metadata about legacy native scriptlets, but
CCS format does not make raw native scriptlets portable across distributions.
Raw replay of `family-compatible` legacy scriptlets is accepted only when an
explicit target compatibility matrix entry authorizes the source and host target
pair and the shallow compatibility preflight succeeds. The default production
matrix is empty, so Conary fails closed unless a later release ships or configures
validated compatibility evidence.
```

- [ ] **Step 2: Verify the active goal queue still points to the design and plan**

In `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`, ensure Goal 7 lists both docs:

```markdown
Design:

- `docs/superpowers/specs/2026-06-02-legacy-scriptlet-compatibility-matrix-override-audit-design.md`

Implementation plan:

- `docs/superpowers/plans/2026-06-02-legacy-scriptlet-compatibility-matrix-override-audit-plan.md`
```

- [ ] **Step 3: Run focused verification commands**

Run:

```bash
cargo test -p conary-core target_compatibility
cargo test -p conary-core legacy_replay
cargo test -p conary --test bundle_replay
cargo test -p conary --test foreign_replay
cargo test -p conary --bin conary live_host_safety
```

Expected: all pass.

- [ ] **Step 4: Run workspace quality gates**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

Expected: all pass with no warnings or whitespace errors.

- [ ] **Step 5: Inspect final diff for policy leaks**

Run:

```bash
rg -n "CONARY_TEST_COMPATIBILITY_MATRIX_JSON|__CONARY_TEST_MATRIX_JSON|compatibility matrix|family-compatible|source_target_from_bundle" crates/conary-core/src apps/conary/src apps/conary/tests docs/modules
```

Expected:

- `CONARY_TEST_COMPATIBILITY_MATRIX_JSON` appears only in the commands-level helper and tests.
- No public CLI help text mentions matrix injection.
- Production planning helpers no longer pass `source_target_from_bundle(...)` into `LegacyReplayPolicyInput.target`.
- Docs state the default production matrix is empty.

- [ ] **Step 6: Commit Task 6**

```bash
git add docs/modules docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md
git commit -m "docs: document legacy replay compatibility matrix boundary"
```

---

## Final Verification Checklist

Run the full Goal 7 verification set before merging:

```bash
cargo test -p conary-core target_compatibility
cargo test -p conary-core legacy_replay
cargo test -p conary --test bundle_replay
cargo test -p conary --test foreign_replay
cargo test -p conary --bin conary live_host_safety
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

Then inspect repo state:

```bash
git status --short --branch
git log --oneline --decorate -5
```

Goal 7 implementation is ready to merge when the verification commands pass, the final diff contains no release-build matrix injection path, and the only production matrix is `TargetCompatibilityMatrix::production_default()` with zero entries.
