# LSM Policy Semantics Lock Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lock the current SELinux/AppArmor public-authority boundary so future LSM expansion cannot accidentally promote provider-mode, status, disable, broad reload, or unbacked policy mutations.

**Architecture:** Keep the existing narrow `selinux-policy/v1` and `apparmor-policy/v1` adapters unchanged. Add explicit adapter-boundary, review-intent, and converter-level regressions proving unsupported LSM forms remain blocked/private while still preserving sanitized review metadata. Refresh docs to state that broader LSM public readiness requires explicit provider facts, profile/module content semantics, and operator-visible fallback behavior.

**Tech Stack:** Rust unit tests in `conary-core`, passive conversion tests, docs-audit checks.

## Global Constraints

- No new SELinux or AppArmor public-ready command forms in this slice.
- `apparmor-policy/v1` remains limited to one payload-backed `apparmor_parser -r|--replace /etc/apparmor.d/usr.bin.demo`-style profile reload.
- AppArmor mode changes, disable/status helpers, directory reloads, multi-profile reloads, nested profile paths, and non-payload profile paths remain blocked/private.
- `selinux-policy/v1` remains limited to payload-scoped label refresh, payload-backed file-context rules, persistent boolean declarations, and payload-backed policy-module installs.
- SELinux broad roots, policy-store removal, non-persistent boolean changes, unsupported `semanage` operations, and non-payload module paths remain blocked/private.
- Blocked AppArmor helper evidence may project generic `SecurityPolicyIntent` review metadata with `block-on-enforcing-target` fallback, but it must not become complete adapter replacement evidence.
- Conversion must never run SELinux or AppArmor tools against the host policy store.
- Documentation changes must keep `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, `docs/modules/remi.md`, docs-audit ledger, and inventory aligned.

---

## File Structure

- Modify `crates/conary-core/src/ccs/convert/adapters.rs`
  - Extend existing SELinux/AppArmor blocked-form adapter boundary tests.
- Modify `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`
  - Add explicit AppArmor mode/disable/status review-intent operation tests.
- Modify `crates/conary-core/src/ccs/convert/converter.rs`
  - Add a passive conversion regression proving an unsupported AppArmor mode helper remains blocked while preserving review intent.
- Modify `docs/SCRIPTLET_SECURITY.md`
  - State that broader SELinux/AppArmor public-ready expansion requires target provider facts and content semantics.
- Modify `docs/modules/ccs.md`
  - Mirror the LSM public-authority boundary in the CCS module.
- Modify `docs/modules/remi.md`
  - Clarify that Remi public serving does not promote broader LSM helpers.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Keep this plan row and touched LSM claim rows aligned with final behavior.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Regenerate after staging this plan and doc changes.

## Task 1: Pin LSM Adapter And Review-Intent Boundaries

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/adapters.rs`
- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`

**Interfaces:**
- Consumes: existing `AdapterRegistry::classify_invocation_with_context` behavior
- Consumes: existing `security_policy_intent_from_classification` behavior
- Produces: expanded blocked-form cases in existing SELinux/AppArmor adapter tests
- Produces: `blocked_apparmor_lifecycle_helpers_project_review_policy_operations`

- [ ] **Step 1: Extend SELinux blocked-form cases**

In `crates/conary-core/src/ccs/convert/adapters.rs`, extend
`selinux_adapter_leaves_broad_or_unbacked_mutation_blocked` by adding these
cases to its `for (command, argv)` table:

```rust
("restorecon", vec!["-Rv", "/usr"]),
("semodule", vec!["-r", "demo"]),
("setsebool", vec!["demo_can_network", "on"]),
```

The existing assertion must continue to prove each case is
`blocked-class-selinux` with class id `selinux`.

- [ ] **Step 2: Extend AppArmor blocked-form cases**

In `apparmor_adapter_leaves_broad_or_unbacked_profile_mutation_blocked`, add
these cases to the table:

```rust
(
    "apparmor_parser",
    vec![
        "--replace",
        "/etc/apparmor.d/usr.bin.demo",
        "/etc/apparmor.d/usr.bin.other",
    ],
),
(
    "apparmor_parser",
    vec!["--replace", "/etc/apparmor.d/subdir/usr.bin.demo"],
),
("aa-complain", vec!["/etc/apparmor.d/usr.bin.demo"]),
("aa-disable", vec!["/etc/apparmor.d/usr.bin.demo"]),
```

The existing assertion must continue to prove each case is
`blocked-class-apparmor` with class id `apparmor`.

- [ ] **Step 3: Add AppArmor review-intent operation coverage**

In `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`,
add this test after `blocked_apparmor_command_projects_review_policy_intent`:

```rust
#[test]
fn blocked_apparmor_lifecycle_helpers_project_review_policy_operations() {
    for (command, argv, operation, expected_name, paths) in [
        (
            "aa-enforce",
            vec!["/etc/apparmor.d/usr.bin.demo"],
            "mode-enforce",
            Some("/etc/apparmor.d/usr.bin.demo".to_string()),
            vec!["/etc/apparmor.d/usr.bin.demo".to_string()],
        ),
        (
            "aa-complain",
            vec!["/etc/apparmor.d/usr.bin.demo"],
            "mode-complain",
            Some("/etc/apparmor.d/usr.bin.demo".to_string()),
            vec!["/etc/apparmor.d/usr.bin.demo".to_string()],
        ),
        (
            "aa-disable",
            vec!["/etc/apparmor.d/usr.bin.demo"],
            "profile-disable",
            Some("/etc/apparmor.d/usr.bin.demo".to_string()),
            vec!["/etc/apparmor.d/usr.bin.demo".to_string()],
        ),
        ("aa-status", Vec::new(), "status-query", None, Vec::new()),
    ] {
        let classification = ScriptletClassification::Blocked {
            reason_code: "blocked-class-apparmor".to_string(),
            class_id: "apparmor".to_string(),
            command: Some(ScriptletCommandEvidence {
                command: command.to_string(),
                argv: argv.into_iter().map(str::to_string).collect(),
                phase: Some("post-install".to_string()),
                lifecycle_paths: vec!["post-install".to_string()],
                raw_line: None,
                source: "static-signal".to_string(),
                environment: Vec::new(),
            }),
        };

        let intent = security_policy_intent_from_classification(
            "scriptlet:0:post-install",
            &classification,
        )
        .expect("apparmor intent");

        assert_eq!(intent.provider.as_str(), "apparmor");
        assert_eq!(intent.operation, operation);
        assert_eq!(intent.scope.kind, "profile");
        assert_eq!(intent.scope.name, expected_name);
        assert_eq!(intent.scope.paths, paths);
        assert_eq!(intent.fallback.as_str(), "block-on-enforcing-target");
        assert_eq!(intent.reconciliation.state.as_str(), "review");
        assert_eq!(
            intent.reconciliation.reason.as_deref(),
            Some("blocked-class-apparmor")
        );
        assert!(!intent.payload_evidence.payload_backed);
        assert_eq!(intent.source.command.as_deref(), Some(command));
    }
}
```

- [ ] **Step 4: Run focused core tests**

Run:

```bash
cargo test -p conary-core apparmor --lib
cargo test -p conary-core selinux --lib
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/conary-core/src/ccs/convert/adapters.rs crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs
git commit -m "test: pin LSM adapter review boundaries"
```

## Task 2: Pin Unsupported AppArmor Conversion Outcome

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/converter.rs`

**Interfaces:**
- Consumes: passive test converter helpers in `converter.rs`
- Produces: `apparmor_mode_helper_remains_blocked_with_review_policy_intent`

- [ ] **Step 1: Add the converter regression**

In `crates/conary-core/src/ccs/convert/converter.rs`, add this test after
`apparmor_profile_reload_records_public_adapter_backed_policy_intent`:

```rust
#[test]
fn apparmor_mode_helper_remains_blocked_with_review_policy_intent() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "aa-enforce /etc/apparmor.d/usr.bin.demo\n".to_string(),
        flags: None,
    }];
    let mut files = make_test_files();
    files.push(ExtractedFile {
        path: "/etc/apparmor.d/usr.bin.demo".to_string(),
        content: b"profile usr.bin.demo /usr/bin/demo { }\n".to_vec(),
        size: 38,
        mode: 0o644,
        sha256: None,
        symlink_target: None,
    });
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "deb", "sha256:test")
        .expect("conversion succeeds");
    let package =
        crate::ccs::CcsPackage::parse(result.package_path.as_ref().unwrap().to_str().unwrap())
            .expect("converted CCS package should parse");
    let bundle = package
        .manifest()
        .legacy_scriptlets
        .as_ref()
        .expect("written CCS archive should carry passive scriptlet bundle");

    assert_eq!(bundle.publication_status.as_str(), "blocked");
    assert_eq!(bundle.decision_counts.blocked, 1);
    assert_eq!(bundle.entries.len(), 1);
    let entry = &bundle.entries[0];
    assert_eq!(entry.decision.as_str(), "blocked");
    assert_eq!(entry.reason_code, "blocked-class-apparmor");
    assert_eq!(entry.blocked_classes, vec!["apparmor"]);
    assert!(entry.effects.is_empty());
    assert_eq!(entry.security_policy_intents.len(), 1);
    let intent = &entry.security_policy_intents[0];
    assert_eq!(intent.provider.as_str(), "apparmor");
    assert_eq!(intent.operation, "mode-enforce");
    assert_eq!(intent.fallback.as_str(), "block-on-enforcing-target");
    assert_eq!(intent.reconciliation.state.as_str(), "review");
    assert!(!intent.payload_evidence.payload_backed);
    assert_eq!(bundle.security_policy_intents, vec![intent.clone()]);
    assert_eq!(result.scriptlet_metadata.publication_status, "blocked");
    assert_ne!(result.scriptlet_metadata.publication_status, "public");
    assert_eq!(
        result.scriptlet_metadata.security_policy_intents,
        vec![intent.clone()]
    );
}
```

- [ ] **Step 2: Run the focused converter tests**

Run:

```bash
cargo test -p conary-core apparmor_mode_helper_remains_blocked_with_review_policy_intent --lib
cargo test -p conary-core apparmor_profile_reload_records_public_adapter_backed_policy_intent --lib
```

Expected: PASS.

- [ ] **Step 3: Run the broader conversion proof for LSM terms**

Run:

```bash
cargo test -p conary-core apparmor --lib
cargo test -p conary-core selinux --lib
```

Expected: PASS.

- [ ] **Step 4: Commit**

Run:

```bash
git add crates/conary-core/src/ccs/convert/converter.rs
git commit -m "test: keep AppArmor mode helpers private"
```

## Task 3: Document The LSM Expansion Boundary

**Files:**
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

**Interfaces:**
- Consumes: current `selinux-policy/v1` and `apparmor-policy/v1` behavior
- Produces: docs that state broader LSM public-ready expansion requires provider semantics

- [ ] **Step 1: Update scriptlet security docs**

In `docs/SCRIPTLET_SECURITY.md`, add this sentence after the sentence ending
with "`block-on-enforcing-target` fallback when captured as review intent.":

```markdown
Promoting any broader SELinux or AppArmor form requires target-provider facts
for availability, mode, policy store behavior, profile or module content
validation where applicable, and an operator-visible absent-provider fallback.
```

- [ ] **Step 2: Update CCS module docs**

In `docs/modules/ccs.md`, add this sentence after the sentence ending with
"Mode changes, profile disable/status helpers, broad reloads, and unbacked
paths remain blocked/private and use `block-on-enforcing-target` fallback when
captured as review intent.":

```markdown
Future LSM expansion must add target-provider facts and content semantics
before any mode change, status, disable, directory reload, or policy-store
mutation can become public-ready.
```

- [ ] **Step 3: Update Remi module docs**

In `docs/modules/remi.md`, add this sentence after the sentence ending with
"Mode changes, profile disable/status helpers, broad reloads, and non-payload
profile paths remain blocked/private." in the legacy scriptlet publication gate
section:

```markdown
Remi treats those broader LSM forms as non-public until a later target-provider
policy model proves the provider behavior and fallback semantics.
```

- [ ] **Step 4: Update docs-audit ledger**

In `docs/superpowers/documentation-accuracy-audit-ledger.tsv`, confirm the row
for `docs/superpowers/plans/2026-07-08-lsm-policy-semantics-lock-plan.md`
still names the implementation files touched by this slice:
`crates/conary-core/src/ccs/convert/adapters.rs`,
`crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`, and
`crates/conary-core/src/ccs/convert/converter.rs`. Remove
`docs/modules/test-fixtures.md` from this row unless Task 3 actually changes
that file. Keep exactly 9 tab-separated fields and do not replace literal tabs
with spaces.

- [ ] **Step 5: Regenerate inventory**

Run:

```bash
git add docs/superpowers/plans/2026-07-08-lsm-policy-semantics-lock-plan.md docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 6: Run docs gates**

Run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/plans/2026-07-08-lsm-policy-semantics-lock-plan.md
git commit -m "docs: document LSM policy semantics boundary"
```

## Task 4: Final Verification And Review

> [!NOTE]
> `crates/conary-core/src/ccs/convert/adapters.rs` and
> `crates/conary-core/src/ccs/convert/converter.rs` are over the 2,500-line
> hotspot threshold. This lock plan may add narrow regression tests there, but
> future LSM feature expansion beyond lock coverage must include a reviewed
> decomposition path for adapter registry/parsing and converter projection
> boundaries.

**Files:**
- Read: `docs/superpowers/plans/2026-07-08-lsm-policy-semantics-lock-plan.md`
- Read: `.superpowers/sdd/progress.md`

**Interfaces:**
- Consumes: Task 1 through Task 3 commits
- Produces: final review package and clean verification record

- [ ] **Step 1: Run focused LSM proof**

Run:

```bash
cargo test -p conary-core apparmor --lib
cargo test -p conary-core selinux --lib
cargo test -p conary-core security_policy --lib
```

Expected: PASS.

- [ ] **Step 2: Run conversion/publication smoke**

Run:

```bash
cargo test -p conary-core golden_fixtures --lib
cargo test -p conary-core support_matrix --lib
cargo test -p conary --test conversion_integration golden_conversion
cargo test -p remi publication
```

Expected: PASS.

- [ ] **Step 3: Run docs and hygiene proof**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.

- [ ] **Step 4: Request final code review**

Generate a review package from this slice base commit through `HEAD`, dispatch a
final reviewer with the roadmap design and this plan as required context, and
fix Critical/Important findings before considering the slice complete.

- [ ] **Step 5: Record completion**

Append a line to `.superpowers/sdd/progress.md` naming the concrete 7-character
base and head commit abbreviations for this slice and recording the final
review result.

Do not commit `.superpowers/sdd/progress.md`.
