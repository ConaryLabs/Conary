# Docs Source Selection Refresh Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring canonical docs, assistant-facing docs, and user-facing copy back in sync with the implemented source-selection and executable replatform behavior.

**Architecture:** Create one narrow canonical module doc for source selection, make the architecture and LLM maps point to it, refresh the public handbook where command behavior changed, and remove the stale standalone feature-audit ledger instead of trying to keep it current. Keep behavioral changes limited to user-facing wording that still describes pre-merge behavior.

**Tech Stack:** Markdown, Rust user-facing strings, existing unit tests in `apps/conary` and `conary-core`, `rg`, `cargo test`

**Commit Convention:** Each commit in this plan should reference `docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md` in the commit body.

---

## File Map

- Delete: `docs/FEATURE-AUDIT-2026-03-28.md`
  - Remove the stale status ledger rather than refreshing a document that is not referenced elsewhere.
- Create: `docs/modules/source-selection.md`
  - Canonical source-selection doc covering model fields, runtime mirrors, ranking modes, eligibility, and flow behavior across install, update, and model/replatform paths.
- Modify: `docs/llms/README.md`
  - Point assistants at the new canonical source-selection doc.
- Modify: `docs/llms/subsystem-map.md`
  - Add stable “look here first” pointers for `effective_policy`, source-selection config, and replatform execution.
- Modify: `docs/ARCHITECTURE.md`
  - Acknowledge the shared runtime policy layer and the fact that model/update/install now share source-selection behavior.
- Modify: `docs/conaryopedia-v2.md`
  - Refresh the public handbook for `conary distro`, `selection-mode`, model `[system]` fields, update source-switch behavior, and model apply/replatform behavior.
- Modify: `docs/INTEGRATION-TESTING.md`
  - Reconcile Phase 4 group summaries, ranges, and positive-path examples with the current manifests.
- Modify: `apps/conary-test/README.md`
  - Keep the suite inventory aligned with the Phase 4 manifest coverage descriptions.
- Modify: `apps/conary/src/commands/model.rs`
  - Replace stale “planning-only / without replacing packages yet” messaging with wording that matches current behavior.
- Modify: `crates/conary-core/src/model/diff.rs`
  - Replace stale pending-convergence warning text and update tests that assert on the old wording.
- Modify: `README.md`
  - Add a small discoverability mention for the `conary distro` source-selection surface.
- Modify: `ROADMAP.md`
  - Reword the “safer migration flows” roadmap item so it reflects landed replatform work and the remaining gaps.
- Modify: `docs/superpowers/plans/2026-04-07-source-selection-program-plan.md`
  - Mark the executed source-selection program plan as completed so future agents do not treat it as pending work.

## Chunk 1: Canonical Docs and Maps

### Task 1: Remove the stale feature-audit ledger

**Files:**
- Delete: `docs/FEATURE-AUDIT-2026-03-28.md`

- [ ] **Step 1: Confirm the file is unreferenced before deleting it**

Run: `rg -n "FEATURE-AUDIT-2026-03-28|Feature Audit|feature audit" .`
Expected: only `docs/FEATURE-AUDIT-2026-03-28.md` is returned

- [ ] **Step 2: Delete the stale file**

Delete: `docs/FEATURE-AUDIT-2026-03-28.md`

- [ ] **Step 3: Re-run the reference search to confirm nothing still points at it**

Run: `rg -n "FEATURE-AUDIT-2026-03-28|Feature Audit|feature audit" .`
Expected: no matches

- [ ] **Step 4: Commit**

```bash
git add -u docs/FEATURE-AUDIT-2026-03-28.md
git commit -m "docs: remove stale feature audit ledger" -m "Part of docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md"
```

### Task 2: Add one canonical source-selection doc

**Files:**
- Create: `docs/modules/source-selection.md`

- [ ] **Step 1: Draft the document outline directly from the implemented code surfaces**

Required sections:
- Purpose and scope
- Model-layer inputs: `system.profile`, `system.selection_mode`, `allowed_distros`, `pin`, `convergence`
- Runtime mirrors: `source.selection-mode`, `source.allowed-distros`, distro pin state
- Effective policy loading via `crates/conary-core/src/repository/effective_policy.rs`
- Ranking semantics: `policy` vs `latest`
- Eligibility vs ranking
- Flow behavior:
  - install
  - update
  - model diff/apply
  - replatform execution planning
- Operator entry points:
  - `conary distro set`
  - `conary distro mixing`
  - `conary distro selection-mode`
  - `conary distro info`

- [ ] **Step 2: Write the canonical doc**

Source from:
- `crates/conary-core/src/model/parser.rs`
- `crates/conary-core/src/repository/effective_policy.rs`
- `apps/conary/src/commands/distro.rs`
- `apps/conary/src/commands/update.rs`
- `apps/conary/src/commands/model.rs`
- `crates/conary-core/src/model/replatform.rs`

Requirements:
- Keep this doc canonical and current-state only.
- Do not copy plan/spec history into it.
- Be explicit about transitional defaults:
  - model-backed default profile maps to `latest`
  - runtime setting default remains `policy` when unset
- Explain that `latest` uses the Repology-backed newest signal among allowed candidates.
- Explain that explicit version constraints remain strict and scheme-aware.

- [ ] **Step 3: Verify the new doc covers every implemented surface we identified**

Run:
- `rg -n "selection_mode|allowed_distros|profile|load_effective_policy|distro selection-mode|replatform" docs/modules/source-selection.md`
- `rg -l "load_effective_policy" crates/conary-core/src/repository/`
- `rg -l "selection_mode" crates/conary-core/src/model/parser.rs`
- `rg -l "cmd_distro_selection_mode|render_distro_info" apps/conary/src/commands/distro.rs`
- `rg -l "replatform_execution_plan" apps/conary/src/commands/model.rs crates/conary-core/src/model/replatform.rs`

Expected:
- the new doc contains the major concepts above
- the documented code entry points still exist where the doc says they do

- [ ] **Step 4: Commit**

```bash
git add docs/modules/source-selection.md
git commit -m "docs: add canonical source selection module guide" -m "Part of docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md"
```

### Task 3: Refresh assistant maps and architecture pointers

**Files:**
- Modify: `docs/llms/README.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/ARCHITECTURE.md`

- [ ] **Step 1: Update `docs/llms/README.md` to link the new canonical doc**

Required edits:
- Add `docs/modules/source-selection.md` to the Core Docs list
- Bump frontmatter `last_updated` and `revision`

- [ ] **Step 2: Update `docs/llms/subsystem-map.md` with stable pointers**

Required edits:
- Add a “look here first” bullet for:
  - `crates/conary-core/src/repository/effective_policy.rs`
  - `crates/conary-core/src/model/parser.rs`
  - `crates/conary-core/src/model/replatform.rs`
  - `apps/conary/src/commands/distro.rs`
  - `apps/conary/src/commands/update.rs`
  - `apps/conary/src/commands/model.rs`
- Add `docs/modules/source-selection.md` under “Prefer Existing Deep Dives”

- [ ] **Step 3: Update `docs/ARCHITECTURE.md` where the module map is now too coarse**

Required edits:
- In the `repository/` section, mention the shared effective source-policy loader and selection/ranking behavior
- In the `model/` section, note that `replatform.rs` now supports executable transaction planning, not just abstract planning
- Keep this high-level; do not duplicate the new module doc

- [ ] **Step 4: Verify the new pointers resolve cleanly and no stale maps remain**

Run: `rg -n "source-selection|source selection|effective_policy|replatform" docs/llms/README.md docs/llms/subsystem-map.md docs/ARCHITECTURE.md`
Expected: the new doc and code entry points are now discoverable from all three files

- [ ] **Step 5: Commit**

```bash
git add docs/llms/README.md docs/llms/subsystem-map.md docs/ARCHITECTURE.md
git commit -m "docs: refresh assistant and architecture source-policy maps" -m "Part of docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md"
```

## Chunk 2: Public Docs and Runtime Copy Alignment

### Task 4: Refresh the public handbook for the new source-selection surface

**Files:**
- Modify: `docs/conaryopedia-v2.md`

- [ ] **Step 1: Update the update-command section**

Required edits in the existing update chapter:
- Explain that update behavior depends on the effective source-selection policy
- Note that `selection_mode=latest` can choose a different allowed source when a newer candidate is available
- Document `--dry-run` as the preview path for source-switching updates
- Note that applying source-switching updates requires confirmation / `--yes`

- [ ] **Step 2: Add or expand the distro/source-policy coverage**

Required additions:
- `conary distro set`
- `conary distro mixing`
- `conary distro selection-mode`
- `conary distro info`
- Short explanation of:
  - pin/mixing as eligibility controls
  - selection mode as ranking control
  - affinity output and runtime default display

- [ ] **Step 3: Update the model chapter**

Required edits:
- Add `[system]` to the “Model Sections” table with the source-policy/system configuration purpose
- Expand the `[system]` examples to include `profile`, `selection_mode`, `allowed_distros`, and richer `pin`
- Describe how model-layer source policy mirrors into runtime settings
- Update `model apply` text so it reflects executable replatform transactions when available, with blocked transactions surfaced explicitly
- Correct the stale code snippets in the handbook so they match the current code:
  - `SystemModel` from `crates/conary-core/src/model/parser.rs`
  - `ResolvedModel` from `crates/conary-core/src/model/mod.rs`
  - `DiffAction` from `crates/conary-core/src/model/diff.rs`
- Keep those snippets pedagogical:
  - show the important fields and shapes
  - avoid dumping serde attributes, hidden fields, or unrelated implementation machinery verbatim unless they are necessary to explain behavior

- [ ] **Step 4: Verify the public handbook now exposes the new terms**

Run:
- `rg -n "selection-mode|Selection mode|allowed_distros|selection_mode|profile|conary distro|source policy|replatform|Model Sections|SystemModel|ResolvedModel|DiffAction" docs/conaryopedia-v2.md`
- `rg -n "pub struct SystemModel" crates/conary-core/src/model/parser.rs`
- `rg -n "pub struct ResolvedModel" crates/conary-core/src/model/mod.rs`
- `rg -n "pub enum DiffAction" crates/conary-core/src/model/diff.rs`

Expected:
- matches in the update, source-policy/distro, and model sections
- the documented snippets still map to real code entry points

- [ ] **Step 5: Commit**

```bash
git add docs/conaryopedia-v2.md
git commit -m "docs: refresh public source-selection handbook coverage" -m "Part of docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md"
```

### Task 5: Reconcile integration-test docs with current Phase 4 coverage

**Files:**
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `apps/conary-test/README.md`

- [ ] **Step 1: Update the Phase 4 summary table and descriptions from the current manifests**

Required edits:
- Reconcile Group D and Group E ranges against:
  - `apps/conary/tests/integration/remi/manifests/phase4-group-d.toml`
  - `apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`
- If ranges still overlap in the manifests, document that overlap clearly instead of pretending the groups are disjoint
- Expand Group E beyond “Cross-distro compatibility” so it reflects distro pinning, replatform apply, mixing-policy behavior, takeover, and native package compatibility coverage

- [ ] **Step 2: Refresh the positive-path coverage list**

Required additions:
- mention the distro command family coverage
- mention model-driven replatform/apply coverage
- mention cross-distro takeover / ownership-ladder coverage

- [ ] **Step 3: Keep `apps/conary-test/README.md` in sync with the same manifest ranges**

Required edits:
- update the Phase 4 Group D / E range comments to match the actual manifests
- keep the group descriptions aligned with `docs/INTEGRATION-TESTING.md`

- [ ] **Step 4: Verify the docs now match the authoritative manifest files**

Run:
- `rg -n "^id = \"T" apps/conary/tests/integration/remi/manifests/phase4-group-d.toml | tail -n 5`
- `rg -n "^id = \"T" apps/conary/tests/integration/remi/manifests/phase4-group-e.toml | tail -n 5`
- `rg -n "Phase 4|Group D|Group E|Cross-distro|replatform|takeover|distro" docs/INTEGRATION-TESTING.md apps/conary-test/README.md`

Expected:
- the docs match the current Phase 4 manifest boundaries and descriptions
- Group E coverage is described in enough detail to reflect the current suite

- [ ] **Step 5: Commit**

```bash
git add docs/INTEGRATION-TESTING.md apps/conary-test/README.md
git commit -m "docs(testing): align phase4 coverage docs with current manifests" -m "Part of docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md"
```

### Task 6: Align stale model and diff messaging with current behavior

**Files:**
- Modify: `apps/conary/src/commands/model.rs`
- Modify: `crates/conary-core/src/model/diff.rs`
- Test: `apps/conary/src/commands/model.rs`
- Test: `crates/conary-core/src/model/diff.rs`

- [ ] **Step 1: Update the `conary` test assertions first to describe the intended new behavior**

Required replacements:
- Replace assertion targets tied to “without replacing packages yet” with wording that reflects:
  - policy changes can still be applied on their own
  - executable replatform transactions run when available
  - blocked realignment remains pending rather than “the system can’t do this yet”
- Add an explicit note in the test-adjacent code review/comments if helpful that the existing mixed-state line
  - `Executable replatform transactions will be applied through the shared install path; blocked ones will remain pending and be reported as errors.`
  is reviewed and intentionally kept because it already matches current behavior

Update assertions in:
- `test_source_policy_summary_for_policy_only_transition`
- `test_source_policy_summary_policy_only_stays_conservative`

- [ ] **Step 2: Run the focused `conary` tests to confirm the new assertions fail against the old strings**

Run:
- `cargo test -p conary test_source_policy_summary_for_policy_only_transition -- --exact`
- `cargo test -p conary test_source_policy_summary_policy_only_stays_conservative -- --exact`
Expected: FAIL because the assertions now target wording that has not been implemented yet

- [ ] **Step 3: Update the `model.rs` summary strings**

Required replacements:
- Replace “without replacing packages yet” with wording that reflects:
  - source-policy-only changes can still be applied immediately
  - executable replatform transactions run when available
  - blocked or not-yet-selected realignment remains pending without implying the feature is unimplemented
- Replace “planning-only in this slice / automatic replacement execution is still pending” with wording that says no executable transactions are available in this plan, while blocked transactions remain reported

- [ ] **Step 4: Re-run the focused `conary` tests**

Run:
- `cargo test -p conary test_source_policy_summary_for_policy_only_transition -- --exact`
- `cargo test -p conary test_source_policy_summary_policy_only_stays_conservative -- --exact`
Expected: PASS

- [ ] **Step 5: Update the `conary-core` test assertions for the diff warning**

Required replacement:
- Replace assertion targets tied to “automatic package convergence planning is still pending” with wording that describes pending or blocked realignment more accurately without implying the implementation is missing

Update assertions in:
- `test_source_pin_only_transition_warns_about_pending_convergence`
- `test_source_pin_with_package_changes_does_not_emit_pending_convergence_warning`

- [ ] **Step 6: Run the focused `conary-core` tests to confirm the new assertions fail against the old warning**

Run:
- `cargo test -p conary-core test_source_pin_only_transition_warns_about_pending_convergence -- --exact`
- `cargo test -p conary-core test_source_pin_with_package_changes_does_not_emit_pending_convergence_warning -- --exact`
Expected: FAIL because the assertions now target wording that has not been implemented yet

- [ ] **Step 7: Update the diff warning text**

Required replacement:
- Replace “automatic package convergence planning is still pending” with wording that describes pending or blocked realignment more accurately without implying the implementation is missing

- [ ] **Step 8: Run the focused tests again after implementation**

Run:
- `cargo test -p conary test_source_policy_summary_for_policy_only_transition -- --exact`
- `cargo test -p conary test_source_policy_summary_policy_only_stays_conservative -- --exact`
- `cargo test -p conary-core test_source_pin_only_transition_warns_about_pending_convergence -- --exact`
- `cargo test -p conary-core test_source_pin_with_package_changes_does_not_emit_pending_convergence_warning -- --exact`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add apps/conary/src/commands/model.rs crates/conary-core/src/model/diff.rs
git commit -m "fix(model): align source-policy copy with executable replatforming" -m "Part of docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md"
```

### Task 7: Refresh top-level discoverability and plan-status docs

**Files:**
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `docs/superpowers/plans/2026-04-07-source-selection-program-plan.md`

- [ ] **Step 1: Update the root README for source-selection discoverability**

Required edits:
- Add a brief mention in the declarative-model or cross-distro sections that users can steer source selection with the `conary distro` command family
- Include at least one compact example showing `conary distro selection-mode latest` or `conary distro info`

- [ ] **Step 2: Reword the roadmap item that now partially overlaps landed work**

Required edit:
- Replace “Safer migration flows for changing system roles” with wording that reflects:
  - basic replatform/migration flows now exist
  - the remaining roadmap work is around polish, safeguards, and broader validation

- [ ] **Step 3: Mark the executed source-selection implementation plan as completed**

Required edit:
- Add a short status note near the top of `docs/superpowers/plans/2026-04-07-source-selection-program-plan.md` stating that the plan was executed and merged, so future readers do not treat it as pending work

- [ ] **Step 4: Verify the discoverability and status updates landed**

Run:
- `rg -n "distro|selection-mode|source selection" README.md`
- `rg -n "migration flows|replatform|changing system roles" ROADMAP.md`
- `rg -n "executed|merged|completed|status" docs/superpowers/plans/2026-04-07-source-selection-program-plan.md`

Expected:
- README exposes the source-selection surface
- ROADMAP describes the remaining replatform work, not the already-landed core
- the old implementation plan clearly reads as completed work

- [ ] **Step 5: Commit**

```bash
git add README.md ROADMAP.md docs/superpowers/plans/2026-04-07-source-selection-program-plan.md
git commit -m "docs: refresh source-selection discoverability and plan status" -m "Part of docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md"
```

### Task 8: Run final docs-drift verification

**Files:**
- Verify: `docs/modules/source-selection.md`
- Verify: `docs/llms/README.md`
- Verify: `docs/llms/subsystem-map.md`
- Verify: `docs/ARCHITECTURE.md`
- Verify: `docs/conaryopedia-v2.md`
- Verify: `docs/INTEGRATION-TESTING.md`
- Verify: `apps/conary-test/README.md`
- Verify: `apps/conary/src/commands/model.rs`
- Verify: `crates/conary-core/src/model/diff.rs`
- Verify: `README.md`
- Verify: `ROADMAP.md`
- Verify: `docs/superpowers/plans/2026-04-07-source-selection-program-plan.md`

- [ ] **Step 1: Search for the stale phrases we intentionally removed**

Run:
```bash
rg -n \
  "without replacing packages yet|planning-only in this slice|automatic replacement execution is still pending|automatic package convergence planning is still pending" \
  docs/ARCHITECTURE.md \
  docs/conaryopedia-v2.md \
  docs/INTEGRATION-TESTING.md \
  docs/llms/README.md \
  docs/llms/subsystem-map.md \
  docs/modules/source-selection.md \
  apps/conary-test/README.md \
  apps/conary/src/commands/model.rs \
  crates/conary-core/src/model/diff.rs \
  README.md \
  ROADMAP.md
```
Expected: no matches

- [ ] **Step 2: Run focused tests and a formatting sanity check**

Run:
- `cargo test -p conary test_source_policy_summary_for_policy_only_transition -- --exact`
- `cargo test -p conary test_source_policy_summary_policy_only_stays_conservative -- --exact`
- `cargo test -p conary-core test_source_pin_only_transition_warns_about_pending_convergence -- --exact`
- `cargo test -p conary-core test_source_pin_with_package_changes_does_not_emit_pending_convergence_warning -- --exact`
- `cargo fmt --check`

Expected: PASS

- [ ] **Step 3: Commit the final sweep if any uncommitted doc/link cleanup remains**

```bash
git add docs/modules/source-selection.md docs/llms/README.md docs/llms/subsystem-map.md docs/ARCHITECTURE.md docs/conaryopedia-v2.md docs/INTEGRATION-TESTING.md apps/conary-test/README.md apps/conary/src/commands/model.rs crates/conary-core/src/model/diff.rs README.md ROADMAP.md docs/superpowers/plans/2026-04-07-source-selection-program-plan.md
git commit -m "docs: finish source-selection doc refresh sweep" -m "Part of docs/superpowers/plans/2026-04-07-docs-source-selection-refresh-plan.md"
```
