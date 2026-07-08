# File Capability Public Policy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep `file-capability/v1` adapter replacement evidence intact while making only `cap_net_bind_service` public-ready by default.

**Architecture:** Add a conversion-publication policy helper under `crates/conary-core/src/ccs/convert/` so manifest validation remains the broad "known Linux capability" syntax layer. Apply that helper during legacy scriptlet bundle aggregation: complete high-risk `file-capability/v1` effects stay `replaced`, still project into `[[file_capabilities]]`, but set aggregate `publication_status = "private-review"`. Bump converted-row policy version and tighten publication summary shape validation so stale cached public rows cannot bypass the stricter policy.

**Tech Stack:** Rust, serde JSON/TOML metadata, rusqlite converted-row model tests, cargo test, docs-audit checks.

## Global Constraints

- Public-ready file capabilities are allowlisted to exactly `cap_net_bind_service` in this slice.
- `file-capability/v1` continues to classify known `setcap cap_*=+ep <payload-executable>` forms as complete adapter replacement evidence.
- Known high-risk capabilities remain valid manifest/install syntax and still project into `[[file_capabilities]]`.
- Known high-risk capabilities must make aggregate bundle/publication status `private-review` unless a future target-profile slice explicitly allows them.
- Do not add target-profile file-capability overrides in this slice.
- Unknown capabilities, inheritable flags, non-`+ep` forms, removals, non-payload paths, setgid, broad chmod, and non-payload privilege mutations remain blocked or private-review under existing rules.
- Bump `CONVERSION_VERSION` from `4` to `5` and ensure stale converted rows are never public-ready.
- Newly written non-default scriptlet publication summaries must include `security_policy_intents`; constructor default `{}` compatibility remains valid only for true unknown/public rows with no evidence.
- Do not change `setuid-mode/v1` policy behavior in this slice.
- Keep docs and fixture metadata aligned with `docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md`.

---

## File Structure

- Create `crates/conary-core/src/ccs/convert/public_policy.rs`
  - Own the first public-ready file capability allowlist.
  - Export crate-private helpers for file-capability public policy review reasons.
- Modify `crates/conary-core/src/ccs/convert/mod.rs`
  - Register the new private `public_policy` module.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`
  - Apply public-policy review reasons during aggregate bundle/publication status construction.
  - Merge public-policy review reason codes into `ScriptletBundleSummary.review_reason_codes`.
- Modify `crates/conary-core/src/ccs/convert/converter.rs`
  - Add conversion integration tests proving allowed and high-risk file capabilities keep replacement/projection semantics while public status differs.
- Modify `crates/conary-core/src/ccs/convert/golden_fixtures.rs`
  - Add a private-review high-risk file-capability fixture alongside the public-ready allowlisted fixture.
- Modify `crates/conary-core/src/ccs/convert/support_matrix.rs`
  - Let the `file-capability/v1` row declare both public-ready and private-review fixture evidence.
  - Keep known adapter rows accountable for at least one public-ready fixture.
- Modify `crates/conary-core/src/db/models/converted.rs`
  - Bump `CONVERSION_VERSION` to `5`.
  - Make `ConvertedPackage::is_scriptlet_public_ready()` fail closed for stale rows.
  - Require `security_policy_intents` in non-default summary JSON shape validation.
- Modify `docs/SCRIPTLET_SECURITY.md`
  - Document the public allowlist and high-risk private-review behavior.
- Modify `docs/modules/ccs.md`
  - Clarify known Linux capability syntax versus public-ready capability policy.
- Modify `docs/modules/remi.md`
  - Clarify that high-risk file-capability conversions remain non-public even when fully replaced.
- Modify `docs/modules/test-fixtures.md`
  - Register the public and private-review file-capability fixture split.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Register this plan and the implementation claim updates.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Regenerate after staging this new plan.

## Task 1: Add File-Capability Public Policy Helper

**Files:**
- Create: `crates/conary-core/src/ccs/convert/public_policy.rs`
- Modify: `crates/conary-core/src/ccs/convert/mod.rs`

**Interfaces:**
- Produces: `FILE_CAPABILITY_PUBLIC_REVIEW_REASON: &str`
- Produces: `file_capability_public_review_reason(capabilities: &[String]) -> Option<&'static str>`
- Produces: `entry_public_policy_review_reasons(entry: &LegacyScriptletEntry) -> Vec<String>`

- [ ] **Step 1: Write the failing helper tests**

Create `crates/conary-core/src/ccs/convert/public_policy.rs` with the path comment and these tests first:

```rust
// conary-core/src/ccs/convert/public_policy.rs

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn cap_net_bind_service_is_public_ready_by_default() {
        assert_eq!(
            file_capability_public_review_reason(&caps(&["cap_net_bind_service"])),
            None
        );
    }

    #[test]
    fn high_risk_known_capabilities_require_private_review() {
        for capability in [
            "cap_sys_admin",
            "cap_sys_module",
            "cap_sys_rawio",
            "cap_sys_boot",
            "cap_sys_ptrace",
            "cap_bpf",
            "cap_net_admin",
            "cap_setpcap",
            "cap_setfcap",
        ] {
            assert_eq!(
                file_capability_public_review_reason(&caps(&[capability])),
                Some(FILE_CAPABILITY_PUBLIC_REVIEW_REASON),
                "{capability}"
            );
        }
    }

    #[test]
    fn mixed_public_and_private_capabilities_require_private_review() {
        assert_eq!(
            file_capability_public_review_reason(&caps(&[
                "cap_net_bind_service",
                "cap_sys_admin",
            ])),
            Some(FILE_CAPABILITY_PUBLIC_REVIEW_REASON)
        );
    }
}
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core public_policy
```

Expected: FAIL because the constants and helper function do not exist yet.

- [ ] **Step 3: Implement the minimal helper**

In `crates/conary-core/src/ccs/convert/mod.rs`, add the private module:

```rust
mod public_policy;
```

In `crates/conary-core/src/ccs/convert/public_policy.rs`, keep the tests and add:

```rust
use crate::ccs::legacy_scriptlets::LegacyScriptletEntry;
use std::collections::BTreeSet;

pub(crate) const FILE_CAPABILITY_PUBLIC_REVIEW_REASON: &str =
    "public-policy-file-capability-private-review";

const PUBLIC_READY_FILE_CAPABILITIES: &[&str] = &["cap_net_bind_service"];

pub(crate) fn file_capability_public_review_reason(
    capabilities: &[String],
) -> Option<&'static str> {
    let all_public_ready = !capabilities.is_empty()
        && capabilities
            .iter()
            .all(|capability| PUBLIC_READY_FILE_CAPABILITIES.contains(&capability.as_str()));

    (!all_public_ready).then_some(FILE_CAPABILITY_PUBLIC_REVIEW_REASON)
}

pub(crate) fn entry_public_policy_review_reasons(
    entry: &LegacyScriptletEntry,
) -> Vec<String> {
    let mut reasons = BTreeSet::new();

    for effect in &entry.effects {
        if effect.adapter_id.as_deref() != Some("file-capability/v1")
            || effect.kind != "file-capability"
        {
            continue;
        }

        let capabilities = effect
            .extra
            .get("capabilities")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(toml::Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();

        if let Some(reason) = file_capability_public_review_reason(&capabilities) {
            reasons.insert(reason.to_string());
        }
    }

    reasons.into_iter().collect()
}
```

- [ ] **Step 4: Verify the helper tests pass**

Run:

```bash
cargo test -p conary-core public_policy
```

Expected: PASS.

- [ ] **Step 5: Commit Task 1**

Run:

```bash
git add crates/conary-core/src/ccs/convert/mod.rs crates/conary-core/src/ccs/convert/public_policy.rs
git commit -m "security: add file capability public policy helper"
```

## Task 2: Apply Public Policy During Bundle Aggregation

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`
- Modify: `crates/conary-core/src/ccs/convert/converter.rs`

**Interfaces:**
- Consumes: `public_policy::entry_public_policy_review_reasons(...)`
- Produces: high-risk file-capability bundles with `scriptlet_fidelity = "fully-replaced"`, `decision_counts.replaced = 1`, and `publication_status = "private-review"`

- [ ] **Step 1: Add the failing high-risk conversion test**

In `crates/conary-core/src/ccs/convert/converter.rs`, place this test next to `conversion_integration_projects_payload_backed_setcap_into_file_capability_authority`:

```rust
#[test]
fn conversion_integration_keeps_high_risk_setcap_replaced_but_private_review() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "setcap cap_sys_admin=+ep /usr/bin/test\n".to_string(),
        flags: None,
    }];
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &make_test_files(), "rpm", "sha256:test")
        .expect("conversion succeeds");

    assert_eq!(result.scriptlet_metadata.scriptlet_fidelity, "fully-replaced");
    assert_eq!(result.scriptlet_metadata.target_compatibility, "conary-portable");
    assert_eq!(result.scriptlet_metadata.publication_status, "private-review");
    assert_eq!(result.scriptlet_metadata.decision_counts.replaced, 1);
    assert_eq!(result.scriptlet_metadata.decision_counts.review, 0);
    assert_eq!(
        result.scriptlet_metadata.review_reason_codes,
        vec!["public-policy-file-capability-private-review".to_string()]
    );

    let file_capabilities = &result.build_result.manifest.file_capabilities;
    assert_eq!(file_capabilities.len(), 1);
    assert_eq!(file_capabilities[0].path, "/usr/bin/test");
    assert_eq!(
        file_capabilities[0].capabilities,
        vec!["cap_sys_admin".to_string()]
    );

    let bundle = result
        .build_result
        .manifest
        .legacy_scriptlets
        .as_ref()
        .expect("conversion should embed passive scriptlet bundle");
    assert_eq!(bundle.entries.len(), 1);
    assert_eq!(bundle.entries[0].decision.as_str(), "replaced");
    assert_eq!(
        bundle.entries[0].reason_code,
        "helper-complete-file-capability"
    );
    assert_eq!(
        bundle.entries[0].effects[0].adapter_id.as_deref(),
        Some("file-capability/v1")
    );
}
```

- [ ] **Step 2: Strengthen the existing allowed-capability test**

In the existing `conversion_integration_projects_payload_backed_setcap_into_file_capability_authority` test, keep the existing assertions and add:

```rust
assert_eq!(result.scriptlet_metadata.scriptlet_fidelity, "fully-replaced");
assert_eq!(result.scriptlet_metadata.target_compatibility, "conary-portable");
assert_eq!(result.scriptlet_metadata.publication_status, "public");
assert!(result.scriptlet_metadata.review_reason_codes.is_empty());
assert_eq!(result.scriptlet_metadata.decision_counts.replaced, 1);
assert_eq!(result.scriptlet_metadata.decision_counts.review, 0);
```

- [ ] **Step 3: Run the failing conversion tests**

Run:

```bash
cargo test -p conary-core conversion_integration_projects_payload_backed_setcap_into_file_capability_authority
cargo test -p conary-core conversion_integration_keeps_high_risk_setcap_replaced_but_private_review
```

Expected: FAIL because high-risk file capabilities are still aggregate-public today.

- [ ] **Step 4: Apply policy during aggregate status and summary construction**

In `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`, import the helper:

```rust
use super::super::public_policy;
```

Add this helper near `sorted_entry_reason_codes`:

```rust
fn public_policy_review_reason_codes(bundle: &LegacyScriptletBundle) -> Vec<String> {
    bundle
        .entries
        .iter()
        .flat_map(public_policy::entry_public_policy_review_reasons)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
```

Change `summary_from_bundle(...)` so `review_reason_codes` merges entry review reasons and public-policy reasons:

```rust
let mut review_reason_codes = sorted_entry_reason_codes(bundle, "review");
review_reason_codes.extend(public_policy_review_reason_codes(bundle));
review_reason_codes.sort();
review_reason_codes.dedup();
```

Change the final public branch in `aggregate_status(...)`:

```rust
let public_policy_review_required = entries
    .iter()
    .any(|entry| !public_policy::entry_public_policy_review_reasons(entry).is_empty());
if public_policy_review_required {
    return (
        ScriptletFidelity::FullyReplaced,
        TargetCompatibility::ConaryPortable,
        PublicationPolicy::PrivateReview,
        PublicationStatus::PrivateReview,
    );
}
```

Place that block immediately before the final return expression that currently
returns `(ScriptletFidelity::FullyReplaced, TargetCompatibility::ConaryPortable,
PublicationPolicy::PublicIfNoBlocked, PublicationStatus::Public)`.

- [ ] **Step 5: Verify the conversion tests pass**

Run:

```bash
cargo test -p conary-core conversion_integration_projects_payload_backed_setcap_into_file_capability_authority
cargo test -p conary-core conversion_integration_keeps_high_risk_setcap_replaced_but_private_review
```

Expected: PASS.

- [ ] **Step 6: Commit Task 2**

Run:

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs crates/conary-core/src/ccs/convert/converter.rs
git commit -m "security: gate high risk file capabilities from public status"
```

## Task 3: Split Public And Private-Review Fixture Evidence

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/golden_fixtures.rs`
- Modify: `crates/conary-core/src/ccs/convert/support_matrix.rs`
- Modify: `crates/conary-core/src/ccs/convert/adapters.rs`

**Interfaces:**
- Produces: fixture id `adapter-file-capability-high-risk`
- Updates: `file-capability/v1` support-matrix fixture evidence to include both `adapter-file-capability` and `adapter-file-capability-high-risk`

- [ ] **Step 1: Add failing support-matrix expectations**

In `crates/conary-core/src/ccs/convert/support_matrix.rs`, add a focused test near the support-matrix tests:

```rust
#[test]
fn file_capability_support_matrix_distinguishes_public_and_private_review_fixtures() {
    let matrix = SupportMatrix::default();
    let row = matrix
        .entries()
        .iter()
        .find(|entry| entry.adapter_id == Some("file-capability/v1"))
        .expect("file-capability adapter row exists");

    assert_eq!(
        row.fixture_names,
        &["adapter-file-capability", "adapter-file-capability-high-risk"]
    );

    let fixtures: std::collections::BTreeMap<_, _> = golden_fixtures::all_cases()
        .iter()
        .map(|case| (case.id, case.expected_outcome))
        .collect();
    assert_eq!(
        fixtures.get("adapter-file-capability"),
        Some(&golden_fixtures::GoldenFixtureOutcome::FullyReplaced)
    );
    assert_eq!(
        fixtures.get("adapter-file-capability-high-risk"),
        Some(&golden_fixtures::GoldenFixtureOutcome::ReviewRequired)
    );
}
```

Update `public_ready_adapter_rows_have_golden_fixture_evidence` so known rows may include private-review fixture evidence, but still require at least one public-ready fixture:

```rust
let mut has_public_ready_fixture = false;
for fixture_name in entry.fixture_names {
    let fixture = fixtures.get(fixture_name).unwrap_or_else(|| {
        panic!("adapter {adapter_id} fixture {fixture_name} is not declared")
    });
    if fixture.expected_outcome == expected {
        has_public_ready_fixture = true;
        assert!(
            fixture.source_distro_id.is_some() && fixture.target_distro_id.is_some(),
            "public-ready fixture {fixture_name} must use exact source and target distro ids"
        );
    }
}
assert!(
    has_public_ready_fixture,
    "adapter {adapter_id} has no public-ready golden fixture evidence"
);
```

Do not require every known-row fixture to have `FullyReplaced` outcome.

- [ ] **Step 2: Add a failing adapter registry golden case for high-risk setcap**

In `crates/conary-core/src/ccs/convert/adapters.rs`, add this case to the `cases` array in `adapter_registry_golden_helpers_are_fully_replaced_with_adapter_evidence`:

```rust
GoldenAdapterCase {
    fixture_id: "adapter-file-capability-high-risk",
    command: "setcap",
    argv: &["cap_sys_admin=+ep", "/usr/bin/demo"],
    adapter_id: "file-capability/v1",
    reason_code: "helper-complete-file-capability",
},
```

- [ ] **Step 3: Run the failing fixture tests**

Run:

```bash
cargo test -p conary-core support_matrix
cargo test -p conary-core adapter_registry_golden_helpers_are_fully_replaced_with_adapter_evidence
```

Expected: FAIL because `adapter-file-capability-high-risk` is not declared yet and the support-matrix row has only the public fixture.

- [ ] **Step 4: Add the high-risk fixture metadata**

In `crates/conary-core/src/ccs/convert/golden_fixtures.rs`, add this case to `ALL_GOLDEN_FIXTURE_CASES` near `adapter-file-capability`:

```rust
fixture(
    "adapter-file-capability-high-risk",
    GoldenFixtureOutcome::ReviewRequired,
),
```

Do not add it to `REQUIRED_GOAL8_CASES` as `FullyReplaced`; it is known adapter evidence but not public-ready fixture evidence.

In `crates/conary-core/src/ccs/convert/support_matrix.rs`, change the `file-capability/v1` fixture names from:

```rust
&["adapter-file-capability"],
```

to:

```rust
&["adapter-file-capability", "adapter-file-capability-high-risk"],
```

- [ ] **Step 5: Verify fixture and support-matrix tests pass**

Run:

```bash
cargo test -p conary-core support_matrix
cargo test -p conary-core adapter_registry_golden_helpers_are_fully_replaced_with_adapter_evidence
```

Expected: PASS.

- [ ] **Step 6: Commit Task 3**

Run:

```bash
git add crates/conary-core/src/ccs/convert/golden_fixtures.rs crates/conary-core/src/ccs/convert/support_matrix.rs crates/conary-core/src/ccs/convert/adapters.rs
git commit -m "test: distinguish private review file capability fixture"
```

## Task 4: Version-Stale Cached Rows And Tighten Summary Shape

**Files:**
- Modify: `crates/conary-core/src/db/models/converted.rs`

**Interfaces:**
- Updates: `CONVERSION_VERSION = 5`
- Updates: `ConvertedPackage::is_scriptlet_public_ready()` returns false when `needs_reconversion()` is true
- Updates: non-default summary shape requires `security_policy_intents`

- [ ] **Step 1: Add failing converted-row tests**

In `crates/conary-core/src/db/models/converted.rs`, add these tests near the existing publication-shape tests:

```rust
#[test]
fn stale_converted_rows_are_not_scriptlet_public_ready() {
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "stale".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
        &["sha256:chunk".to_string()],
        42,
        "sha256:content".to_string(),
        "/tmp/stale.ccs".to_string(),
    );
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "fully-replaced".to_string(),
        target_compatibility: "conary-portable".to_string(),
        publication_status: "public".to_string(),
        evidence_digest: Some(crate::hash::sha256_prefixed(b"evidence")),
        decision_counts: crate::ccs::convert::ScriptletDecisionCountsSummary {
            replaced: 1,
            ..Default::default()
        },
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.conversion_version = CONVERSION_VERSION - 1;

    assert!(converted.needs_reconversion());
    assert!(!converted.is_scriptlet_public_ready());
}

#[test]
fn non_default_publication_summary_requires_security_policy_intents() {
    let mut converted = ConvertedPackage::new(
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
    );
    converted.scriptlet_fidelity = "fully-replaced".to_string();
    converted.target_compatibility = "conary-portable".to_string();
    converted.publication_status = "public".to_string();
    converted.evidence_digest = Some(crate::hash::sha256_prefixed(b"evidence"));
    converted.scriptlet_summary_json = serde_json::json!({
        "scriptlet_fidelity": "fully-replaced",
        "target_compatibility": "conary-portable",
        "publication_status": "public",
        "decision_counts": {
            "replaced": 1,
            "legacy": 0,
            "blocked": 0,
            "review": 0
        },
        "blocked_reason_codes": [],
        "review_reason_codes": [],
        "unknown_commands": [],
        "blocked_classes": [],
        "boot_security_intents": []
    })
    .to_string();

    assert!(!converted.scriptlet_summary_for_publication().valid);
    assert!(!converted.is_scriptlet_public_ready());
}

#[test]
fn non_default_publication_summary_accepts_security_policy_intents() {
    let mut converted = ConvertedPackage::new(
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
    );
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "fully-replaced".to_string(),
        target_compatibility: "conary-portable".to_string(),
        publication_status: "public".to_string(),
        evidence_digest: Some(crate::hash::sha256_prefixed(b"evidence")),
        decision_counts: crate::ccs::convert::ScriptletDecisionCountsSummary {
            replaced: 1,
            ..Default::default()
        },
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();

    assert!(converted.scriptlet_summary_for_publication().valid);
    assert!(converted.is_scriptlet_public_ready());
}
```

- [ ] **Step 2: Run the failing converted-row tests**

Run:

```bash
cargo test -p conary-core stale_converted_rows_are_not_scriptlet_public_ready
cargo test -p conary-core non_default_publication_summary_requires_security_policy_intents
cargo test -p conary-core non_default_publication_summary_accepts_security_policy_intents
```

Expected: FAIL because stale rows can still answer public-ready directly, and `security_policy_intents` is not required.

- [ ] **Step 3: Bump the conversion version**

Change:

```rust
pub const CONVERSION_VERSION: i32 = 4;
```

to:

```rust
pub const CONVERSION_VERSION: i32 = 5;
```

- [ ] **Step 4: Make direct public-ready checks fail closed for stale rows**

Change `ConvertedPackage::is_scriptlet_public_ready()` to:

```rust
pub fn is_scriptlet_public_ready(&self) -> bool {
    if self.needs_reconversion() {
        return false;
    }
    let publication = self.scriptlet_summary_for_publication();
    publication.valid && publication.summary.publication_status == "public"
}
```

Also apply the same stale guard to the private
`ChunkPublicationCandidate::is_scriptlet_public_ready()` helper if it can carry
stale conversion versions. If that helper cannot see `conversion_version`, add
this comment directly above `ChunkPublicationCandidate::is_scriptlet_public_ready()`:

```rust
// Stale rows are excluded by the chunk_publication_state SQL filter on conversion_version.
```

- [ ] **Step 5: Require `security_policy_intents` in non-default summary JSON**

In `ScriptletPublicationColumns::summary_json_shape_valid_for_publication(...)`, add `"security_policy_intents"` to the required key list immediately after `"boot_security_intents"`.

Keep `is_default_scriptlet_publication_shape(...)` unchanged so constructor defaults remain compatible only when scalar columns also show unknown/public/no evidence.

- [ ] **Step 6: Verify the converted-row tests pass**

Run:

```bash
cargo test -p conary-core stale_converted_rows_are_not_scriptlet_public_ready
cargo test -p conary-core non_default_publication_summary_requires_security_policy_intents
cargo test -p conary-core non_default_publication_summary_accepts_security_policy_intents
```

Expected: PASS.

- [ ] **Step 7: Run focused publication regression tests**

Run:

```bash
cargo test -p conary-core converted_package
cargo test -p remi publication
cargo test -p remi scriptlet_publication
```

Expected: PASS or, for `scriptlet_publication`, no tests matched if that filter is absent. Record the exact result in the task report.

- [ ] **Step 8: Commit Task 4**

Run:

```bash
git add crates/conary-core/src/db/models/converted.rs
git commit -m "security: stale scriptlet conversions before public serving"
```

## Task 5: Update Docs, Audit Metadata, And Focused Gates

**Files:**
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

**Interfaces:**
- Documents: known Linux file-capability syntax versus public-ready file-capability policy.
- Documents: high-risk known capabilities are private-review even when replacement is complete.
- Documents: fixture split between `adapter-file-capability` and `adapter-file-capability-high-risk`.

- [ ] **Step 1: Update scriptlet security docs**

In `docs/SCRIPTLET_SECURITY.md`, update the file-capability section so it says:

```markdown
`file-capability/v1` still recognizes known Linux `setcap cap_*=+ep
<payload-executable>` grants as complete replacement evidence, but only
`cap_net_bind_service` is public-ready by default. High-risk known capabilities
such as `cap_sys_admin`, `cap_sys_module`, `cap_sys_rawio`, `cap_sys_boot`,
`cap_sys_ptrace`, `cap_bpf`, `cap_net_admin`, `cap_setpcap`, and
`cap_setfcap` remain private-review until a future target-profile policy
explicitly allows them.
```

- [ ] **Step 2: Update CCS docs**

In `docs/modules/ccs.md`, update the file-capability paragraph so it separates:

```markdown
The manifest still validates `[[file_capabilities]]` against the known Linux
capability table. Public-ready conversion is narrower: the first public
allowlist is `cap_net_bind_service`; other known capability names remain valid
native manifest authority but non-public conversion evidence.
```

- [ ] **Step 3: Update Remi docs**

In `docs/modules/remi.md`, update the public gate section so it says:

```markdown
Fully replaced scriptlets are public only when their projected authority also
passes public policy. For `file-capability/v1`, `cap_net_bind_service` may be
public-ready; high-risk known capabilities produce valid replacement evidence
and valid CCS manifest authority but Remi treats the converted row as
private-review.
```

- [ ] **Step 4: Update fixture docs**

In `docs/modules/test-fixtures.md`, add or update the file-capability fixture notes:

```markdown
- `adapter-file-capability`: allowlisted `cap_net_bind_service` replacement
  evidence, expected public-ready fully replaced outcome.
- `adapter-file-capability-high-risk`: known high-risk capability replacement
  evidence, expected private-review outcome while preserving adapter evidence.
```

- [ ] **Step 5: Update docs-audit ledger and inventory**

Add this plan to `docs/superpowers/documentation-accuracy-audit-ledger.tsv` with:

```text
docs/superpowers/plans/2026-07-08-file-capability-public-policy-plan.md	docs/superpowers/plans/2026-07-08-file-capability-public-policy-plan.md	planning	maintainer	scriptlet-security; file-capabilities; remi-publication-gate; implementation-plan	docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md; docs/SCRIPTLET_SECURITY.md; docs/modules/ccs.md; docs/modules/remi.md; docs/modules/test-fixtures.md; crates/conary-core/src/ccs/convert/public_policy.rs; crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs; crates/conary-core/src/ccs/convert/converter.rs; crates/conary-core/src/ccs/convert/golden_fixtures.rs; crates/conary-core/src/ccs/convert/support_matrix.rs; crates/conary-core/src/db/models/converted.rs	verified	pending	Implementation plan for the first scriptlet public-authority slice, tightening file-capability public policy to cap_net_bind_service while preserving complete adapter replacement evidence for known high-risk capabilities as private-review and version-staling cached converted rows.
```

After staging the new plan and ledger row, regenerate or patch `docs/superpowers/documentation-accuracy-audit-inventory.tsv` so it includes:

```text
docs/superpowers/plans/2026-07-08-file-capability-public-policy-plan.md	planning	maintainer
```

- [ ] **Step 6: Run focused implementation gates**

Run:

```bash
cargo test -p conary-core public_policy
cargo test -p conary-core conversion_integration_projects_payload_backed_setcap_into_file_capability_authority
cargo test -p conary-core conversion_integration_keeps_high_risk_setcap_replaced_but_private_review
cargo test -p conary-core support_matrix
cargo test -p conary-core adapter_registry_golden_helpers_are_fully_replaced_with_adapter_evidence
cargo test -p conary-core converted_package
cargo test -p remi publication
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: PASS. If a filter matches zero tests, record the exact zero-match output and run the owning broader focused command from the owner card.

- [ ] **Step 7: Run docs and formatting gates**

Run:

```bash
cargo fmt --check
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.

- [ ] **Step 8: Commit Task 5**

Run:

```bash
git add docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: document file capability public policy"
```

## Final Slice Review

After Task 5, run a whole-slice review package from the branch base to `HEAD`, dispatch a final reviewer, and fix Critical or Important findings before considering this slice complete.

The final slice closeout must report:

- commits created for Tasks 1-5;
- all focused implementation and docs commands from Task 5;
- any zero-match test filters and the broader command used instead;
- final reviewer verdict and any fixed findings;
- whether any QEMU generation gates were skipped because this slice does not change generation artifacts.
- whether end-to-end `ReviewRequired` golden conversion fixture coverage was
  added here or intentionally deferred to the next Remi/publication alignment
  slice.
