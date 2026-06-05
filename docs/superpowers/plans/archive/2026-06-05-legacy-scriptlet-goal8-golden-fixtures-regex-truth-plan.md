# Legacy Scriptlet Goal 8 Golden Fixtures And Regex Truth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete Goal 8a and Goal 8b as one coordinated evidence packet: expand legacy scriptlet golden behavior coverage across supported lifecycle surfaces, then retire the old regex scriptlet analyzer as a source of conversion authority once adapter registry and support-matrix evidence is in place.

**Architecture:** `conary-core` remains the source of conversion classification truth. CLI integration tests exercise package conversion and replay/refusal boundaries. Remi persists and publishes only the safe conversion outcomes produced by core. Documentation describes only evidence-backed behavior for Fedora 44, Ubuntu 26.04, and Arch.

**Tech Stack:** Rust workspace, `conary-core` conversion modules, `apps/conary` integration tests, Remi in-crate server tests, `conary-test` inventory checks, repo doc-truth scripts.

---

## Combined Design

Goal 8 has two small but tightly related slices:

- **Goal 8a, Lifecycle Expansion And Golden Behavior Fixtures:** build a golden corpus that proves conversion behavior for known-safe scriptlet classes, known-review classes, unsupported lifecycle surfaces, and package-manager recursion failures.
- **Goal 8b, Regex Authority Retirement And Docs Truth:** ensure regex-derived scriptlet matches are advisory evidence only, while adapter classification, blocked-class classification, support-matrix coverage, and replay-policy checks provide the actual authority for conversion and publication decisions.

Combining the design and plan avoids duplicate context gathering. Implementation should still proceed in separate commits so each slice has a small verification surface and can be merged cleanly before the next one starts.

## Review Gate

This packet is intended for external review before implementation. Resolve review findings and lock in the final plan before launching Goal 8a Task 1.

## Known Preflight Blocker

As of this packet, `bash scripts/check-doc-truth.sh` fails on unrelated schema-version drift: several docs still mention schema 69 while the repository schema version is 71. Do not mask that failure. Before treating Goal 8b Task 3 or final verification as complete, either resolve that drift in a narrow preflight cleanup or confirm it has already been fixed on `main`.

## Scope

- Add a golden corpus for the legacy scriptlet behavior rows already called out by the legacy scriptlet semantics bundle design:
  - no scriptlets
  - user/group creation
  - systemd enable and daemon reload
  - tmpfiles/cache refresh
  - alternatives registration
  - residual unknown shell requiring legacy replay
  - blocked package-manager recursion
  - foreign legacy replay rejection
  - RPM trigger or file-trigger quarantine
  - DEB trigger quarantine
  - Arch `.INSTALL` wrapper replay or review boundary
- Prove that every fixture name referenced by `crates/conary-core/src/ccs/convert/support_matrix.rs` has test coverage or an explicit review/blocked outcome.
- Keep the conversion planner free of host I/O. Host target discovery remains outside core.
- Keep public conversion outcomes default-deny unless the adapter registry and support matrix prove them safe.
- Make Remi tests assert public-ready behavior from persisted conversion records rather than re-classifying scriptlets in the server.
- Update public docs only after the behavior and publication gates are covered.

## Non-Goals

- No new source-target support beyond Fedora 44, Ubuntu 26.04, and Arch.
- No CLI host target resolution work.
- No expansion of legacy replay to foreign targets.
- No hidden host probing in `conary-core`.
- No broad package execution harness for every distro image in this slice.
- No public claim that all scriptlets are portable or automatically convertible.
- No serving partial conversions as ordinary ready-to-install CCS packages.

## Conversion Authority Model

The intended authority chain after Goal 8 is:

1. Parse package metadata and scriptlet bodies into structured conversion inputs.
2. Classify scriptlets with the adapter registry and blocked-class registry.
3. Look up target compatibility and support-matrix evidence.
4. Decide one of the stable outcomes:
   - `native-free`
   - `fully-replaced`
   - `legacy-replay`
   - `review-required`
   - `blocked`
5. Persist the decision, evidence, fidelity, and review reason identifiers.
6. Let Remi publish only outcomes that are public-ready for the requested surface.

The regex analyzer may remain as an advisory detector for discovered hook names, debug evidence, or fidelity hints. It must not be able to mark scriptlets as `replaced`, `fully-replaced`, `native-free`, or public-ready by itself.

## Golden Corpus Rules

Golden cases should be small and deterministic. Prefer direct metadata and scriptlet fixtures over full package execution unless a task explicitly needs CLI package conversion.

Each case should name:

- fixture id
- source package format
- exact source distro id when the case is public-ready
- exact target distro id when the case is public-ready
- lifecycle phase
- scriptlet body or absence of scriptlet body
- expected conversion outcome
- expected fidelity
- expected public-ready status
- expected stable reason id when the outcome is `review-required` or `blocked`

Golden tests should fail when a support-matrix row is added without evidence or when a public-ready outcome is produced without adapter/support-matrix backing.

Public-ready golden cases must use the current supported user distro IDs: `fedora-44`, `ubuntu-26.04`, or `arch`. Family-level values such as `rpm`, `deb`, `fedora`, or `ubuntu` are classification inputs, not sufficient public-ready source or target identifiers.

## Execution Model

Implement Goal 8 serially. The tasks intentionally share `crates/conary-core/src/ccs/convert/mod.rs`, `converter.rs`, `analyzer.rs`, and support-matrix fixtures, so parallel implementation would create avoidable merge conflicts and could split the authority-chain checks across incompatible edits.

## Implementation Plan

### Goal 8a Task 1: Add Support-Matrix Fixture Coverage Tests

Purpose: lock the support matrix to explicit fixture evidence before broadening lifecycle coverage.

Files likely touched:

- `crates/conary-core/src/ccs/convert/support_matrix.rs`
- `crates/conary-core/src/ccs/convert/mod.rs`
- `crates/conary-core/src/ccs/convert/golden_fixtures.rs`

Steps:

- [ ] Add failing unit tests proving every fixture name referenced by `SupportMatrix` exists in a declared golden fixture catalog.
- [ ] Add failing unit tests proving every required Goal 8 corpus row has a fixture id and expected outcome.
- [ ] Add the minimal golden fixture catalog needed to make those tests pass.
- [ ] Declare `mod golden_fixtures;` in `crates/conary-core/src/ccs/convert/mod.rs`.
- [ ] Keep the catalog data-only: no host probing, no package-manager calls, no file-system assumptions outside static fixture text.
- [ ] Keep `golden_fixtures` private unless a later CLI or Remi task proves a public API is needed.

Expected test shape:

```rust
#[test]
fn support_matrix_fixture_names_have_declared_golden_cases() {
    let fixtures = golden_fixtures::declared_fixture_ids();
    for entry in SupportMatrix::default().entries() {
        for fixture_name in entry.fixture_names {
            assert!(
                fixtures.contains(fixture_name),
                "support-matrix fixture {fixture_name} has no golden case"
            );
        }
    }
}

#[test]
fn goal8_required_corpus_rows_are_declared() {
    let fixtures = golden_fixtures::required_goal8_cases();
    assert!(fixtures.contains("adapter-registry-native-free"));
    assert!(fixtures.contains("blocked-class-package-manager-recursion"));
    assert!(fixtures.contains("review-class-deb-trigger"));
}
```

Verification:

- `cargo test -p conary-core support_matrix`
- `cargo fmt --check`
- `git diff --check`

Suggested commit:

- `test(core): cover legacy scriptlet support matrix fixtures`

### Goal 8a Task 2: Add Core Golden Classification Corpus

Purpose: prove adapter-supported, review-required, and blocked scriptlet outcomes in core before CLI or Remi tests depend on them.

Files likely touched:

- `crates/conary-core/src/ccs/convert/adapters.rs`
- `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`
- `crates/conary-core/src/ccs/convert/golden_fixtures.rs`
- `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`

Steps:

- [ ] Add failing tests for no-scriptlet conversion producing `native-free` and public-ready metadata.
- [ ] Add failing tests for sysusers, systemd daemon reload, systemd unit state, tmpfiles, cache refresh, and alternatives fixtures producing adapter-backed `fully-replaced` outcomes.
- [ ] Before any unknown shell fixture can become `legacy-replay`, add replay aggregation tests proving `decision_counts.legacy > 0`, `scriptlet_fidelity = "legacy-replay"`, source/native target compatibility is set, and `publication_status != "public"` unless a later public replay lane is deliberately added.
- [ ] Add failing tests for unknown shell fixtures producing `legacy-replay` only when same-target replay policy allows it.
- [ ] Add failing tests for package-manager recursion producing `blocked` with a stable reason id.
- [ ] Add failing tests for RPM triggers, DEB triggers, and Arch install wrapper boundaries producing `review-required` or `blocked` according to the existing blocked-class registry.
- [ ] Add failing tests proving public-ready golden cases cannot use unsupported or family-only distro identifiers.
- [ ] Implement only the missing classification glue required by those tests.
- [ ] Prefer `build_legacy_scriptlet_bundle` and classification helper tests for core golden assertions; use full `LegacyConverter::convert` only where archive-generation behavior is intentionally under test.
- [ ] Do not execute fixture scripts.

Expected assertions:

```rust
assert_eq!(
    case.expected_fidelity,
    result.scriptlet_metadata.scriptlet_fidelity
);
assert_eq!(
    case.expected_publication_status,
    result.scriptlet_metadata.publication_status
);

let bundle = result
    .legacy_scriptlets
    .as_ref()
    .expect("golden scriptlet case should embed a passive bundle");
if let Some(expected_decision) = case.expected_decision {
    assert!(
        bundle
            .entries
            .iter()
            .any(|entry| entry.decision == expected_decision)
    );
} else {
    assert!(bundle.entries.is_empty());
    assert_eq!(bundle.decision_counts.total(), 0);
}

if let Some(reason_id) = case.expected_reason_id {
    assert!(
        result
            .scriptlet_metadata
            .blocked_reason_codes
            .iter()
            .chain(result.scriptlet_metadata.review_reason_codes.iter())
            .any(|code| code == reason_id)
    );
}

let has_adapter_evidence = result.scriptlet_classification.entries.iter().any(|entry| {
    matches!(
        &entry.classification,
        ScriptletClassification::Known { effects, .. }
            if effects.iter().any(|effect| effect.adapter_id.is_some())
    )
});
assert_eq!(case.expect_adapter_evidence, has_adapter_evidence);
```

Verification:

- `cargo test -p conary-core adapters`
- `cargo test -p conary-core blocked_classes`
- `cargo test -p conary-core scriptlet_bundle`
- `cargo fmt --check`
- `git diff --check`

Suggested commit:

- `test(core): add legacy scriptlet golden classification corpus`

### Goal 8a Task 3: Add CLI Conversion Golden Tests

Purpose: prove the same golden outcomes through the CLI conversion boundary without adding host I/O to core.

Files likely touched:

- `apps/conary/tests/conversion_integration.rs`
- `apps/conary/tests/common/mod.rs`
- `apps/conary/tests/common/legacy_scriptlet_fixtures.rs`
- `apps/conary/tests/fixtures/`

Steps:

- [ ] Add integration fixtures for the required Goal 8 corpus rows that need CLI-level coverage.
- [ ] Add conversion tests that compare converted metadata against the golden outcome table.
- [ ] Prove adapter-supported cases produce CCS metadata with replacement evidence and no unsafe replay requirement.
- [ ] Prove unknown same-target replay cases remain marked as replay-required rather than silently public-ready.
- [ ] Prove foreign replay requests are rejected before mutation with a stable reason id.
- [ ] Keep CLI host target resolution scoped to existing CLI code paths.

Expected assertion pattern:

```rust
let bundle = parsed
    .manifest()
    .legacy_scriptlets
    .as_ref()
    .expect("converted package should carry scriptlet bundle");
assert_eq!(bundle.scriptlet_fidelity.as_str(), "fully-replaced");
assert!(bundle.entries.iter().any(|entry| {
    entry
        .effects
        .iter()
        .any(|effect| effect.adapter_id.is_some())
}));
assert!(!bundle
    .entries
    .iter()
    .any(|entry| entry.decision == ScriptletDecision::Legacy));
```

```rust
let error = conversion_result
    .expect_err("foreign replay should be rejected before mutation")
    .to_string();
assert!(error.contains("legacy-replay-foreign-target"));
```

Verification:

- `cargo test -p conary --test conversion_integration`
- `cargo run -p conary-test -- list`
- `cargo fmt --check`
- `git diff --check`

Suggested commit:

- `test(cli): add legacy scriptlet golden conversion coverage`

### Goal 8a Task 4: Add Lifecycle And Trigger Refusal Coverage

Purpose: make update/remove/trigger boundaries explicit so unsupported lifecycle scriptlets fail safely.

Files likely touched:

- `apps/conary/tests/bundle_replay.rs`
- `apps/conary/tests/foreign_replay.rs`
- `apps/conary/tests/conversion_integration.rs`
- `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- `crates/conary-core/src/scriptlet/`

Steps:

- [ ] Add failing tests for unsupported install/update/remove lifecycle variants that should require review or block.
- [ ] Add failing tests for RPM trigger and file-trigger quarantine.
- [ ] Add failing tests for DEB trigger quarantine.
- [ ] Add failing tests for Arch `.INSTALL` wrapper replay boundaries.
- [ ] Ensure each refusal happens before scriptlet execution or package mutation.
- [ ] Preserve current same-target replay behavior only for explicitly allowed residual shell cases.

Verification:

- `cargo test -p conary --test bundle_replay`
- `cargo test -p conary --test foreign_replay`
- `cargo test -p conary --test conversion_integration`
- `cargo fmt --check`
- `git diff --check`

Suggested commit:

- `test(cli): cover legacy scriptlet lifecycle refusals`

### Goal 8a Task 5: Add Remi Publication Golden Tests

Purpose: prove Remi persists and publishes scriptlet conversion outcomes without broadening unsafe public surfaces.

Files likely touched:

- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/publication.rs`
- `apps/remi/src/server/prewarm.rs`

Steps:

- [ ] Add Remi tests that persist representative `native-free`, `fully-replaced`, `legacy-replay`, `review-required`, and `blocked` conversion records.
- [ ] Prove public listing or download paths include only public-ready outcomes.
- [ ] Prove review-required and blocked records retain review evidence but do not expose local script paths or unsafe replay bundles.
- [ ] Prove Remi does not re-run regex classification as an independent source of publication truth.
- [ ] Keep tests in Remi source modules, matching the existing in-crate conversion test pattern.

Verification:

- `cargo test -p remi conversion`
- `cargo test -p remi publication`
- `cargo fmt --check`
- `git diff --check`

Suggested commit:

- `test(remi): cover scriptlet publication golden outcomes`

### Goal 8b Task 1: Freeze Regex Analyzer To Advisory-Only

Purpose: remove the old regex analyzer from the authority path while preserving useful detected-hook evidence.

Files likely touched:

- `crates/conary-core/src/ccs/convert/analyzer.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`
- `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- `crates/conary-core/src/ccs/convert/adapters.rs`

Steps:

- [ ] Add failing tests where regex-detected hooks are present but no adapter/support-matrix evidence exists.
- [ ] Assert those cases cannot produce `native-free`, `fully-replaced`, `replaced`, or public-ready conversion results from regex evidence alone.
- [ ] Assert regex-only detections do not populate executable `build_result.manifest.hooks`; generated hooks must come from adapter-backed or curated evidence.
- [ ] Preserve detected hook names as advisory metadata when they help review diagnostics.
- [ ] Route conversion fidelity and public-ready decisions through adapter classification, blocked-class classification, support-matrix evidence, and replay policy.
- [ ] Rename or narrow any helper whose name implies regex authority if the implementation can do so without churn.

Expected assertion:

```rust
assert!(
    result
        .detected_hooks
        .systemd
        .iter()
        .any(|hook| hook.unit == "demo.service")
);
assert_ne!(
    result.scriptlet_metadata.scriptlet_fidelity,
    ScriptletFidelity::FullyReplaced.as_str()
);
assert_ne!(result.scriptlet_metadata.publication_status, "public");
assert!(result.build_result.manifest.hooks.users.is_empty());
assert!(result.build_result.manifest.hooks.groups.is_empty());
assert!(result.build_result.manifest.hooks.services.is_empty());
assert!(result.build_result.manifest.hooks.systemd.is_empty());
assert!(!result.scriptlet_classification.entries.iter().any(|entry| {
    matches!(
        &entry.classification,
        ScriptletClassification::Known { effects, .. }
            if effects.iter().any(|effect| effect.adapter_id.is_some())
    )
}));
```

Verification:

- `cargo test -p conary-core analyzer`
- `cargo test -p conary-core converter`
- `cargo test -p conary-core scriptlet_bundle`
- `cargo fmt --check`
- `git diff --check`

Suggested commit:

- `refactor(core): keep regex scriptlet analysis advisory`

### Goal 8b Task 2: Add Adapter Parity Evidence Gate

Purpose: make adapter parity measurable before docs say the old regex authority is retired.

Files likely touched:

- `crates/conary-core/src/ccs/convert/support_matrix.rs`
- `crates/conary-core/src/ccs/convert/golden_fixtures.rs`
- `crates/conary-core/src/ccs/convert/adapters.rs`
- `crates/conary-core/src/ccs/convert/blocked_classes.rs`

Steps:

- [ ] Add a test that every adapter with a public-ready outcome has at least one golden fixture.
- [ ] Add a test that every review-required or blocked support-matrix row has a stable reason id.
- [ ] Add a test that every public-ready fixture references adapter evidence or explicit native-free evidence.
- [ ] Ensure the parity gate fails when new adapter classes are added without golden coverage.
- [ ] Keep the evidence in Rust data/tests, not in prose-only documentation.

Verification:

- `cargo test -p conary-core support_matrix`
- `cargo test -p conary-core golden_fixtures`
- `cargo fmt --check`
- `git diff --check`

Suggested commit:

- `test(core): gate scriptlet adapter parity with golden evidence`

### Goal 8b Task 3: Update Docs Truth

Purpose: align public documentation with evidence-backed scriptlet behavior.

Files likely touched:

- `docs/modules/ccs.md`
- `docs/modules/remi.md`
- `docs/modules/source-selection.md`
- `docs/conaryopedia-v2.md`
- `README.md`

Steps:

- [ ] Run `bash scripts/check-doc-truth.sh` before editing and resolve any still-present schema-version drift as a separate preflight cleanup if it is unrelated to scriptlet fidelity claims.
- [ ] Update CCS docs to describe the current authority chain: adapter registry, blocked classes, support matrix, replay policy, and target compatibility.
- [ ] Update Remi docs to state that public publication is allowed only for public-ready outcomes.
- [ ] Update Remi sparse-index docs so `converted=true` means a public-ready converted artifact, not merely a completed conversion row.
- [ ] Review source-selection foreign replay wording and keep it limited to existing explicit-policy behavior; do not broaden foreign replay support.
- [ ] Update public docs to state current supported source targets: Fedora 44, Ubuntu 26.04, and Arch.
- [ ] Remove stale language implying regex-derived scriptlet matches are authoritative.
- [ ] Remove stale language implying all legacy scriptlets can be converted or replayed across targets.
- [ ] Run doc truth checks and targeted stale-claim searches.

Verification:

- `bash scripts/check-doc-truth.sh`
- `rg -n "regex.*author|all legacy scriptlets|cross-target replay|foreign replay|ready for immediate download|converted field|converted = true|job status.*ready|Fedora 44|Ubuntu 26\\.04|\\bArch\\b" README.md docs/modules docs/conaryopedia-v2.md`
- `cargo fmt --check`
- `git diff --check`

Suggested commit:

- `docs: align scriptlet fidelity claims with evidence`

### Goal 8 Final Verification

Purpose: prove Goal 8a and Goal 8b are complete as one reviewed packet after the individual implementation commits are merged.

Run:

- `cargo test -p conary-core support_matrix`
- `cargo test -p conary-core adapters`
- `cargo test -p conary-core blocked_classes`
- `cargo test -p conary-core analyzer`
- `cargo test -p conary-core converter`
- `cargo test -p conary-core scriptlet_bundle`
- `cargo test -p conary --test conversion_integration`
- `cargo test -p conary --test bundle_replay`
- `cargo test -p conary --test foreign_replay`
- `cargo test -p remi conversion`
- `cargo test -p remi publication`
- `cargo run -p conary-test -- list`
- `bash scripts/check-doc-truth.sh`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --check`
- `git diff --check`

The doc-truth command is a hard final gate only after the known schema-version drift has been resolved or confirmed absent on the current `main`.

Completion evidence to report:

- commit SHAs for each Goal 8a/8b implementation slice
- verification commands run
- final `git status --short --branch`
- whether any files outside the planned scope changed

## Review Checklist

Before implementation starts, review this packet for:

- stale paths
- unsupported source targets
- accidental Remi out-of-crate test references
- regex analyzer authority leaks
- public-ready claims without adapter/support-matrix evidence
- doc claims broader than Fedora 44, Ubuntu 26.04, and Arch
- any implementation task that would require host I/O in `conary-core`

## Docs-Only Lock-In Verification

Before committing this plan packet, run:

- `git diff --check`
- `rg -n "T[B]D|T[O]DO|fill[ ]in|place[ ]holder|app[s]/remi/tests|Cen[t]OS|cen[t]os|RH[E]L|red[ ]hat" docs/superpowers/plans/2026-06-05-legacy-scriptlet-goal8-golden-fixtures-regex-truth-plan.md docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `rg -n "ready for immediate download|converted field|converted = true|job status.*ready|all legacy scriptlets|cross-target replay|foreign replay" README.md docs/modules docs/conaryopedia-v2.md`
- `bash scripts/check-doc-truth.sh`
- `git status --short --branch`

The public-doc claim search may surface current Goal 8b docs-truth targets. Review and record those matches; do not treat them as plan lock-in blockers unless the plan packet itself overclaims or omits the file that must be checked.

If `bash scripts/check-doc-truth.sh` still fails only on schema 69 versus schema 71 drift, record that as the known external blocker instead of masking it.
