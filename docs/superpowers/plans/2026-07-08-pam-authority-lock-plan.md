# PAM Authority Lock Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep PAM stack mutation private/blocked for public Remi while broadening the command evidence and fixture proof for common PAM helper forms.

**Architecture:** Extend the existing blocked-class registry and Remi scriptlet-corpus hints so common PAM stack helpers are classified as `blocked-class-pam`. Add support-matrix and converter/publication regressions proving PAM helpers do not create adapter replacement evidence, manifest authority, or public-ready converted rows. Update docs to state that PAM needs a future native adapter, target-profile facts, rollback behavior, and operator-visible review before any public-ready promotion.

**Tech Stack:** Rust unit tests in `conary-core` and `remi`, passive conversion tests, docs-audit checks.

## Global Constraints

- No PAM public-ready command forms in this slice.
- PAM helpers remain `blocked-class-pam` evidence with `publication_status = "blocked"`.
- Do not add a PAM adapter, manifest projection, target-profile PAM facts, replay behavior, or Remi public gate exception.
- Admin/non-public test serving may continue to serve valid blocked rows only through the existing default-off admin lane; public package, detail, index, sparse, OCI, and chunk routes continue to require public-ready status.
- Keep command-evidence and corpus classification sanitized; do not expose host-local paths or review artifact paths.
- Documentation changes must keep `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, `docs/modules/remi.md`, `docs/modules/test-fixtures.md`, docs-audit ledger, and inventory aligned.

---

## File Structure

- Modify `crates/conary-core/src/ccs/convert/blocked_classes.rs`
  - Add common PAM helper command coverage and blocked-class tests.
- Modify `apps/remi/src/server/scriptlet_corpus.rs`
  - Keep advisory corpus blocked-class hints aligned with the blocked-class registry.
- Modify `crates/conary-core/src/ccs/convert/support_matrix.rs`
  - Add explicit fixture/support proof that PAM has only a blocked row and no adapter row.
- Modify `crates/conary-core/src/ccs/convert/converter.rs`
  - Add a passive conversion regression proving a PAM helper remains blocked/private.
- Modify `apps/remi/src/server/publication.rs`
  - Add a Remi publication regression proving blocked PAM summaries stay private and non-public-only for chunks.
- Modify `docs/SCRIPTLET_SECURITY.md`
  - Document the PAM authority boundary alongside boot/security scriptlet evidence.
- Modify `docs/modules/ccs.md`
  - Mirror the PAM blocked/private boundary in the CCS module.
- Modify `docs/modules/remi.md`
  - Clarify that Remi public serving does not promote PAM helper conversions.
- Modify `docs/modules/test-fixtures.md`
  - Register the `blocked-class-pam` fixture intent.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Register this plan and touched claims.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Regenerate after staging this plan and docs.

## Task 1: Extend PAM Blocked-Class And Corpus Hints

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- Modify: `apps/remi/src/server/scriptlet_corpus.rs`

**Interfaces:**
- Consumes: `BlockedClassRegistry::match_invocation`
- Consumes: `ScriptletCorpusSummary::from_scriptlets`
- Produces: common PAM helpers mapped to `blocked-class-pam`
- Produces: corpus `blocked_class_hints = ["pam"]` for the same helper family

- [ ] **Step 1: Add the blocked-class regression**

In `crates/conary-core/src/ccs/convert/blocked_classes.rs`, add this test after `blocked_classes_cover_kernel_install_selinux_module_and_label_tools`:

```rust
#[test]
fn blocked_classes_cover_common_pam_stack_helpers() {
    let registry = BlockedClassRegistry::default();

    for (command, argv) in [
        ("authselect", vec!["select", "sssd", "with-mkhomedir"]),
        ("authconfig", vec!["--enablefaillock", "--update"]),
        ("pam-auth-update", vec!["--package"]),
        ("pam-config", vec!["--add", "--mkhomedir"]),
    ] {
        let class = registry
            .match_invocation(&invocation(command, &argv))
            .unwrap_or_else(|| panic!("missing blocked class for {command}"));
        assert_eq!(class.id, "pam");
        assert_eq!(class.reason_code, "blocked-class-pam");
        assert_eq!(class.default_outcome, BlockedClassOutcome::Blocked);
    }
}
```

- [ ] **Step 2: Add the corpus hint regression**

In `apps/remi/src/server/scriptlet_corpus.rs`, add this test after `corpus_summary_marks_package_manager_recursion`:

```rust
#[test]
fn corpus_summary_marks_common_pam_stack_helpers() {
    let summary = ScriptletCorpusSummary::from_scriptlets(
        "fedora",
        "pam-ish",
        &[scriptlet(
            "authconfig --enablefaillock --update\npam-config --add --mkhomedir\n",
        )],
    );

    assert_eq!(summary.command_counts.get("authconfig"), Some(&1));
    assert_eq!(summary.command_counts.get("pam-config"), Some(&1));
    assert_eq!(summary.blocked_class_hints, vec!["pam".to_string()]);
}
```

- [ ] **Step 3: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core blocked_classes_cover_common_pam_stack_helpers --lib
cargo test -p remi corpus_summary_marks_common_pam_stack_helpers
```

Expected: both FAIL because `authconfig` and `pam-config` are not yet covered.

- [ ] **Step 4: Extend the PAM command lists**

In `crates/conary-core/src/ccs/convert/blocked_classes.rs`, change the PAM blocked-class command list from:

```rust
&["authselect", "pam-auth-update"],
```

to:

```rust
&["authconfig", "authselect", "pam-auth-update", "pam-config"],
```

In `apps/remi/src/server/scriptlet_corpus.rs`, change:

```rust
"authselect" | "pam-auth-update" => {
    classes.push("pam".to_string());
}
```

to:

```rust
"authconfig" | "authselect" | "pam-auth-update" | "pam-config" => {
    classes.push("pam".to_string());
}
```

- [ ] **Step 5: Verify the focused tests pass**

Run:

```bash
cargo test -p conary-core blocked_classes_cover_common_pam_stack_helpers --lib
cargo test -p conary-core pam --lib
cargo test -p remi corpus_summary_marks_common_pam_stack_helpers
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/conary-core/src/ccs/convert/blocked_classes.rs apps/remi/src/server/scriptlet_corpus.rs
git commit -m "test: cover PAM blocked helper forms"
```

## Task 2: Pin PAM Support-Matrix Boundary

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/support_matrix.rs`

**Interfaces:**
- Consumes: `SupportMatrix::default()`
- Produces: explicit test proof that `pam` has a blocked-class row only

- [ ] **Step 1: Add the support-matrix regression**

In `crates/conary-core/src/ccs/convert/support_matrix.rs`, add this test after `boot_security_classes_remain_blocked_without_native_adapters`:

```rust
#[test]
fn pam_class_remains_blocked_without_native_adapter() {
    let matrix = SupportMatrix::default();

    let row = matrix
        .entries()
        .iter()
        .find(|entry| entry.class_id == Some("pam"))
        .expect("missing support row for pam");

    assert_eq!(row.outcome, SupportOutcome::Blocked);
    assert!(row.adapter_id.is_none());
    assert_eq!(row.fixture_names, &["blocked-class-pam"]);
}
```

- [ ] **Step 2: Run the focused test**

Run:

```bash
cargo test -p conary-core pam_class_remains_blocked_without_native_adapter --lib
```

Expected: PASS if the existing support matrix already has the correct row; if it fails, fix the row instead of adding adapter evidence.

- [ ] **Step 3: Run broader support-matrix proof**

Run:

```bash
cargo test -p conary-core support_matrix --lib
```

Expected: PASS.

- [ ] **Step 4: Commit**

Run:

```bash
git add crates/conary-core/src/ccs/convert/support_matrix.rs
git commit -m "test: pin PAM support matrix boundary"
```

## Task 3: Prove PAM Conversion And Publication Stay Private

**Files:**
- Modify: `crates/conary-core/src/ccs/convert/converter.rs`
- Modify: `apps/remi/src/server/publication.rs`

**Interfaces:**
- Consumes: passive converter helpers in `converter.rs`
- Consumes: Remi publication classification helpers in `publication.rs`
- Produces: converter regression `pam_helper_remains_blocked_without_manifest_authority`
- Produces: publication regression `blocked_pam_report_stays_private_and_non_public_only`

- [ ] **Step 1: Add the converter regression**

In `crates/conary-core/src/ccs/convert/converter.rs`, add this test after `apparmor_mode_helper_remains_blocked_with_review_policy_intent`:

```rust
#[test]
fn pam_helper_remains_blocked_without_manifest_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "authconfig --enablefaillock --update\n".to_string(),
        flags: None,
    }];
    let files = make_test_files();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(&metadata, &files, "rpm", "sha256:test")
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
    assert_eq!(entry.reason_code, "blocked-class-pam");
    assert_eq!(entry.blocked_classes, vec!["pam"]);
    assert!(entry.effects.is_empty());
    assert!(entry.boot_security_intents.is_empty());
    assert!(entry.security_policy_intents.is_empty());
    assert!(bundle.boot_security_intents.is_empty());
    assert!(bundle.security_policy_intents.is_empty());
    assert_eq!(result.scriptlet_metadata.publication_status, "blocked");
    assert_eq!(result.scriptlet_metadata.blocked_classes, vec!["pam"]);
    assert_ne!(result.scriptlet_metadata.publication_status, "public");
}
```

- [ ] **Step 2: Add the Remi publication regression**

In `apps/remi/src/server/publication.rs`, add this test after `blocked_apparmor_report_stays_private_and_carries_security_policy_intent`:

```rust
#[test]
fn blocked_pam_report_stays_private_and_non_public_only() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut summary = golden_summary("blocked", "blocked", "blocked");
    summary.decision_counts = ScriptletDecisionCountsSummary {
        blocked: 1,
        ..ScriptletDecisionCountsSummary::default()
    };
    summary
        .blocked_reason_codes
        .push("blocked-class-pam".to_string());
    summary.blocked_classes.push("pam".to_string());
    insert_golden_converted(&conn, "pam-private", "pam-chunk", &summary);

    let converted = ConvertedPackage::find_publication_candidates(&conn, "fedora", None)
        .unwrap()
        .into_iter()
        .find(|converted| converted.package_name.as_deref() == Some("pam-private"))
        .expect("private converted PAM row should remain queryable as server state");
    assert!(!converted.is_scriptlet_public_ready());
    assert_eq!(
        ConvertedPackage::chunk_publication_state(&conn, "pam-chunk").unwrap(),
        ChunkPublicationState::NonPublicOnly
    );

    let report = match classify_converted_package(&converted) {
        PublicationDecision::Blocked(report) => report,
        other => panic!("expected blocked PAM report, got {other:?}"),
    };
    assert_eq!(report.publication_status, "blocked");
    assert_eq!(report.blocked_classes, vec!["pam"]);
    assert!(report.boot_security_intents.is_empty());
    assert!(report.security_policy_intents.is_empty());
    assert!(report.message.contains("pam"));
}
```

- [ ] **Step 3: Run the focused tests**

Run:

```bash
cargo test -p conary-core pam_helper_remains_blocked_without_manifest_authority --lib
cargo test -p remi blocked_pam_report_stays_private_and_non_public_only
```

Expected: PASS after Task 1; if the converter test fails before Task 1 is present, verify `authconfig` is mapped to `blocked-class-pam`.

- [ ] **Step 4: Run publication smoke**

Run:

```bash
cargo test -p remi publication
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/conary-core/src/ccs/convert/converter.rs apps/remi/src/server/publication.rs
git commit -m "test: keep PAM conversions private"
```

## Task 4: Document PAM Public-Authority Boundary

**Files:**
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

**Interfaces:**
- Consumes: Task 1 through Task 3 behavior
- Produces: docs and audit metadata aligned with blocked/private PAM behavior

- [ ] **Step 1: Update scriptlet security docs**

In `docs/SCRIPTLET_SECURITY.md`, add this paragraph after the paragraph ending with "Raw replay, `--no-scripts`, or malformed summary metadata must not make these packages public-ready.":

```markdown
PAM stack helpers such as `authselect`, `authconfig`, `pam-auth-update`, and
`pam-config` are also blocked for public serving. Public-ready PAM conversion
requires a future native PAM adapter with target-profile PAM stack facts,
rollback semantics, and operator-visible review.
```

- [ ] **Step 2: Update CCS module docs**

In `docs/modules/ccs.md`, add this paragraph after the paragraph ending with "Legacy replay, review-required, blocked, malformed, or local-only scriptlet outcomes remain private conversion results.":

```markdown
Common PAM stack helpers (`authselect`, `authconfig`, `pam-auth-update`, and
`pam-config`) remain `blocked-class-pam` evidence. They do not project native
manifest authority or public Remi eligibility without a future native PAM
policy adapter and target-profile PAM facts.
```

- [ ] **Step 3: Update Remi module docs**

In `docs/modules/remi.md`, add this paragraph after the paragraph ending with "This gate is publication-only. It does not replay scriptlets, promote reviewed packages, or change client install/update/remove behavior.":

```markdown
PAM helper conversions are blocked/non-public under the same gate. The
default-off admin test lane may expose sanitized blocked metadata to maintainers,
but public package, detail, index, sparse, OCI, and chunk routes continue to
require public-ready status.
```

- [ ] **Step 4: Update fixture docs**

In `docs/modules/test-fixtures.md`, add this bullet after the private-review sysctl fixture bullets:

```markdown
- `blocked-class-pam`: common PAM stack helper evidence such as `authconfig`
  or `pam-config`, expected blocked outcome until native PAM policy authority is
  modeled.
```

- [ ] **Step 5: Update docs-audit ledger**

In `docs/superpowers/documentation-accuracy-audit-ledger.tsv`, add or update the row for this plan with exactly 9 tab-separated fields:

```tsv
docs/superpowers/plans/2026-07-08-pam-authority-lock-plan.md	docs/superpowers/plans/2026-07-08-pam-authority-lock-plan.md	planning	maintainer	scriptlet-security; pam; remi-publication-gate; implementation-plan	docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md; docs/SCRIPTLET_SECURITY.md; docs/modules/ccs.md; docs/modules/remi.md; docs/modules/test-fixtures.md; crates/conary-core/src/ccs/convert/blocked_classes.rs; crates/conary-core/src/ccs/convert/support_matrix.rs; crates/conary-core/src/ccs/convert/converter.rs; apps/remi/src/server/scriptlet_corpus.rs; apps/remi/src/server/publication.rs	verified	corrected	Implementation plan for locking PAM helper scriptlet handling: common PAM stack helpers remain blocked/private conversion evidence until a native PAM adapter, target-profile PAM facts, rollback behavior, and operator-visible review are designed.
```

Do not replace literal tabs with spaces.

- [ ] **Step 6: Regenerate inventory**

Run:

```bash
git add docs/superpowers/plans/2026-07-08-pam-authority-lock-plan.md docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 7: Run docs gates**

Run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected: PASS.

- [ ] **Step 8: Commit**

Run:

```bash
git add docs/SCRIPTLET_SECURITY.md docs/modules/ccs.md docs/modules/remi.md docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/plans/2026-07-08-pam-authority-lock-plan.md
git commit -m "docs: document PAM authority boundary"
```

## Task 5: Final Verification And Review

**Files:**
- Read: `docs/superpowers/plans/2026-07-08-pam-authority-lock-plan.md`
- Read: `.superpowers/sdd/progress.md`

**Interfaces:**
- Consumes: Task 1 through Task 4 commits
- Produces: final review package and clean verification record

- [ ] **Step 1: Run focused PAM proof**

Run:

```bash
cargo test -p conary-core pam --lib
cargo test -p remi corpus_summary_marks_common_pam_stack_helpers
cargo test -p remi blocked_pam_report_stays_private_and_non_public_only
```

Expected: PASS.

- [ ] **Step 2: Run conversion/publication smoke**

Run:

```bash
cargo test -p conary-core support_matrix --lib
cargo test -p conary-core golden_fixtures --lib
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

Append a line to `.superpowers/sdd/progress.md` naming the concrete
7-character base and head commit abbreviations for this slice and recording the
final review result.

Do not commit `.superpowers/sdd/progress.md`.
