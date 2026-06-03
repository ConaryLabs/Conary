# Legacy Scriptlet Publication Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Goal 5 by making Remi's passive scriptlet publication metadata authoritative for public serving, discovery, and job outcomes without adding replay, curation promotion, installer behavior, or database migrations.

**Architecture:** Add a Remi-local publication policy module backed by health-aware `ConvertedPackage` summary helpers. Thread publication decisions through conversion persistence, hot-cache results, jobs, package/download handlers, public discovery surfaces, chunk/blob streaming, and admin CCS upload/review artifact paths. Non-public current rows become terminal review/blocked outcomes, while stale rows continue to trigger reconversion.

**Tech Stack:** Rust, Axum, `serde`, `serde_json`, `rusqlite`, existing Remi `ConvertedPackage` Goal 4 columns, `ScriptletBundleSummary`, `LegacyScriptletBundle`, `ScriptletPackageMetadata`, chunk CAS, OCI handlers, and current Remi admin auth scopes.

---

## Source Context

Read before implementation:

- `AGENTS.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-publication-gate-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-passive-remi-bundle-embedding-design.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- `crates/conary-core/src/db/models/converted.rs`
- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/jobs.rs`
- `apps/remi/src/server/handlers/jobs.rs`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/handlers/index.rs`
- `apps/remi/src/server/index_gen.rs`
- `apps/remi/src/server/handlers/detail.rs`
- `apps/remi/src/server/handlers/sparse.rs`
- `apps/remi/src/server/federated_index.rs`
- `apps/remi/src/server/delta_manifests.rs`
- `apps/remi/src/server/search.rs`
- `apps/remi/src/server/prewarm.rs`
- `apps/remi/src/server/handlers/chunks.rs`
- `apps/remi/src/server/handlers/oci.rs`
- `apps/remi/src/server/handlers/admin/packages.rs`
- `apps/remi/src/server/routes/admin.rs`
- `docs/modules/remi.md`

## Scope Rules

- Do not add a database migration. Use the Goal 4 scriptlet columns already on `converted_packages`.
- Do not change package parsing, adapter classification, install/update/remove, scriptlet replay, or client-side enforcement.
- Do not promote `private-review`, `blocked`, or `local-only` rows to public-ready in Goal 5.
- Do not expose `review_artifact_path` or local cache paths through public APIs.
- Treat non-public current rows as terminal policy outcomes, not conversion failures and not missing rows.
- Treat stale rows as missing/reconvertable exactly as Goal 4 did.
- Keep `ConvertedPackage::new()` and `ConvertedPackage::new_server()` signatures stable.
- Prefer shared helpers over repeating `publication_status == "public"` checks.

## File Structure

Create:

- `apps/remi/src/server/publication.rs`
  - Owns `PublicationDecision`, `PublicationRefusal`, `PublicationGateReport`, `ServerConversionOutcome`, response helpers, review artifact helpers, and chunk reachability helpers that are Remi-specific.

Modify:

- `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
  - Expose `ScriptletBundleSummary::from_bundle(...)` for admin CCS upload projection.
- `crates/conary-core/src/db/models/converted.rs`
  - Add `ScriptletSummaryForPublication`, health-aware summary parsing, public-ready helpers, chunk hash parsing, and public-ready chunk lookup helpers.
- `apps/remi/src/server/mod.rs`
  - Export `publication`.
- `apps/remi/src/server/conversion.rs`
  - Return `ServerConversionOutcome`, persist review artifacts for non-public rows, and gate hot-cache results.
- `apps/remi/src/server/jobs.rs`
  - Add `ReviewRequired` and `Blocked` terminal statuses and extend job results with scriptlet/publication metadata.
- `apps/remi/src/server/handlers/jobs.rs`
  - Return publication reports for review/blocked jobs and omit ready manifests.
- `apps/remi/src/server/handlers/packages.rs`
  - Gate package manifest and download paths with structured `409`/`403` refusals.
- `apps/remi/src/server/handlers/index.rs`
  - Only mark public-ready rows as converted in `/v1/:distro/metadata`.
- `apps/remi/src/server/index_gen.rs`
  - Only include public-ready converted rows in generated public indexes.
- `apps/remi/src/server/handlers/detail.rs`
  - Count and flag only public-ready rows.
- `apps/remi/src/server/handlers/sparse.rs`
  - Expose content hashes only for public-ready rows.
- `apps/remi/src/server/federated_index.rs`
  - Expose federated sparse content hashes only for public-ready rows.
- `apps/remi/src/server/delta_manifests.rs`
  - Use public-ready rows for chunk lists, eligibility, and version enumeration.
- `apps/remi/src/server/search.rs`
  - Mark search results converted only for public-ready rows.
- `apps/remi/src/server/prewarm.rs`
  - Keep non-public rows terminal to avoid reconversion loops, but do not call them public-ready.
- `apps/remi/src/server/handlers/chunks.rs`
  - Refuse raw chunk access for non-public-only local hashes.
- `apps/remi/src/server/handlers/oci.rs`
  - Refuse OCI manifests/tags/catalog/blobs for non-public rows.
- `apps/remi/src/server/handlers/admin/packages.rs`
  - Gate uploaded CCS bundles and add admin review-artifact retrieval.
- `apps/remi/src/server/routes/admin.rs`
  - Route admin review-artifact retrieval.
- `docs/modules/remi.md`
  - Document Goal 5 serving policy and deferred replay/curation boundary.

Test:

- Existing colocated unit tests in the files above.
- New focused tests named in each task below.

## Task 1: Publication Policy And Summary Health

**Files:**

- Create: `apps/remi/src/server/publication.rs`
- Modify: `apps/remi/src/server/mod.rs`
- Modify: `crates/conary-core/src/db/models/mod.rs`
- Modify: `crates/conary-core/src/db/models/converted.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`

- [ ] **Step 1: Write failing summary-health tests**

Add tests to `crates/conary-core/src/db/models/converted.rs`:

```rust
#[test]
fn scriptlet_summary_for_publication_accepts_constructor_default_shape() {
    let converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "plain".to_string(),
        "1.0".to_string(),
        "ccs".to_string(),
        "upload:fedora:abc".to_string(),
        "full".to_string(),
        &["abc".to_string()],
        3,
        "abc".to_string(),
        "/tmp/plain.ccs".to_string(),
    );

    let publication = converted.scriptlet_summary_for_publication();

    assert!(publication.valid);
    assert_eq!(publication.summary.publication_status, "public");
    assert!(converted.is_scriptlet_public_ready());
}

#[test]
fn scriptlet_summary_for_publication_rejects_default_json_with_scriptlet_evidence() {
    let mut converted = ConvertedPackage::new(
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
    );
    converted.scriptlet_fidelity = "blocked".to_string();
    converted.target_compatibility = "blocked".to_string();
    converted.publication_status = "public".to_string();
    converted.evidence_digest = Some(crate::hash::sha256_prefixed(b"evidence"));
    converted.scriptlet_summary_json = "{}".to_string();

    let publication = converted.scriptlet_summary_for_publication();

    assert!(!publication.valid);
    assert!(!converted.is_scriptlet_public_ready());
}

#[test]
fn scriptlet_summary_for_publication_rejects_partial_and_malformed_json() {
    let mut converted = ConvertedPackage::new(
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
    );
    converted.scriptlet_summary_json = r#"{"publication_status":"public"}"#.to_string();
    assert!(!converted.scriptlet_summary_for_publication().valid);

    converted.scriptlet_summary_json = "{not valid json".to_string();
    assert!(!converted.scriptlet_summary_for_publication().valid);
}

#[test]
fn scriptlet_public_ready_requires_valid_summary_and_public_status() {
    let mut converted = ConvertedPackage::new(
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
    );
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        publication_status: "private-review".to_string(),
        review_reason_codes: vec!["review-class-debconf".to_string()],
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();

    assert!(converted.scriptlet_summary_for_publication().valid);
    assert!(!converted.is_scriptlet_public_ready());
}
```

Add tests to `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`:

```rust
#[test]
fn scriptlet_bundle_summary_from_bundle_is_public_api() {
    let metadata = package_metadata("public-api", "1.0");
    let classification = ScriptletClassificationReport::default();
    let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();

    let summary = ScriptletBundleSummary::from_bundle(
        &build.bundle,
        Some(crate::hash::sha256_prefixed(b"x")),
    );

    assert_eq!(summary.publication_status, build.bundle.publication_status.as_str());
    assert_eq!(summary.evidence_digest, Some(crate::hash::sha256_prefixed(b"x")));
    assert_eq!(summary.review_artifact_path, None);
}
```

- [ ] **Step 2: Run the failing core tests**

Run:

```bash
cargo test -p conary-core scriptlet_summary_for_publication
cargo test -p conary-core scriptlet_bundle_summary_from_bundle_is_public_api
```

Expected: fail because the health-aware helper and public `from_bundle` API do not exist.

- [ ] **Step 3: Implement health-aware summary helpers**

Add to `crates/conary-core/src/db/models/converted.rs` near the model impl:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletSummaryForPublication {
    pub summary: ScriptletBundleSummary,
    pub valid: bool,
}
```

Add methods to `impl ConvertedPackage`:

```rust
pub fn scriptlet_publication_status(&self) -> &str {
    self.publication_status.as_str()
}

pub fn scriptlet_summary_for_publication(&self) -> ScriptletSummaryForPublication {
    let value = match serde_json::from_str::<serde_json::Value>(&self.scriptlet_summary_json) {
        Ok(value) => value,
        Err(_) => {
            return ScriptletSummaryForPublication {
                summary: self.scriptlet_summary(),
                valid: false,
            };
        }
    };

    let shape_valid = self.summary_json_shape_valid_for_publication(&value);
    let summary = self.scriptlet_summary();
    let status_matches = value
        .get("publication_status")
        .and_then(|value| value.as_str())
        .map(|status| status == self.publication_status)
        .unwrap_or_else(|| self.is_default_scriptlet_publication_shape(&value));

    ScriptletSummaryForPublication {
        summary,
        valid: shape_valid && status_matches,
    }
}

pub fn is_scriptlet_public_ready(&self) -> bool {
    let publication = self.scriptlet_summary_for_publication();
    publication.valid && publication.summary.publication_status == "public"
}

pub fn parsed_chunk_hashes(&self) -> Vec<String> {
    self.chunk_hashes_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
        .unwrap_or_default()
}

fn summary_json_shape_valid_for_publication(&self, value: &serde_json::Value) -> bool {
    if self.is_default_scriptlet_publication_shape(value) {
        return true;
    }

    let Some(object) = value.as_object() else {
        return false;
    };

    [
        "scriptlet_fidelity",
        "target_compatibility",
        "publication_status",
        "decision_counts",
        "blocked_reason_codes",
        "review_reason_codes",
        "unknown_commands",
        "blocked_classes",
    ]
    .iter()
    .all(|key| object.contains_key(*key))
}

fn is_default_scriptlet_publication_shape(&self, value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|object| object.is_empty())
        && self.scriptlet_fidelity == "unknown"
        && self.target_compatibility == "unknown"
        && self.publication_status == "public"
        && self.evidence_digest.is_none()
        && self.curation_evidence_digest.is_none()
        && json_string_array_is_empty(&self.blocked_reason_codes_json)
        && self.review_artifact_path.is_none()
}

fn json_string_array_is_empty(value: &str) -> bool {
    match serde_json::from_str::<Vec<String>>(value) {
        Ok(values) => values.is_empty(),
        Err(_) => false,
    }
}
```

Do not use an exact string comparison such as `blocked_reason_codes_json == "[]"`
for the constructor-default shape. That would be coupled to one JSON formatting
spelling; parse the JSON and check logical emptiness instead.

Re-export `ScriptletSummaryForPublication` from
`crates/conary-core/src/db/models/mod.rs` alongside `ConvertedPackage`, because
Remi imports it through `conary_core::db::models`.

- [ ] **Step 4: Expose `ScriptletBundleSummary::from_bundle`**

In `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`, replace the private summary projection call with a public associated function:

```rust
impl ScriptletBundleSummary {
    pub fn from_bundle(
        bundle: &LegacyScriptletBundle,
        evidence_digest: Option<String>,
    ) -> Self {
        summary_from_bundle(bundle, evidence_digest)
    }
}
```

Keep the private `summary_from_bundle(...)` function for internal call sites, or update internal call sites to call the public method. Do not change the aggregation semantics.

- [ ] **Step 5: Create Remi publication policy module**

Create `apps/remi/src/server/publication.rs`:

```rust
// apps/remi/src/server/publication.rs
//! Publication policy for legacy scriptlet conversion results.

use crate::server::conversion::ScriptletPackageMetadata;
use axum::{Json, http::StatusCode, response::{IntoResponse, Response}};
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{ConvertedPackage, ScriptletSummaryForPublication};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationDecision {
    Ready,
    ReviewRequired(PublicationGateReport),
    Blocked(PublicationGateReport),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationRefusal {
    ReviewRequired(PublicationGateReport),
    Blocked(PublicationGateReport),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicationGateReport {
    pub publication_status: String,
    pub scriptlet_fidelity: String,
    pub target_compatibility: String,
    pub summary_valid: bool,
    pub message: String,
    pub reason_codes: Vec<String>,
    pub blocked_reason_codes: Vec<String>,
    pub review_reason_codes: Vec<String>,
    pub unknown_commands: Vec<String>,
    pub blocked_classes: Vec<String>,
    pub evidence_digest: Option<String>,
    pub curation_evidence_digest: Option<String>,
    pub review_artifact_available: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PublicationRefusalResponse {
    pub status: &'static str,
    pub message: String,
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
    pub scriptlets: PublicationGateReport,
}

pub fn classify_converted_package(converted: &ConvertedPackage) -> PublicationDecision {
    classify_summary(converted.scriptlet_summary_for_publication())
}

pub fn classify_summary(publication: ScriptletSummaryForPublication) -> PublicationDecision {
    if publication.valid && publication.summary.publication_status == "public" {
        return PublicationDecision::Ready;
    }

    let report = report_from_summary(&publication.summary, publication.valid);
    if publication.summary.publication_status == "blocked" {
        PublicationDecision::Blocked(report)
    } else {
        PublicationDecision::ReviewRequired(report)
    }
}

pub fn refusal_response(
    refusal: PublicationRefusal,
    distro: &str,
    package: &str,
    version: Option<&str>,
) -> Response {
    let (status, status_text, report) = match refusal {
        PublicationRefusal::ReviewRequired(report) => {
            (StatusCode::CONFLICT, "review-required", report)
        }
        PublicationRefusal::Blocked(report) => (StatusCode::FORBIDDEN, "blocked", report),
    };

    (
        status,
        Json(PublicationRefusalResponse {
            status: status_text,
            message: report.message.clone(),
            distro: distro.to_string(),
            package: package.to_string(),
            version: version.map(str::to_string),
            scriptlets: report,
        }),
    )
        .into_response()
}

pub fn decision_refusal(decision: PublicationDecision) -> Option<PublicationRefusal> {
    match decision {
        PublicationDecision::Ready => None,
        PublicationDecision::ReviewRequired(report) => {
            Some(PublicationRefusal::ReviewRequired(report))
        }
        PublicationDecision::Blocked(report) => Some(PublicationRefusal::Blocked(report)),
    }
}

pub fn report_from_summary(
    summary: &ScriptletBundleSummary,
    summary_valid: bool,
) -> PublicationGateReport {
    let mut reason_codes = Vec::new();
    let mut seen = BTreeSet::new();
    for code in &summary.blocked_reason_codes {
        push_reason(&mut reason_codes, &mut seen, code.clone());
    }
    for code in &summary.review_reason_codes {
        push_reason(&mut reason_codes, &mut seen, code.clone());
    }
    for command in sorted(&summary.unknown_commands) {
        push_reason(&mut reason_codes, &mut seen, format!("unknown-command:{command}"));
    }
    for class_id in sorted(&summary.blocked_classes) {
        push_reason(&mut reason_codes, &mut seen, class_id);
    }
    if !summary_valid {
        push_reason(
            &mut reason_codes,
            &mut seen,
            "publication-gate-malformed-summary".to_string(),
        );
    }

    PublicationGateReport {
        publication_status: summary.publication_status.clone(),
        scriptlet_fidelity: summary.scriptlet_fidelity.clone(),
        target_compatibility: summary.target_compatibility.clone(),
        summary_valid,
        message: message_for_status(&summary.publication_status, summary_valid).to_string(),
        reason_codes,
        blocked_reason_codes: summary.blocked_reason_codes.clone(),
        review_reason_codes: summary.review_reason_codes.clone(),
        unknown_commands: sorted(&summary.unknown_commands),
        blocked_classes: sorted(&summary.blocked_classes),
        evidence_digest: summary.evidence_digest.clone(),
        curation_evidence_digest: summary.curation_evidence_digest.clone(),
        review_artifact_available: summary.review_artifact_path.is_some(),
    }
}

pub fn public_metadata(summary: &ScriptletBundleSummary) -> ScriptletPackageMetadata {
    ScriptletPackageMetadata::from(summary)
}

fn push_reason(reasons: &mut Vec<String>, seen: &mut BTreeSet<String>, reason: String) {
    if seen.insert(reason.clone()) {
        reasons.push(reason);
    }
}

fn sorted(values: &[String]) -> Vec<String> {
    values.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

fn message_for_status(status: &str, valid: bool) -> &'static str {
    if !valid {
        return "Converted package has malformed scriptlet publication metadata";
    }
    match status {
        "blocked" => "Converted package is blocked by legacy scriptlet policy",
        "local-only" => "Converted package is local-only and cannot be served publicly",
        "private-review" => "Converted package requires scriptlet review before public serving",
        _ => "Converted package is not public-ready",
    }
}
```

Modify `apps/remi/src/server/mod.rs`:

```rust
pub mod publication;
```

- [ ] **Step 6: Add publication policy unit tests**

Add to `apps/remi/src/server/publication.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::convert::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};

    fn summary(status: &str) -> ScriptletBundleSummary {
        ScriptletBundleSummary {
            publication_status: status.to_string(),
            scriptlet_fidelity: status.to_string(),
            target_compatibility: status.to_string(),
            ..ScriptletBundleSummary::default()
        }
    }

    #[test]
    fn publication_policy_maps_statuses_to_decisions() {
        assert!(matches!(
            classify_summary(ScriptletSummaryForPublication {
                summary: summary("public"),
                valid: true,
            }),
            PublicationDecision::Ready
        ));
        assert!(matches!(
            classify_summary(ScriptletSummaryForPublication {
                summary: summary("private-review"),
                valid: true,
            }),
            PublicationDecision::ReviewRequired(_)
        ));
        assert!(matches!(
            classify_summary(ScriptletSummaryForPublication {
                summary: summary("blocked"),
                valid: true,
            }),
            PublicationDecision::Blocked(_)
        ));
        assert!(matches!(
            classify_summary(ScriptletSummaryForPublication {
                summary: summary("public"),
                valid: false,
            }),
            PublicationDecision::ReviewRequired(_)
        ));
    }

    #[test]
    fn publication_report_reasons_are_deterministic_and_deduplicated() {
        let summary = ScriptletBundleSummary {
            publication_status: "private-review".to_string(),
            decision_counts: ScriptletDecisionCountsSummary {
                review: 2,
                ..ScriptletDecisionCountsSummary::default()
            },
            blocked_reason_codes: vec!["blocked-b".to_string(), "blocked-a".to_string()],
            review_reason_codes: vec!["review-a".to_string(), "review-a".to_string()],
            unknown_commands: vec!["zz".to_string(), "aa".to_string()],
            blocked_classes: vec!["class-b".to_string(), "class-a".to_string()],
            ..ScriptletBundleSummary::default()
        };

        let report = report_from_summary(&summary, true);

        assert_eq!(
            report.reason_codes,
            vec![
                "blocked-b",
                "blocked-a",
                "review-a",
                "unknown-command:aa",
                "unknown-command:zz",
                "class-a",
                "class-b",
            ]
        );
    }
}
```

- [ ] **Step 7: Run tests and commit**

Run:

```bash
cargo test -p conary-core scriptlet_summary_for_publication
cargo test -p conary-core scriptlet_bundle_summary_from_bundle_is_public_api
cargo test -p remi publication
```

Expected: pass.

Commit:

```bash
git add crates/conary-core/src/db/models/converted.rs crates/conary-core/src/db/models/mod.rs crates/conary-core/src/ccs/convert/scriptlet_bundle.rs apps/remi/src/server/mod.rs apps/remi/src/server/publication.rs
git commit -m "feat(remi): add scriptlet publication policy"
```

## Task 2: Conversion Outcomes And Job States

**Files:**

- Modify: `apps/remi/src/server/publication.rs`
- Modify: `apps/remi/src/server/conversion.rs`
- Modify: `apps/remi/src/server/jobs.rs`
- Modify: `apps/remi/src/server/handlers/jobs.rs`
- Modify: `apps/remi/src/server/handlers/packages.rs`

- [ ] **Step 1: Write failing job and conversion tests**

Add tests to `apps/remi/src/server/jobs.rs`:

```rust
#[test]
fn review_required_and_blocked_jobs_are_terminal_not_failed() {
    let mut manager = JobManager::new(2);
    let review = manager
        .create_job("review".into(), "fedora".into(), "pkg".into(), None, None)
        .unwrap();
    let blocked = manager
        .create_job("blocked".into(), "fedora".into(), "pkg2".into(), None, None)
        .unwrap();

    manager.update_status(&review, JobStatus::ReviewRequired);
    manager.update_status(&blocked, JobStatus::Blocked);

    assert!(manager.get_job(&review).unwrap().completed_at.is_some());
    assert!(manager.get_job(&blocked).unwrap().completed_at.is_some());
}
```

Keep this first test focused on terminal-state behavior. Add `JobStats`
assertions only after Step 6 extends the stats struct; otherwise the initial TDD
test introduces an avoidable parse error on fields that the plan has not added
yet.

Add tests to `apps/remi/src/server/conversion.rs`:

```rust
#[test]
fn server_conversion_outcome_reports_terminal_state() {
    let result = ServerConversionResult {
        name: "pkg".to_string(),
        version: "1.0".to_string(),
        distro: "fedora".to_string(),
        chunk_hashes: Vec::new(),
        total_size: 0,
        content_hash: "sha256:test".to_string(),
        ccs_path: std::path::PathBuf::from("/tmp/pkg.ccs"),
        cache_state: "cold".to_string(),
        scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
        publication: None,
        timing: None,
    };

    assert!(matches!(ServerConversionOutcome::Ready(result).job_status(), JobStatus::Ready));
}
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p remi review_required_and_blocked_jobs_are_terminal_not_failed
cargo test -p remi server_conversion_outcome_reports_terminal_state
```

Expected: fail because the new statuses/outcome type do not exist.

- [ ] **Step 3: Add `ServerConversionOutcome`**

Add to `apps/remi/src/server/publication.rs`:

```rust
use crate::server::conversion::ServerConversionResult;
use crate::server::jobs::JobStatus;

#[derive(Debug)]
pub enum ServerConversionOutcome {
    Ready(ServerConversionResult),
    ReviewRequired(ServerConversionResult),
    Blocked(ServerConversionResult),
}

impl ServerConversionOutcome {
    pub fn into_result(self) -> ServerConversionResult {
        match self {
            Self::Ready(result) | Self::ReviewRequired(result) | Self::Blocked(result) => result,
        }
    }

    pub fn result(&self) -> &ServerConversionResult {
        match self {
            Self::Ready(result) | Self::ReviewRequired(result) | Self::Blocked(result) => result,
        }
    }

    pub fn job_status(&self) -> JobStatus {
        match self {
            Self::Ready(_) => JobStatus::Ready,
            Self::ReviewRequired(_) => JobStatus::ReviewRequired,
            Self::Blocked(_) => JobStatus::Blocked,
        }
    }
}
```

- [ ] **Step 4: Extend server conversion results**

Modify `apps/remi/src/server/conversion.rs` imports and structs:

```rust
use crate::server::publication::{
    PublicationDecision, PublicationGateReport, ReviewArtifactInput, ServerConversionOutcome,
    classify_converted_package, decision_refusal, report_from_summary,
};
```

Add to `ServerConversionResult`:

```rust
/// Publication refusal report for review-required or blocked results.
pub publication: Option<PublicationGateReport>,
```

Change conversion methods that currently return `Result<ServerConversionResult>` for package conversion/hot cache to return `Result<ServerConversionOutcome>`:

```rust
fn outcome_from_converted_result(
    converted: &ConvertedPackage,
    mut result: ServerConversionResult,
) -> ServerConversionOutcome {
    match classify_converted_package(converted) {
        PublicationDecision::Ready => ServerConversionOutcome::Ready(result),
        PublicationDecision::ReviewRequired(report) => {
            result.publication = Some(report);
            ServerConversionOutcome::ReviewRequired(result)
        }
        PublicationDecision::Blocked(report) => {
            result.publication = Some(report);
            ServerConversionOutcome::Blocked(result)
        }
    }
}
```

Update all `ServerConversionResult` literals in `conversion.rs` tests and production code with:

```rust
publication: None,
```

- [ ] **Step 5: Gate cold persistence and hot cache**

Before changing persistence, add the review artifact data types and
`write_review_artifact(...)` helper described in Task 6 Step 3 to
`apps/remi/src/server/publication.rs`. Task 2 needs that helper for fresh
conversion persistence; Task 6 adds the admin retrieval/upload tests and reuses
the same helper rather than creating a second artifact format.

In `persist_conversion_result(...)`, after `converted.set_scriptlet_metadata(...)`, classify the row before insert:

```rust
let decision = classify_converted_package(&converted);
if let Some(refusal) = decision_refusal(decision.clone()) {
    let mut report = match refusal {
        PublicationRefusal::ReviewRequired(report) | PublicationRefusal::Blocked(report) => report,
    };
    report.review_artifact_available = true;
    let conversion_fidelity = conversion_result.fidelity.level.to_string();
    let artifact_path = crate::server::publication::write_review_artifact(
        &self.cache_dir,
        ReviewArtifactInput {
            distro: &distro,
            package: &metadata.name,
            version: &metadata.version,
            architecture: metadata.architecture.as_deref(),
            original_format: &conversion_result.original_format,
            conversion_fidelity: &conversion_fidelity,
            conversion_version: conary_core::db::models::CONVERSION_VERSION,
            ccs_content_hash: &content_hash,
            ccs_total_size: total_size,
            publication: report,
        },
    )?;
    let mut summary = conversion_result.scriptlet_metadata.clone();
    summary.review_artifact_path = Some(artifact_path.to_string_lossy().to_string());
    converted.set_scriptlet_metadata(&summary)?;
}
converted.insert(&conn)?;
```

Return `ServerConversionOutcome` instead of raw `ServerConversionResult`:

```rust
let outcome = outcome_from_converted_result(&converted, result);
Ok(outcome)
```

Change `build_result_from_existing(...)` to return `Result<ServerConversionOutcome>` and classify the stored `ConvertedPackage`. Do not turn non-public current rows into `Err`.

- [ ] **Step 6: Extend job status and result shape**

In `apps/remi/src/server/jobs.rs`, update `JobStatus`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Converting,
    Ready,
    ReviewRequired,
    Blocked,
    Failed(String),
}
```

Extend `jobs::ConversionResult`:

```rust
pub scriptlets: crate::server::conversion::ScriptletPackageMetadata,
pub publication: Option<crate::server::publication::PublicationGateReport>,
```

Update terminal checks:

```rust
fn is_terminal_status(status: &JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Ready | JobStatus::ReviewRequired | JobStatus::Blocked | JobStatus::Failed(_)
    )
}
```

Use that helper in `evict_terminal_jobs_for_capacity()` and `update_status()`. Change `complete_with_result`:

```rust
pub fn complete_with_result(&mut self, id: &JobId, status: JobStatus, result: ConversionResult) {
    debug_assert!(matches!(
        &status,
        JobStatus::Ready | JobStatus::ReviewRequired | JobStatus::Blocked
    ));
    if let Some(job) = self.jobs.get_mut(id) {
        job.status = status;
        job.completed_at = Some(Instant::now());
        job.result = Some(result);
    }
}
```

Before running the Task 2 tests, audit every status match site:

```bash
rg -n "JobStatus::|match job.status|matches!\\(.*JobStatus" apps/remi/src/server
```

Update every exhaustive match before trusting the compiler/test failures. The
known required sites are `jobs.rs`, `handlers/jobs.rs`, and
`handlers/packages.rs`.

Extend `JobStats`:

```rust
pub review_required: usize,
pub blocked: usize,
```

Count `ReviewRequired` and `Blocked` separately in `stats()`. Do not increment
`completed` or `failed` for these statuses.

After extending `JobStats`, add these assertions to
`review_required_and_blocked_jobs_are_terminal_not_failed`:

```rust
let stats = manager.stats();
assert_eq!(stats.completed, 0);
assert_eq!(stats.failed, 0);
assert_eq!(stats.review_required, 1);
assert_eq!(stats.blocked, 1);
```

- [ ] **Step 7: Update job handler response**

Modify `apps/remi/src/server/handlers/jobs.rs`:

```rust
pub publication: Option<crate::server::publication::PublicationGateReport>,
```

Map status strings:

```rust
JobStatus::ReviewRequired => "review-required",
JobStatus::Blocked => "blocked",
```

Only include `manifest` for `JobStatus::Ready`. Set:

```rust
let publication = job.result.as_ref().and_then(|result| result.publication.clone());
```

Include `publication` in `JobStatusResponse`. When building the existing ready
manifest JSON, also include the sanitized scriptlet metadata from
`r.scriptlets`:

```rust
"scriptlets": &r.scriptlets,
```

- [ ] **Step 8: Update `run_conversion()`**

In `apps/remi/src/server/handlers/packages.rs`, change the conversion result branch:

```rust
Ok(outcome) => {
    let status = outcome.job_status();
    let conversion_result = outcome.into_result();
    let job_result = crate::server::jobs::ConversionResult {
        chunk_hashes: conversion_result.chunk_hashes,
        total_size: conversion_result.total_size,
        content_hash: conversion_result.content_hash,
        ccs_path: conversion_result.ccs_path,
        actual_version: conversion_result.version,
        scriptlets: conversion_result.scriptlets,
        publication: conversion_result.publication,
    };
    state_guard
        .job_manager
        .complete_with_result(&job_id, status, job_result);
}
```

- [ ] **Step 9: Run tests and commit**

Run:

```bash
cargo test -p remi jobs
cargo test -p remi conversion
cargo test -p remi server_conversion_outcome_reports_terminal_state
```

Expected: pass.

Commit:

```bash
git add apps/remi/src/server/publication.rs apps/remi/src/server/conversion.rs apps/remi/src/server/jobs.rs apps/remi/src/server/handlers/jobs.rs apps/remi/src/server/handlers/packages.rs
git commit -m "feat(remi): thread publication outcomes through jobs"
```

## Task 3: Package Manifest And Download Gates

**Files:**

- Modify: `apps/remi/src/server/handlers/packages.rs`
- Modify: `apps/remi/src/server/publication.rs`

- [ ] **Step 1: Write failing package/download tests**

Add tests to `apps/remi/src/server/handlers/packages.rs`:

```rust
#[test]
fn check_converted_returns_review_refusal_for_current_private_row() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("test.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();
    let ccs_path = temp.path().join("pkg.ccs");
    std::fs::write(&ccs_path, b"fake ccs").unwrap();

    let mut converted = conary_core::db::models::ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
        &["abc".to_string()],
        8,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
    );
    converted
        .set_scriptlet_metadata(&conary_core::ccs::convert::ScriptletBundleSummary {
            publication_status: "private-review".to_string(),
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            review_reason_codes: vec!["review-class-debconf".to_string()],
            ..Default::default()
        })
        .unwrap();
    converted.insert(&conn).unwrap();

    let lookup = check_converted(&db_path, "fedora", "pkg", Some("1.0"), None).unwrap();

    assert!(matches!(lookup, ConvertedManifestLookup::ReviewRequired(_)));
}

#[test]
fn converted_download_lookup_refuses_blocked_rows() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("test.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();
    let ccs_path = temp.path().join("pkg.ccs");
    std::fs::write(&ccs_path, b"fake ccs").unwrap();

    let mut converted = conary_core::db::models::ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
        &["abc".to_string()],
        8,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
    );
    converted
        .set_scriptlet_metadata(&conary_core::ccs::convert::ScriptletBundleSummary {
            publication_status: "blocked".to_string(),
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            blocked_reason_codes: vec!["blocked-class-network".to_string()],
            ..Default::default()
        })
        .unwrap();
    converted.insert(&conn).unwrap();

    let lookup =
        converted_ccs_path_for_download(&db_path, "fedora", "pkg", Some("1.0"), None).unwrap();

    assert!(matches!(lookup, ConvertedDownloadLookup::Blocked(_)));
}
```

- [ ] **Step 2: Run failing package tests**

Run:

```bash
cargo test -p remi check_converted_returns_review_refusal_for_current_private_row
cargo test -p remi converted_download_lookup_refuses_blocked_rows
```

Expected: fail because the typed lookup enums do not exist.

- [ ] **Step 3: Add typed lookup enums**

In `apps/remi/src/server/handlers/packages.rs`, add:

```rust
enum ConvertedManifestLookup {
    Ready(PackageManifest),
    ReviewRequired(crate::server::publication::PublicationGateReport),
    Blocked(crate::server::publication::PublicationGateReport),
    Missing,
}

enum ConvertedDownloadLookup {
    Ready(std::path::PathBuf),
    ReviewRequired(crate::server::publication::PublicationGateReport),
    Blocked(crate::server::publication::PublicationGateReport),
    Missing,
}
```

Change `check_converted(...)` return type to:

```rust
) -> Result<ConvertedManifestLookup, anyhow::Error>
```

For a current row with an existing file:

```rust
match crate::server::publication::classify_converted_package(&converted) {
    PublicationDecision::Ready => Ok(ConvertedManifestLookup::Ready(manifest)),
    PublicationDecision::ReviewRequired(report) => Ok(ConvertedManifestLookup::ReviewRequired(report)),
    PublicationDecision::Blocked(report) => Ok(ConvertedManifestLookup::Blocked(report)),
}
```

Return `ConvertedManifestLookup::Missing` for stale, absent, or missing-file rows.

Change `converted_ccs_path_for_download(...)` return type to:

```rust
) -> Result<ConvertedDownloadLookup, anyhow::Error>
```

Return `Ready(path)` only when the row is public-ready and the file exists.

- [ ] **Step 4: Wire package manifest refusals**

Update `get_package(...)` spawn result handling:

```rust
Ok(Ok(ConvertedManifestLookup::Ready(manifest))) => return Json(manifest).into_response(),
Ok(Ok(ConvertedManifestLookup::ReviewRequired(report))) => {
    return crate::server::publication::refusal_response(
        PublicationRefusal::ReviewRequired(report),
        &distro,
        &name,
        query.version.as_deref(),
    );
}
Ok(Ok(ConvertedManifestLookup::Blocked(report))) => {
    return crate::server::publication::refusal_response(
        PublicationRefusal::Blocked(report),
        &distro,
        &name,
        query.version.as_deref(),
    );
}
Ok(Ok(ConvertedManifestLookup::Missing)) => {}
```

- [ ] **Step 5: Wire download refusals**

In `download_package(...)`, handle job statuses:

```rust
JobStatus::ReviewRequired => {
    if let Some(report) = job.result.as_ref().and_then(|result| result.publication.clone()) {
        return crate::server::publication::refusal_response(
            PublicationRefusal::ReviewRequired(report),
            &distro,
            &name,
            query.version.as_deref(),
        );
    }
    return (StatusCode::CONFLICT, "Conversion requires review").into_response();
}
JobStatus::Blocked => {
    if let Some(report) = job.result.as_ref().and_then(|result| result.publication.clone()) {
        return crate::server::publication::refusal_response(
            PublicationRefusal::Blocked(report),
            &distro,
            &name,
            query.version.as_deref(),
        );
    }
    return (StatusCode::FORBIDDEN, "Conversion blocked").into_response();
}
```

Handle DB lookup results:

```rust
Ok(Ok(ConvertedDownloadLookup::Ready(path))) => path,
Ok(Ok(ConvertedDownloadLookup::ReviewRequired(report))) => {
    return crate::server::publication::refusal_response(
        PublicationRefusal::ReviewRequired(report),
        &distro,
        &name,
        query.version.as_deref(),
    );
}
Ok(Ok(ConvertedDownloadLookup::Blocked(report))) => {
    return crate::server::publication::refusal_response(
        PublicationRefusal::Blocked(report),
        &distro,
        &name,
        query.version.as_deref(),
    );
}
Ok(Ok(ConvertedDownloadLookup::Missing)) => {
    return get_package(State(state), Path((distro, name)), Query(query)).await;
}
```

- [ ] **Step 6: Run tests and commit**

Run:

```bash
cargo test -p remi package_publication
cargo test -p remi check_converted_returns_review_refusal_for_current_private_row
cargo test -p remi converted_download_lookup_refuses_blocked_rows
cargo test -p remi packages
```

Expected: pass. The named unit filters in this task are the concrete gate; keep
`cargo test -p remi packages` as the broader handler regression pass.

Commit:

```bash
git add apps/remi/src/server/handlers/packages.rs apps/remi/src/server/publication.rs
git commit -m "feat(remi): gate package serving by scriptlet policy"
```

## Task 4: Public Metadata, Search, Sparse, Delta, And OCI Discovery Gates

**Files:**

- Modify: `apps/remi/src/server/handlers/index.rs`
- Modify: `apps/remi/src/server/index_gen.rs`
- Modify: `apps/remi/src/server/handlers/detail.rs`
- Modify: `apps/remi/src/server/handlers/sparse.rs`
- Modify: `apps/remi/src/server/federated_index.rs`
- Modify: `apps/remi/src/server/delta_manifests.rs`
- Modify: `apps/remi/src/server/search.rs`
- Modify: `apps/remi/src/server/prewarm.rs`
- Modify: `apps/remi/src/server/handlers/oci.rs`

- [ ] **Step 1: Write failing metadata/index tests**

Update existing tests in `apps/remi/src/server/handlers/index.rs` and `apps/remi/src/server/index_gen.rs` so non-public rows are omitted:

```rust
#[test]
fn metadata_hides_non_public_scriptlet_rows() {
    let (temp_file, conn) = create_test_db();
    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        "gtk3".to_string(),
        "3.24.0".to_string(),
        "sha256:repo".to_string(),
        1024,
        "https://example.com/gtk3.rpm".to_string(),
    );
    repo_pkg.architecture = Some("x86_64".to_string());
    repo_pkg.insert(&conn).unwrap();

    let summary = ScriptletBundleSummary {
        publication_status: "private-review".to_string(),
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        review_reason_codes: vec!["review-class-debconf".to_string()],
        ..Default::default()
    };
    insert_converted_with_summary(&conn, "fedora", "gtk3", "3.24.0", Some("x86_64"), summary);

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
    let pkg = metadata.packages.iter().find(|pkg| pkg.name == "gtk3").unwrap();

    assert!(!pkg.converted);
    assert_eq!(metadata.converted_count, 0);
    assert!(
        pkg.metadata
            .as_ref()
            .and_then(|value| value.get("scriptlets"))
            .is_none()
    );
}

#[test]
fn metadata_omits_converted_only_non_public_rows() {
    let (temp_file, conn) = create_test_db();
    let mut repo = Repository::new("fedora".to_string(), "https://example.com".to_string());
    repo.default_strategy_distro = Some("fedora".to_string());
    repo.insert(&conn).unwrap();

    insert_converted_with_summary(
        &conn,
        "fedora",
        "private-only",
        "1.0",
        Some("x86_64"),
        ScriptletBundleSummary {
            publication_status: "blocked".to_string(),
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            blocked_reason_codes: vec!["blocked-class-network".to_string()],
            ..Default::default()
        },
    );

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();

    assert!(metadata.packages.iter().all(|pkg| pkg.name != "private-only"));
    assert_eq!(metadata.converted_count, 0);
}
```

In generated-index tests:

```rust
#[test]
fn generated_index_omits_converted_only_non_public_rows() {
    let (_temp_file, conn) = create_test_db();
    insert_converted_with_summary(
        &conn,
        "fedora",
        "private-only",
        "1.0",
        Some("x86_64"),
        ScriptletBundleSummary {
            publication_status: "blocked".to_string(),
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            blocked_reason_codes: vec!["blocked-class-network".to_string()],
            ..Default::default()
        },
    );

    let packages = get_packages_for_distro(&conn, "fedora").unwrap();

    assert!(packages.iter().all(|package| package.name != "private-only"));
}
```

Use the existing `create_test_db()` helper in each module where it already
exists. Where a module does not have `insert_converted_with_summary(...)`, add a
small local helper that constructs `ConvertedPackage::new_server(...)`, assigns
`package_architecture`, calls `set_scriptlet_metadata(&summary)`, and inserts the
row. Do not reference placeholder helpers such as `test_connection()` unless
they already exist in that module.

- [ ] **Step 2: Write failing detail/search/sparse/delta/OCI tests**

Add one focused non-public assertion per module. Each test should create at least
one current `ConvertedPackage` with:

```rust
ScriptletBundleSummary {
    publication_status: "private-review".to_string(),
    scriptlet_fidelity: "review-required".to_string(),
    target_compatibility: "review-required".to_string(),
    review_reason_codes: vec!["review-class-debconf".to_string()],
    ..Default::default()
}
```

Use these test names and assertions:

| File | Test | Required assertion |
| --- | --- | --- |
| `apps/remi/src/server/handlers/detail.rs` | `package_detail_counts_only_public_ready_conversions` | Seed one public row and one private-review row; assert detail/overview converted totals count only the public row. |
| `apps/remi/src/server/search.rs` | `search_rebuild_marks_non_public_rows_unconverted` | Seed a repo package plus private-review conversion; after rebuild, assert the indexed result has `converted == false`. |
| `apps/remi/src/server/handlers/sparse.rs` | `sparse_entry_hides_non_public_content_hash` | Seed private-review conversion; assert the sparse version exists but `content_hash.is_none()`. |
| `apps/remi/src/server/federated_index.rs` | `federated_sparse_hides_non_public_content_hash` | Seed private-review conversion; assert the federated sparse version exists but `content_hash.is_none()`. |
| `apps/remi/src/server/delta_manifests.rs` | `delta_manifests_ignore_non_public_conversions` | Seed a private-review converted version; assert `get_version_chunks()` returns no chunks for it and version enumeration omits it. |
| `apps/remi/src/server/handlers/oci.rs` | `oci_tags_catalog_and_manifest_ignore_non_public_rows` | Seed private-review conversion; assert tag list omits the version, catalog omits the repo when no public version remains, and manifest lookup returns missing. |
| `apps/remi/src/server/prewarm.rs` | `prewarm_treats_non_public_current_rows_as_terminal_not_public_ready` | Seed a current private-review row; assert the prewarm existing-conversion check does not schedule reconversion, but reports/counts it separately from public-ready conversions. |

If a module lacks a local helper for summary-bearing converted rows, add this
helper in that module's test section and reuse it from the new test:

```rust
fn insert_private_review_conversion(
    conn: &rusqlite::Connection,
    distro: &str,
    package: &str,
    version: &str,
    chunks: &[String],
) {
    let mut converted = conary_core::db::models::ConvertedPackage::new_server(
        distro.to_string(),
        package.to_string(),
        version.to_string(),
        "rpm".to_string(),
        format!("sha256:{package}-{version}-source"),
        "high".to_string(),
        chunks,
        42,
        format!("sha256:{package}-{version}-content"),
        format!("/tmp/{package}-{version}.ccs"),
    );
    converted
        .set_scriptlet_metadata(&conary_core::ccs::convert::ScriptletBundleSummary {
            publication_status: "private-review".to_string(),
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            review_reason_codes: vec!["review-class-debconf".to_string()],
            ..Default::default()
        })
        .unwrap();
    converted.insert(conn).unwrap();
}
```

- [ ] **Step 3: Run failing discovery tests**

Run:

```bash
cargo test -p remi metadata_hides_non_public_scriptlet_rows
cargo test -p remi metadata_omits_converted_only_non_public_rows
cargo test -p remi generated_index_omits_converted_only_non_public_rows
cargo test -p remi package_detail_counts_only_public_ready_conversions
cargo test -p remi search_rebuild_marks_non_public_rows_unconverted
cargo test -p remi sparse_entry_hides_non_public_content_hash
cargo test -p remi federated_sparse_hides_non_public_content_hash
cargo test -p remi delta_manifests_ignore_non_public_conversions
cargo test -p remi oci_tags_catalog_and_manifest_ignore_non_public_rows
cargo test -p remi prewarm_treats_non_public_current_rows_as_terminal_not_public_ready
```

Expected: fail because existing queries treat current non-public rows as converted-ready.

- [ ] **Step 4: Add a public-ready row filtering helper pattern**

For each module that currently selects rows directly from `converted_packages`, use one of these two patterns:

Pattern A, when the function can load `ConvertedPackage` values:

```rust
if converted.needs_reconversion() || !converted.is_scriptlet_public_ready() {
    continue;
}
```

Pattern B, when the function currently has a narrow SQL projection:

```rust
let candidates = load_converted_candidates(conn, distro, package_name)?;
let public_ready = candidates
    .into_iter()
    .filter(|row| !row.needs_reconversion() && row.is_scriptlet_public_ready())
    .collect::<Vec<_>>();
```

Define `load_converted_candidates` in the module that needs it with a concrete
signature matching that module's parameters. For package-scoped modules, use:

```rust
fn load_converted_candidates(
    conn: &rusqlite::Connection,
    distro: &str,
    package_name: &str,
) -> conary_core::Result<Vec<conary_core::db::models::ConvertedPackage>> {
    conary_core::db::models::ConvertedPackage::find_publication_candidates(
        conn,
        distro,
        Some(package_name),
    )
}
```

Add this helper to `ConvertedPackage` during Task 4 when the second module needs
the same candidate load:

```rust
pub fn find_publication_candidates(
    conn: &Connection,
    distro: &str,
    package_name: Option<&str>,
) -> Result<Vec<Self>> {
    let sql = if package_name.is_some() {
        format!(
            "SELECT {} FROM converted_packages
             WHERE distro = ?1 AND package_name = ?2
               AND conversion_version >= ?3",
            Self::COLUMNS
        )
    } else {
        format!(
            "SELECT {} FROM converted_packages
             WHERE distro = ?1 AND conversion_version >= ?2",
            Self::COLUMNS
        )
    };

    let rows = if let Some(package_name) = package_name {
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_map(params![distro, package_name, CONVERSION_VERSION], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_map(params![distro, CONVERSION_VERSION], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}
```

Do not use `publication_status = 'public'` alone except as a SQL prefilter before the Rust helper.

- [ ] **Step 5: Update `/v1/:distro/metadata`**

In `apps/remi/src/server/handlers/index.rs`, make `load_converted_metadata_rows(...)` load candidate rows and filter:

```rust
let scriptlet_publication = converted.scriptlet_summary_for_publication();
if !scriptlet_publication.valid || scriptlet_publication.summary.publication_status != "public" {
    continue;
}
```

Only public-ready rows should populate:

- `converted_set`
- `converted_scriptlets_by_key`
- converted-only package append loop
- `converted_count`

Remove or update any test that expected private-review scriptlets in public metadata.

- [ ] **Step 6: Update generated indexes**

In `apps/remi/src/server/index_gen.rs`, filter converted rows before adding `VersionEntry { converted: true, content_hash, scriptlets }`:

```rust
if conv.needs_reconversion() || !conv.is_scriptlet_public_ready() {
    continue;
}
```

Only call `ScriptletPackageMetadata::from(&conv.scriptlet_summary())` for public-ready rows.

- [ ] **Step 7: Update detail, sparse, federated sparse, delta, search, and prewarm**

Apply health-aware filtering at these sites:

```text
apps/remi/src/server/handlers/detail.rs
- query_package_detail()
- query_versions_internal()
- query_overview()

apps/remi/src/server/handlers/sparse.rs
- build_sparse_entry()

apps/remi/src/server/federated_index.rs
- build_local_sparse_entry()

apps/remi/src/server/delta_manifests.rs
- get_version_chunks()
- versions_have_current_conversions()
- compute_deltas_for_package()

apps/remi/src/server/search.rs
- SearchEngine::rebuild_from_db()

apps/remi/src/server/prewarm.rs
- is_already_converted()
```

For `prewarm.rs`, preserve terminal-loop avoidance:

```rust
enum ExistingConversionState {
    MissingOrStale,
    PublicReady,
    NonPublicTerminal,
}
```

Use `NonPublicTerminal` to avoid repeated reconversion, but do not report it as public-ready.
The new prewarm test must cover both sides of the distinction: stale rows still
return `MissingOrStale`, while current private-review or blocked rows return
`NonPublicTerminal` and are not counted as public-ready.

- [ ] **Step 8: Update OCI manifest, tag, and catalog discovery**

In `apps/remi/src/server/handlers/oci.rs`:

```rust
let Some(converted) = converted else {
    return None;
};
if converted.needs_reconversion() || !converted.is_scriptlet_public_ready() {
    return None;
}
```

Apply that rule to:

- `build_manifest()`
- `build_tags_list()`
- `build_catalog()`

- [ ] **Step 9: Run tests and commit**

Run:

```bash
cargo test -p remi index
cargo test -p remi detail
cargo test -p remi sparse
cargo test -p remi federated_index
cargo test -p remi delta_manifests
cargo test -p remi search
cargo test -p remi prewarm
cargo test -p remi oci
```

Expected: pass.

Commit:

```bash
git add apps/remi/src/server/handlers/index.rs apps/remi/src/server/index_gen.rs apps/remi/src/server/handlers/detail.rs apps/remi/src/server/handlers/sparse.rs apps/remi/src/server/federated_index.rs apps/remi/src/server/delta_manifests.rs apps/remi/src/server/search.rs apps/remi/src/server/prewarm.rs apps/remi/src/server/handlers/oci.rs
git commit -m "feat(remi): hide non-public conversions from discovery"
```

## Task 5: Raw Chunk And OCI Blob Reachability

**Files:**

- Modify: `crates/conary-core/src/db/models/converted.rs`
- Modify: `apps/remi/src/server/publication.rs`
- Modify: `apps/remi/src/server/handlers/chunks.rs`
- Modify: `apps/remi/src/server/handlers/oci.rs`

- [ ] **Step 1: Write failing chunk reachability tests**

Add to `crates/conary-core/src/db/models/converted.rs`:

```rust
#[test]
fn chunk_public_ready_lookup_requires_at_least_one_public_row() {
    let (_temp, conn) = create_test_db();
    let shared_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut private = ConvertedPackage::new_server(
        "fedora".to_string(),
        "private".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:private".to_string(),
        "high".to_string(),
        &[shared_hash.to_string()],
        10,
        "sha256:private-content".to_string(),
        "/tmp/private.ccs".to_string(),
    );
    private
        .set_scriptlet_metadata(&ScriptletBundleSummary {
            publication_status: "private-review".to_string(),
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            review_reason_codes: vec!["review-class-debconf".to_string()],
            ..Default::default()
        })
        .unwrap();
    private.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::chunk_publication_state(&conn, shared_hash).unwrap(),
        ChunkPublicationState::NonPublicOnly,
    );

    let mut public = ConvertedPackage::new_server(
        "fedora".to_string(),
        "public".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:public".to_string(),
        "high".to_string(),
        &[shared_hash.to_string()],
        10,
        "sha256:public-content".to_string(),
        "/tmp/public.ccs".to_string(),
    );
    public.set_scriptlet_metadata(&ScriptletBundleSummary::default()).unwrap();
    public.insert(&conn).unwrap();

    assert_eq!(
        ConvertedPackage::chunk_publication_state(&conn, shared_hash).unwrap(),
        ChunkPublicationState::PublicReady,
    );
}

#[test]
fn chunk_publication_state_allows_unreferenced_cas_hashes() {
    let (_temp, conn) = create_test_db();

    assert_eq!(
        ConvertedPackage::chunk_publication_state(
            &conn,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .unwrap(),
        ChunkPublicationState::NoConvertedReference,
    );
}
```

Add handler tests in `apps/remi/src/server/handlers/chunks.rs` with these names
and assertions:

| Test | Setup | Required assertion |
| --- | --- | --- |
| `get_chunk_returns_not_found_for_non_public_only_hash` | Create a temp `ServerState`, write a local chunk file for a valid 64-character lowercase SHA-256 test hash such as `aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa`, insert one current private-review row whose `chunk_hashes_json` contains that hash, then call `get_chunk(State(state), Path(hash.to_string()), HeaderMap::new())`. | Response status is `404`, not `200` or redirect. |
| `head_chunk_returns_not_found_for_non_public_only_hash` | Same setup as the GET refusal test, but call `head_chunk(State(state), Path(hash.to_string()))`. | Response status is `404`, not `200`. |
| `get_chunk_allows_hash_shared_with_public_ready_row` | Create the same local chunk file, insert one private-review row and one public-ready row that both reference the valid test hash, then call `get_chunk(...)`. | Response status is `200`. |
| `head_chunk_allows_hash_shared_with_public_ready_row` | Same shared-public setup as the GET allow test, but call `head_chunk(...)`. | Response status is `200`. |
| `get_chunk_allows_unreferenced_protected_local_cache_hash` | Create the same local chunk file, insert a protected `chunk_access` row for the hash, and insert no `converted_packages` row referencing it. | Response status follows existing CAS behavior (`200` for a local chunk), proving Goal 5 does not make unrelated/protected cache objects package-private. |

Add a local helper in the chunk test module:

```rust
async fn chunk_state_with_db(
    hash: &str,
    rows: Vec<conary_core::ccs::convert::ScriptletBundleSummary>,
) -> Arc<RwLock<crate::server::ServerState>> {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("test.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();

    let chunk_dir = temp.path().join("chunks");
    let cache_dir = temp.path().join("cache");
    let chunk_path = crate::server::handlers::cas_object_path(&chunk_dir, hash);
    std::fs::create_dir_all(chunk_path.parent().unwrap()).unwrap();
    std::fs::write(&chunk_path, b"chunk bytes").unwrap();

    for (index, summary) in rows.into_iter().enumerate() {
        let mut converted = conary_core::db::models::ConvertedPackage::new_server(
            "fedora".to_string(),
            format!("pkg-{index}"),
            "1.0".to_string(),
            "rpm".to_string(),
            format!("sha256:source-{index}"),
            "high".to_string(),
            &[hash.to_string()],
            11,
            format!("sha256:content-{index}"),
            format!("/tmp/pkg-{index}.ccs"),
        );
        converted.set_scriptlet_metadata(&summary).unwrap();
        converted.insert(&conn).unwrap();
    }

    let config = crate::server::ServerConfig {
        db_path,
        chunk_dir,
        cache_dir,
        ..Default::default()
    };
    std::fs::create_dir_all(&config.cache_dir).unwrap();
    let state = crate::server::ServerState::new(config).expect("test server state");
    std::mem::forget(temp);
    Arc::new(RwLock::new(state))
}
```

- [ ] **Step 2: Run failing chunk tests**

Run:

```bash
cargo test -p conary-core chunk_public_ready_lookup_requires_at_least_one_public_row
cargo test -p conary-core chunk_publication_state_allows_unreferenced_cas_hashes
cargo test -p remi get_chunk_returns_not_found_for_non_public_only_hash
cargo test -p remi head_chunk_returns_not_found_for_non_public_only_hash
cargo test -p remi get_chunk_allows_hash_shared_with_public_ready_row
cargo test -p remi head_chunk_allows_hash_shared_with_public_ready_row
cargo test -p remi get_chunk_allows_unreferenced_protected_local_cache_hash
```

Expected: fail because reachability helper and handler gate do not exist.

- [ ] **Step 3: Add chunk reachability helper**

Add `ChunkPublicationState` near `ConvertedPackage` and re-export it from
`crates/conary-core/src/db/models/mod.rs` alongside `ConvertedPackage`. Then add
the helper to `impl ConvertedPackage`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkPublicationState {
    NoConvertedReference,
    PublicReady,
    NonPublicOnly,
}

pub fn chunk_publication_state(conn: &Connection, hash: &str) -> Result<ChunkPublicationState> {
    let bare_hash = hash.strip_prefix("sha256:").unwrap_or(hash);
    let prefixed_hash = format!("sha256:{bare_hash}");
    let bare_pattern = format!("%\"{bare_hash}\"%");
    let prefixed_pattern = format!("%\"{prefixed_hash}\"%");

    let mut stmt = conn.prepare(
        "SELECT chunk_hashes_json,
                scriptlet_fidelity, target_compatibility, publication_status,
                evidence_digest, curation_evidence_digest, blocked_reason_codes_json,
                scriptlet_summary_json, review_artifact_path
         FROM converted_packages
         WHERE conversion_version >= ?1
           AND chunk_hashes_json IS NOT NULL
           AND (chunk_hashes_json LIKE ?2 OR chunk_hashes_json LIKE ?3)",
    )?;
    let rows = stmt
        .query_map(params![CONVERSION_VERSION, bare_pattern, prefixed_pattern], |row| {
            ChunkPublicationCandidate::from_row(row)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut saw_converted_reference = false;
    for candidate in rows {
        if !candidate.references_hash(hash) {
            continue;
        }

        saw_converted_reference = true;
        if candidate.is_scriptlet_public_ready() {
            return Ok(ChunkPublicationState::PublicReady);
        }
    }

    Ok(if saw_converted_reference {
        ChunkPublicationState::NonPublicOnly
    } else {
        ChunkPublicationState::NoConvertedReference
    })
}
```

Add a small private `ChunkPublicationCandidate` in `converted.rs` for this
query. It should hold `chunk_hashes_json` plus the scriptlet summary columns
needed to evaluate public readiness. Factor the parse-health logic from
`scriptlet_summary_for_publication()` into a private helper so the candidate can
reuse the exact same `summary_valid && publication_status == "public"` decision
without constructing a full `ConvertedPackage`.

The SQL `LIKE` predicates are a narrowing prefilter, not replacement authority
and not guaranteed to use an index with a leading wildcard. Always parse
`chunk_hashes_json` as `Vec<String>` and perform exact hash/prefixed-hash
comparison in Rust before deciding a row references the chunk. This keeps hot
chunk/blob requests from loading every converted-package row and still avoids
false positives from text matching. If performance still becomes a concern, a
later goal can add a normalized chunk-to-package index.

- [ ] **Step 4: Gate public chunk handlers**

Add to `apps/remi/src/server/publication.rs`:

```rust
use conary_core::db::models::{ChunkPublicationState, ConvertedPackage};

pub fn local_chunk_servable_by_public_gate(
    db_path: &std::path::Path,
    hash: &str,
) -> anyhow::Result<bool> {
    let conn = crate::server::open_runtime_db(db_path)?;
    Ok(!matches!(
        ConvertedPackage::chunk_publication_state(&conn, hash)?,
        ChunkPublicationState::NonPublicOnly
    ))
}
```

In `apps/remi/src/server/handlers/chunks.rs`, check immediately after hash
normalization and before Bloom-filter short-circuiting, upstream pull-through,
`head_chunk` metadata responses, local `get_chunk` file streaming, R2 redirect,
`find_missing` found lists, and `batch_fetch` reads:

```rust
let db_path = state_guard.config.db_path.clone();
let hash_for_lookup = hash.clone();
let is_public = match tokio::task::spawn_blocking(move || {
    crate::server::publication::local_chunk_servable_by_public_gate(&db_path, &hash_for_lookup)
})
.await
{
    Ok(Ok(value)) => value,
    Ok(Err(error)) => {
        tracing::error!("Failed to check chunk publication reachability: {error}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
    }
    Err(error) => {
        tracing::error!("Chunk reachability task failed: {error}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
    }
};
if !is_public {
    return chunk_not_found();
}
```

For `find_missing`, place non-public-only hashes in `missing`. For `batch_fetch`, place them in `missing` and do not read bytes.
The gate must run before `get_chunk` can call `pull_through_fetch(...)`; a
configured upstream must not fetch and cache a hash that is reachable only from a
current non-public converted row.

- [ ] **Step 5: Gate OCI blob handlers**

In `apps/remi/src/server/handlers/oci.rs`, apply the same helper in `get_blob_inner(...)` and `head_blob_inner(...)` before streaming or acknowledging local blob data:

```rust
let db_path = state_guard.config.db_path.clone();
let hash_for_lookup = hash.clone();
let chunk_allowed = match tokio::task::spawn_blocking(move || {
    crate::server::publication::local_chunk_servable_by_public_gate(&db_path, &hash_for_lookup)
})
.await
{
    Ok(Ok(value)) => value,
    Ok(Err(error)) => {
        tracing::error!("Failed to check OCI blob publication reachability: {error}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
    }
    Err(error) => {
        tracing::error!("OCI blob reachability task failed: {error}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
    }
};
if !chunk_allowed {
    return oci_error_response(StatusCode::NOT_FOUND, "BLOB_UNKNOWN", "Blob not found");
}
```

Use the actual OCI error helper shape in the file. Do not use `?` inside handler
functions that return `Response`; either move the fallible gate into a helper
returning `anyhow::Result<Response>` or handle `Ok/Err` explicitly in the
handler, as shown above. Do not call the synchronous SQLite helper directly on
the async runtime thread.

Add OCI blob handler tests mirroring the raw chunk cases:

| Test | Setup | Required assertion |
| --- | --- | --- |
| `oci_blob_returns_not_found_for_non_public_only_hash` | Create a temp OCI server state, write a local chunk/blob file for the same kind of valid 64-character hash used by the raw chunk tests, insert one current private-review row whose chunks contain the hash, then call `get_blob_inner(...)` or the router path with `sha256:{hash}`. | Response status is `404` with `BLOB_UNKNOWN`. |
| `oci_head_blob_returns_not_found_for_non_public_only_hash` | Same setup for the `HEAD` path. | Response status is `404`, not `200`. |
| `oci_blob_allows_hash_shared_with_public_ready_row` | Insert private-review and public-ready rows that both reference the hash. | `GET` returns `200`. |

- [ ] **Step 6: Run tests and commit**

Run:

```bash
cargo test -p conary-core chunk_public_ready_lookup_requires_at_least_one_public_row
cargo test -p conary-core chunk_publication_state_allows_unreferenced_cas_hashes
cargo test -p remi chunks
cargo test -p remi oci
cargo test -p remi head_chunk_returns_not_found_for_non_public_only_hash
cargo test -p remi head_chunk_allows_hash_shared_with_public_ready_row
cargo test -p remi oci_blob_returns_not_found_for_non_public_only_hash
cargo test -p remi oci_head_blob_returns_not_found_for_non_public_only_hash
cargo test -p remi oci_blob_allows_hash_shared_with_public_ready_row
```

Expected: pass.

Commit:

```bash
git add crates/conary-core/src/db/models/converted.rs apps/remi/src/server/publication.rs apps/remi/src/server/handlers/chunks.rs apps/remi/src/server/handlers/oci.rs
git commit -m "feat(remi): gate chunk blobs by public reachability"
```

## Task 6: Admin Review Artifacts And CCS Upload Gate

**Files:**

- Modify: `apps/remi/src/server/publication.rs`
- Modify: `apps/remi/src/server/handlers/admin/packages.rs`
- Modify: `apps/remi/src/server/routes/admin.rs`

- [ ] **Step 1: Write failing admin tests**

Add tests to `apps/remi/src/server/handlers/admin/packages.rs`:

```rust
#[tokio::test]
async fn admin_review_artifact_requires_admin_scope() {
    let (app, _db_path) = crate::server::handlers::admin::test_helpers::test_app().await;

    let response = tower::ServiceExt::oneshot(
        app,
        axum::http::Request::builder()
            .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_review_artifact_rejects_paths_outside_review_root() {
    let (app, db_path) = crate::server::handlers::admin::test_helpers::test_app().await;
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let mut converted = conary_core::db::models::ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
        &["abc".to_string()],
        3,
        "sha256:content".to_string(),
        "/tmp/pkg.ccs".to_string(),
    );
    let mut summary = conary_core::ccs::convert::ScriptletBundleSummary {
        publication_status: "private-review".to_string(),
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        review_reason_codes: vec!["review-class-debconf".to_string()],
        ..Default::default()
    };
    summary.review_artifact_path = Some("/etc/passwd".to_string());
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.insert(&conn).unwrap();

    let response = tower::ServiceExt::oneshot(
        app,
        axum::http::Request::builder()
            .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0")
            .header("Authorization", "Bearer test-admin-token-12345")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_upload_with_blocked_bundle_stores_non_public_metadata() {
    let (app, db_path) = crate::server::handlers::admin::test_helpers::test_app().await;
    let archive = blocked_scriptlet_ccs_fixture();

    let response = tower::ServiceExt::oneshot(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/admin/packages/fedora")
            .header("Authorization", "Bearer test-admin-token-12345")
            .body(axum::body::Body::from(archive))
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let converted = conary_core::db::models::ConvertedPackage::find_by_package_identity(
        &conn,
        "fedora",
        "blocked-scriptlet-fixture",
        Some("1.0"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(converted.publication_status, "blocked");
    assert!(converted.review_artifact_path.is_some());
    let expected_digest = conary_core::hash::sha256_prefixed(b"fixture-evidence");
    assert_eq!(converted.evidence_digest.as_deref(), Some(expected_digest.as_str()));
    let artifact_path = std::path::PathBuf::from(converted.review_artifact_path.clone().unwrap());
    let artifact: serde_json::Value =
        serde_json::from_slice(&std::fs::read(artifact_path).unwrap()).unwrap();
    assert_eq!(artifact["schema"], "conary.remi.scriptlet-review.v1");
    assert_eq!(
        artifact["publication"]["evidence_digest"].as_str(),
        Some(expected_digest.as_str())
    );
    assert!(!serde_json::to_string(&artifact).unwrap().contains("review_artifact_path"));
}

#[tokio::test]
async fn admin_review_artifact_lookup_is_arch_specific_and_reports_stale_rows() {
    let (app, db_path) = crate::server::handlers::admin::test_helpers::test_app().await;
    seed_review_artifact_row(&db_path, "pkg", "1.0", Some("x86_64"), "current.json", false);
    seed_review_artifact_row(&db_path, "pkg", "1.0", Some("aarch64"), "stale.json", true);

    let current = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0&arch=x86_64")
            .header("Authorization", "Bearer test-admin-token-12345")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(current.status(), StatusCode::OK);

    let stale = tower::ServiceExt::oneshot(
        app,
        axum::http::Request::builder()
            .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0&arch=aarch64")
            .header("Authorization", "Bearer test-admin-token-12345")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
}
```

Add this helper below the admin package tests:

```rust
fn blocked_scriptlet_ccs_fixture() -> Vec<u8> {
    use conary_core::ccs::builder::{CcsBuilder, write_ccs_package};
    use conary_core::ccs::legacy_scriptlets::{
        DecisionCounts, ForeignReplayPolicy, LegacyScriptletBundle, PublicationPolicy,
        PublicationStatus, ScriptletFidelity, SourceFormat, TargetCompatibility, VersionScheme,
    };

    let temp = tempfile::tempdir().unwrap();
    let mut manifest =
        conary_core::ccs::manifest::CcsManifest::new_minimal("blocked-scriptlet-fixture", "1.0");
    manifest.legacy_scriptlets = Some(LegacyScriptletBundle {
        schema: conary_core::ccs::legacy_scriptlets::LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 1,
        source_format: SourceFormat::Rpm,
        source_family: "rpm".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: None,
        source_arch: Some("x86_64".to_string()),
        source_package: "blocked-scriptlet-fixture".to_string(),
        source_version: "1.0".to_string(),
        source_checksum: Some(conary_core::hash::sha256_prefixed(b"fixture-source")),
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "test".to_string(),
        conversion_policy: "publication-gate-test".to_string(),
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: Some(conary_core::hash::sha256_prefixed(b"fixture-evidence")),
        target_compatibility: TargetCompatibility::Blocked,
        allowed_targets: Vec::new(),
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy: PublicationPolicy::Blocked,
        publication_status: PublicationStatus::Blocked,
        scriptlet_fidelity: ScriptletFidelity::Blocked,
        decision_counts: DecisionCounts {
            blocked: 0,
            ..DecisionCounts::default()
        },
        unsupported_class_counts: std::collections::BTreeMap::new(),
        entries: Vec::new(),
        extra: std::collections::BTreeMap::new(),
    });

    std::fs::write(temp.path().join("payload.txt"), b"fixture").unwrap();
    let path = temp.path().join("blocked.ccs");
    let result = CcsBuilder::new(manifest, temp.path()).build().expect("fixture build");
    write_ccs_package(&result, &path).expect("fixture CCS package");
    std::fs::read(path).expect("fixture bytes")
}
```

Also add a small `seed_review_artifact_row(...)` test helper that creates a
sanitized artifact under the test app's configured `scriptlet-review` root,
inserts a matching `ConvertedPackage` with `review_artifact_path`, optional
`package_architecture`, and either the current `CONVERSION_VERSION` or one lower
for the stale-row assertion. The helper should not place artifact files outside
the configured review root.

- [ ] **Step 2: Run failing admin tests**

Run:

```bash
cargo test -p remi admin_review_artifact_requires_admin_scope
cargo test -p remi admin_review_artifact_rejects_paths_outside_review_root
cargo test -p remi admin_upload_with_blocked_bundle_stores_non_public_metadata
cargo test -p remi admin_review_artifact_lookup_is_arch_specific_and_reports_stale_rows
```

Expected: fail because the route/handler and upload gate do not exist.

- [ ] **Step 3: Verify review artifact helpers and path validation**

Task 2 introduced these helpers because fresh conversion persistence needs them.
If they are not already present, add them now; otherwise verify they match this
contract before wiring admin retrieval:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct ScriptletReviewArtifact {
    pub schema: &'static str,
    pub distro: String,
    pub package: String,
    pub version: String,
    pub architecture: Option<String>,
    pub original_format: String,
    pub publication: PublicationGateReport,
    pub conversion_fidelity: String,
    pub conversion_version: i32,
    pub ccs_content_hash: String,
    pub ccs_total_size: u64,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ReviewArtifactInput<'a> {
    pub distro: &'a str,
    pub package: &'a str,
    pub version: &'a str,
    pub architecture: Option<&'a str>,
    pub original_format: &'a str,
    pub conversion_fidelity: &'a str,
    pub conversion_version: i32,
    pub ccs_content_hash: &'a str,
    pub ccs_total_size: u64,
    pub publication: PublicationGateReport,
}

pub fn review_artifact_root(cache_dir: &std::path::Path) -> std::path::PathBuf {
    cache_dir.join("scriptlet-review")
}

pub fn write_review_artifact(
    cache_dir: &std::path::Path,
    input: ReviewArtifactInput<'_>,
) -> anyhow::Result<std::path::PathBuf> {
    let digest = input
        .publication
        .evidence_digest
        .as_deref()
        .unwrap_or("missing-evidence-digest")
        .replace(':', "-");
    let dir = review_artifact_root(cache_dir)
        .join(sanitize_component(input.distro))
        .join(sanitize_component(input.package))
        .join(sanitize_component(input.version))
        .join(sanitize_component(input.architecture.unwrap_or("noarch")));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{digest}.json"));
    let temp_path = dir.join(format!("{digest}.json.tmp"));
    let artifact = ScriptletReviewArtifact {
        schema: "conary.remi.scriptlet-review.v1",
        distro: input.distro.to_string(),
        package: input.package.to_string(),
        version: input.version.to_string(),
        architecture: input.architecture.map(str::to_string),
        original_format: input.original_format.to_string(),
        publication: input.publication,
        conversion_fidelity: input.conversion_fidelity.to_string(),
        conversion_version: input.conversion_version,
        ccs_content_hash: input.ccs_content_hash.to_string(),
        ccs_total_size: input.ccs_total_size,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let bytes = serde_json::to_vec_pretty(&artifact)?;
    std::fs::write(&temp_path, bytes)?;
    std::fs::rename(&temp_path, &path)?;
    Ok(path)
}

pub fn validate_review_artifact_path(
    cache_dir: &std::path::Path,
    path: &std::path::Path,
) -> anyhow::Result<bool> {
    let root = review_artifact_root(cache_dir);
    let canonical_root = root.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    Ok(canonical_path.starts_with(canonical_root))
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}
```

- [ ] **Step 4: Add admin review-artifact handler**

In `apps/remi/src/server/handlers/admin/packages.rs`, add:

```rust
#[derive(Debug, Deserialize)]
pub struct ReviewArtifactQuery {
    pub version: String,
    pub arch: Option<String>,
}

enum ReviewArtifactLookup {
    Found(String),
    Stale,
    Ambiguous,
    Missing,
}

struct ReviewArtifactRow {
    conversion_version: i32,
    review_artifact_path: Option<String>,
}

fn matching_review_artifact_rows(
    conn: &rusqlite::Connection,
    distro: &str,
    package: &str,
    version: &str,
    arch: Option<&str>,
) -> anyhow::Result<Vec<ReviewArtifactRow>> {
    let rows = if let Some(arch) = arch {
        let mut stmt = conn.prepare(
            "SELECT conversion_version, review_artifact_path
             FROM converted_packages
             WHERE distro = ?1
               AND package_name = ?2
               AND package_version = ?3
               AND package_architecture = ?4
             ORDER BY converted_at DESC",
        )?;
        stmt
            .query_map(rusqlite::params![distro, package, version, arch], |row| {
                Ok(ReviewArtifactRow {
                    conversion_version: row.get(0)?,
                    review_artifact_path: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let mut stmt = conn.prepare(
            "SELECT conversion_version, review_artifact_path
             FROM converted_packages
             WHERE distro = ?1
               AND package_name = ?2
               AND package_version = ?3
             ORDER BY converted_at DESC",
        )?;
        stmt
            .query_map(rusqlite::params![distro, package, version], |row| {
                Ok(ReviewArtifactRow {
                    conversion_version: row.get(0)?,
                    review_artifact_path: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    Ok(rows)
}

pub async fn get_scriptlet_review_artifact(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, package)): Path<(String, String)>,
    Query(query): Query<ReviewArtifactQuery>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }
    for (value, name) in [(&distro, "distro"), (&package, "package")] {
        if let Some(err) = validate_path_param(value, name) {
            return err;
        }
    }

    let (db_path, cache_dir) = {
        let guard = state.read().await;
        (guard.config.db_path.clone(), guard.config.cache_dir.clone())
    };
    let version = query.version.clone();
    let arch = query.arch.clone();

    let lookup = tokio::task::spawn_blocking(move || -> anyhow::Result<ReviewArtifactLookup> {
        let conn = crate::server::open_runtime_db(&db_path)?;
        let rows =
            matching_review_artifact_rows(&conn, &distro, &package, &version, arch.as_deref())?;
        if rows.is_empty() {
            return Ok(ReviewArtifactLookup::Missing);
        }

        let current_rows = rows
            .iter()
            .filter(|row| row.conversion_version >= conary_core::db::models::CONVERSION_VERSION)
            .collect::<Vec<_>>();
        if current_rows.is_empty() {
            return Ok(ReviewArtifactLookup::Stale);
        }
        if arch.is_none() && current_rows.len() > 1 {
            return Ok(ReviewArtifactLookup::Ambiguous);
        }

        let row = current_rows[0];

        Ok(row
            .review_artifact_path
            .clone()
            .map(ReviewArtifactLookup::Found)
            .unwrap_or(ReviewArtifactLookup::Missing))
    })
    .await;

    let path = match lookup {
        Ok(Ok(ReviewArtifactLookup::Found(path))) => std::path::PathBuf::from(path),
        Ok(Ok(ReviewArtifactLookup::Stale)) => {
            return json_error(409, "Converted package needs reconversion", "STALE_CONVERSION");
        }
        Ok(Ok(ReviewArtifactLookup::Ambiguous)) => {
            return json_error(409, "Architecture is required for this package/version", "AMBIGUOUS_ARCHITECTURE");
        }
        Ok(Ok(ReviewArtifactLookup::Missing)) => {
            return json_error(404, "Review artifact not found", "NOT_FOUND");
        }
        Ok(Err(error)) => {
            tracing::error!("Failed to load review artifact path: {error}");
            return json_error(500, "Failed to load review artifact", "DB_ERROR");
        }
        Err(error) => {
            tracing::error!("Review artifact lookup task failed: {error}");
            return json_error(500, "Failed to load review artifact", "INTERNAL_ERROR");
        }
    };

    match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return json_error(403, "Review artifact path is not a file", "FORBIDDEN"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return json_error(404, "Review artifact not found", "NOT_FOUND");
        }
        Err(error) => {
            tracing::warn!("Review artifact metadata lookup failed: {error}");
            return json_error(500, "Failed to read review artifact", "INTERNAL_ERROR");
        }
    }

    match crate::server::publication::validate_review_artifact_path(&cache_dir, &path) {
        Ok(true) => {}
        Ok(false) => return json_error(403, "Review artifact path is outside review root", "FORBIDDEN"),
        Err(error) => {
            tracing::warn!("Review artifact path validation failed: {error}");
            return json_error(403, "Review artifact path is invalid", "FORBIDDEN");
        }
    }

    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => json_error(404, "Review artifact not found", "NOT_FOUND"),
    }
}
```

In `apps/remi/src/server/routes/admin.rs`, add the route to both admin routers:

Keep `matching_review_artifact_rows(...)` in the admin handler module and query
only the columns shown above. Do not call `ConvertedPackage::from_row` from this
module; it is private to `converted.rs`, and the review artifact route does not
need a full `ConvertedPackage`.

```rust
.route(
    "/v1/admin/packages/{distro}/{package}/scriptlet-review",
    get(admin_handlers::get_scriptlet_review_artifact),
)
```

- [ ] **Step 5: Gate admin CCS uploads**

In `apps/remi/src/server/handlers/admin/packages.rs`, after inspection:

```rust
let mut upload_summary = match inspected.manifest.legacy_scriptlets.as_ref() {
    Some(bundle) => {
        if let Err(error) = bundle.validate() {
            tracing::warn!("Uploaded CCS has invalid legacy scriptlet bundle: {error}");
            return json_error(400, "Invalid legacy scriptlet bundle", "INVALID_SCRIPTLETS");
        }
        conary_core::ccs::convert::ScriptletBundleSummary::from_bundle(
            bundle,
            bundle.evidence_digest.clone(),
        )
    }
    None => conary_core::ccs::convert::ScriptletBundleSummary::default(),
};
```

Before `atomic_replace_record(...)`, classify and possibly write artifact:

```rust
let upload_architecture = inspected
    .manifest
    .package
    .platform
    .as_ref()
    .and_then(|platform| platform.arch.as_deref())
    .map(str::to_string);
let publication = crate::server::publication::classify_summary(
    conary_core::db::models::ScriptletSummaryForPublication {
        summary: upload_summary.clone(),
        valid: true,
    },
);
if let Some(refusal) = crate::server::publication::decision_refusal(publication) {
    let mut report = match refusal {
        PublicationRefusal::ReviewRequired(report) | PublicationRefusal::Blocked(report) => report,
    };
    report.review_artifact_available = true;
    let artifact_path = match crate::server::publication::write_review_artifact(
        &cache_dir,
        crate::server::publication::ReviewArtifactInput {
            distro: &distro,
            package: &package_name,
            version: &package_version,
            architecture: upload_architecture.as_deref(),
            original_format: "ccs",
            conversion_fidelity: "uploaded",
            conversion_version: conary_core::db::models::CONVERSION_VERSION,
            ccs_content_hash: &content_hash,
            ccs_total_size: total_size,
            publication: report,
        },
    ) {
        Ok(path) => path,
        Err(error) => {
            tracing::error!("Failed to write upload review artifact: {error}");
            return json_error(500, "Failed to write review artifact", "REVIEW_ARTIFACT_ERROR");
        }
    };
    upload_summary.review_artifact_path = Some(artifact_path.to_string_lossy().to_string());
}
```

Expand `atomic_replace_record(...)` parameters:

```rust
package_architecture: Option<String>,
scriptlet_summary: conary_core::ccs::convert::ScriptletBundleSummary,
```

Inside the transaction, use architecture-aware identity and carry the uploaded
platform architecture into the replacement row:

```rust
let existing = conary_core::db::models::ConvertedPackage::find_by_package_identity_with_arch(
    tx,
    &distro,
    &package_name,
    Some(&package_version),
    package_architecture.as_deref(),
)?;

converted.package_architecture = package_architecture.clone();
converted.set_scriptlet_metadata(&scriptlet_summary)?;
```

If the uploaded CCS has no `package.platform.arch`, pass `None` and preserve the
existing single-identity upload behavior.
Update the existing `atomic_replace_record(...)` call site to pass
`upload_architecture.clone()` and `upload_summary.clone()` before the staged path
commit completes.

If the DB transaction fails after writing a review artifact, remove the new artifact path:

```rust
if let Some(path) = upload_summary.review_artifact_path.as_ref() {
    let _ = tokio::fs::remove_file(path).await;
}
```

- [ ] **Step 6: Run tests and commit**

Run:

```bash
cargo test -p remi admin
cargo test -p remi admin_review_artifact_requires_admin_scope
cargo test -p remi admin_review_artifact_rejects_paths_outside_review_root
cargo test -p remi admin_upload_with_blocked_bundle_stores_non_public_metadata
```

Expected: pass.

Commit:

```bash
git add apps/remi/src/server/publication.rs apps/remi/src/server/handlers/admin/packages.rs apps/remi/src/server/routes/admin.rs
git commit -m "feat(remi): add scriptlet review artifacts"
```

## Task 7: Documentation And Final Verification

**Files:**

- Modify: `docs/modules/remi.md`
- Modify: `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`

- [ ] **Step 1: Update Remi module docs**

In `docs/modules/remi.md`, add a section near the conversion/publication discussion:

```markdown
### Legacy Scriptlet Publication Gate

Remi treats legacy scriptlet metadata embedded during conversion as an active
serving gate. Converted rows whose scriptlet summary is valid and has
`publication_status = "public"` may be advertised, indexed, and served. Rows
with `private-review`, `blocked`, `local-only`, malformed summary JSON, or
non-default scriptlet evidence without an explicit summary are terminal
review/blocked conversion outcomes and are not public-ready.

This gate is publication-only. It does not replay scriptlets, promote reviewed
packages, or change client install/update/remove behavior.
```

- [ ] **Step 2: Update goal queue status**

In `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`, update Goal 5 from planned to implemented after Tasks 1-6 pass. If the goal queue records commit ranges, leave the range blank until the merge/push cleanup turn supplies the final range.

- [ ] **Step 3: Run focused test suite**

Run:

```bash
cargo test -p conary-core converted
cargo test -p conary-core scriptlet_bundle_summary_from_bundle_is_public_api
cargo test -p remi publication
cargo test -p remi jobs
cargo test -p remi package_publication
cargo test -p remi conversion
cargo test -p remi index
cargo test -p remi detail
cargo test -p remi sparse
cargo test -p remi federated_index
cargo test -p remi chunks
cargo test -p remi oci
cargo test -p remi delta_manifests
cargo test -p remi search
cargo test -p remi prewarm
cargo test -p remi admin
```

Expected: pass.

- [ ] **Step 4: Run workspace hygiene**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p conary-core
cargo test -p remi
git diff --check
```

Expected: all pass.

- [ ] **Step 5: Scope audit**

Run:

```bash
git diff --stat origin/main..HEAD
rg -n "legacy replay|execute scriptlet|sandbox|promotion|approve|local-only" apps crates docs/modules/remi.md docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md
```

Expected:

- diff is limited to Remi publication/serving policy, core summary helpers, docs, and tests;
- no install/update/remove code is modified;
- no scriptlet replay, sandbox execution, or operator promotion workflow was introduced;
- references to `local-only` are only defensive policy handling, not emitted promotion behavior.

- [ ] **Step 6: Commit**

Commit:

```bash
git add docs/modules/remi.md docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md
git commit -m "docs(remi): document scriptlet publication gate"
```

## Final Review Checklist

Before merging Goal 5, verify each design requirement maps to code:

- Public-ready predicate is `summary_valid && publication_status == "public"`.
- `{}` summary JSON is valid only for the narrow no-bundle/default shape.
- `private-review` returns `409` structured refusal.
- `blocked` returns `403` structured refusal.
- Review/blocked conversions are terminal jobs, not `Failed`.
- Hot cache returns ready/review/blocked outcomes without reconversion loops.
- Package manifests, downloads, metadata, generated indexes, detail pages, search, sparse, federated sparse, delta, OCI tags/catalog/manifests, raw chunks, and OCI blobs all avoid non-public-ready exposure.
- Raw chunk/blob gating blocks non-public converted-package bytes without blocking unrelated local/protected CAS objects.
- Review artifacts are private, versioned, atomically written, admin-only, architecture-aware, and path-validated under `<cache_dir>/scriptlet-review`.
- Admin CCS upload inspects embedded `legacy_scriptlets` and applies the same gate.
- Public JSON never contains `review_artifact_path` or private local paths.
- No database migration was added.
- No replay, curation promotion, or installer behavior was added.
