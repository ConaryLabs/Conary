# Feature Claims Completion Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring active feature claims, command examples, implementation, and automated coverage into alignment so the documented Conary surface is real, test-backed, and consistently named.

**Architecture:** Tackle the work in four lanes: finish partial feature implementations, standardize all active command references on the current CLI, add positive automated coverage for the weak spots, and only then refresh the active docs and audit. Prefer small vertical slices that land behavior, tests, and user-facing wording together for each feature family.

**Tech Stack:** Rust workspace (`conary`, `conary-core`, `conary-server`, `conary-test`), Clap CLI, SQLite migrations/models, integration manifests under `tests/integration/remi`, Markdown docs, existing Python native-package harnesses where useful.

---

## File Map

- `src/cli/bootstrap.rs`: canonical bootstrap subcommand names and help text
- `src/cli/capability.rs`: public capability command surface
- `src/cli/collection.rs`: collection UX reference point
- `src/commands/derived.rs`: derive CLI behavior, build/show/stale output, installability
- `src/commands/capability.rs`: capability show/list/validate/run command implementations
- `src/commands/install/mod.rs`: local package install routing and install-side hooks for derived outputs
- `src/commands/install/batch.rs`: component-aware install path and scriptlet gating checks
- `src/commands/update.rs`: parent-update stale propagation and rollback-safe derived refresh bookkeeping
- `src/commands/config.rs`: tracked-config happy-path operations
- `src/commands/label.rs`: label add/delegate/link behavior used by CLI and tests
- `src/commands/triggers.rs`: trigger mutation flows
- `src/commands/federation.rs`: federation management proof points
- `src/commands/provenance.rs`: provenance diff/export/audit proof points
- `src/commands/self_update.rs` and `conary-core/src/self_update.rs`: keep self-update claims/tests aligned while refreshing docs
- `src/commands/bootstrap/mod.rs` and `conary-core/src/bootstrap/*`: bootstrap runtime paths behind the renamed commands
- `conary-core/src/derived/mod.rs` and `conary-core/src/derived/builder.rs`: derived-package build artifacts
- `conary-core/src/db/models/derived.rs`: persisted derived-package metadata/state
- `conary-core/src/db/schema.rs` and `conary-core/src/db/migrations/v41_current.rs`: migration and schema updates if derived artifacts need persisted columns/tables
- `conary-core/src/capability/declaration.rs`: capability declaration lookup for runtime enforcement
- `conary-core/src/capability/enforcement/mod.rs`: runtime restriction application
- `conary-core/src/components/*`: component classification defaults and lookup helpers
- `tests/features.rs`: DB/model-level feature tests
- `tests/component.rs`: component install behavior tests
- `tests/workflow.rs`: install/remove/local-format workflow tests
- `tests/scriptlet_harness/test_scriptlets.py`: native package end-to-end harness if reused instead of new Rust-only coverage
- `tests/integration/remi/manifests/phase2-group-c.toml`: bootstrap coverage
- `tests/integration/remi/manifests/phase3-group-l.toml`: bootstrap/self-update lifecycle coverage
- `tests/integration/remi/manifests/phase4-group-a.toml`: config, distro, canonical, groups, registry coverage
- `tests/integration/remi/manifests/phase4-group-b.toml`: model, collection, derive coverage
- `tests/integration/remi/manifests/phase4-group-c.toml`: CCS, bootstrap-adjacent, query/label coverage
- `tests/integration/remi/manifests/phase4-group-d.toml`: trust, provenance, capability, trigger, federation, automation coverage
- `tests/integration/remi/manifests/phase4-group-e.toml`: cross-distro, local-package install, replatform, takeover-adjacent coverage
- `README.md`, `docs/conaryopedia-v2.md`, `docs/INTEGRATION-TESTING.md`, `docs/modules/*.md`, `docs/FEATURE-AUDIT-2026-03-28.md`: active docs and final audit refresh

## Chunk 1: Core Feature Completion

### Task 1: Persist Real Derived Build Outputs

**Files:**
- Modify: `src/commands/derived.rs`
- Modify: `conary-core/src/derived/mod.rs`
- Modify: `conary-core/src/derived/builder.rs`
- Modify: `conary-core/src/db/models/derived.rs`
- Modify: `conary-core/src/db/schema.rs`
- Modify: `conary-core/src/db/migrations/v41_current.rs`
- Test: `tests/features.rs`
- Test: `tests/workflow.rs`

- [ ] **Step 1: Write the failing persistence tests**

```rust
#[test]
fn test_derive_build_records_installable_artifact_metadata() {
    // create derived package, build it, assert artifact path/hash and parent build input are stored
}
```

- [ ] **Step 2: Run the narrow tests to confirm the gap**

Run: `cargo test --features server --test features test_derived_package_status -- --exact`
Expected: FAIL or no assertion covering artifact persistence yet

- [ ] **Step 3: Add derived artifact persistence**

Implement a concrete persisted result for `derive build`:
- artifact hash/path and build metadata in the DB
- parent input/version snapshot for future stale detection
- migration only if new columns are truly required

- [ ] **Step 4: Make `derive show` and `derive build` surface the real artifact**

Update `src/commands/derived.rs` so build output reports the artifact and `show` can display the last successful build details.

- [ ] **Step 5: Run focused verification**

Run:
- `cargo test --features server --test features`
- `cargo test --features server --test workflow`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/commands/derived.rs conary-core/src/derived/mod.rs conary-core/src/derived/builder.rs conary-core/src/db/models/derived.rs conary-core/src/db/schema.rs conary-core/src/db/migrations/v41_current.rs tests/features.rs tests/workflow.rs
git commit -m "feat(derive): persist installable derived build outputs"
```

### Task 2: Wire Derived Stale Tracking Into Install and Update Flows

**Files:**
- Modify: `src/commands/derived.rs`
- Modify: `src/commands/install/mod.rs`
- Modify: `src/commands/update.rs`
- Modify: `src/commands/install/batch.rs`
- Test: `tests/features.rs`
- Test: `tests/workflow.rs`
- Test: `tests/integration/remi/manifests/phase4-group-b.toml`

- [ ] **Step 1: Write failing stale-propagation tests**

```rust
#[test]
fn test_parent_update_marks_built_derived_package_stale() {
    // install parent v1, build derived, update parent to v2, assert derived status becomes stale
}
```

- [ ] **Step 2: Run the narrow stale test**

Run: `cargo test --features server --test features derived -- --nocapture`
Expected: FAIL because stale propagation is still disconnected

- [ ] **Step 3: Wire stale marking into the real update/install path**

Trigger stale transitions from the package update/install pipeline instead of the dead-code helper so parent changes update derived status consistently.

- [ ] **Step 4: Make `phase4-group-b.toml` prove a positive derive flow**

Replace the current “failure acceptable” derive coverage with:
- create derived definition
- build derived successfully
- verify the recorded artifact/build state
- update parent and verify `derive stale`

- [ ] **Step 5: Run verification**

Run:
- `cargo test --features server --test features`
- `cargo test --features server --test workflow`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/commands/derived.rs src/commands/install/mod.rs src/commands/update.rs src/commands/install/batch.rs tests/features.rs tests/workflow.rs tests/integration/remi/manifests/phase4-group-b.toml
git commit -m "feat(derive): mark derived packages stale on parent updates"
```

### Task 3: Promote `capability run` to a Supported Public Flow

**Files:**
- Modify: `src/cli/capability.rs`
- Modify: `src/commands/capability.rs`
- Modify: `src/main.rs`
- Modify: `conary-core/src/capability/declaration.rs`
- Modify: `conary-core/src/capability/enforcement/mod.rs`
- Test: `src/commands/capability.rs`
- Test: `tests/integration/remi/manifests/phase4-group-d.toml`

- [ ] **Step 1: Add failing runtime-enforcement tests**

```rust
#[test]
fn test_capability_run_uses_declared_restrictions_for_installed_package() {
    // look up package declaration and assert the execution path builds an enforcement context
}
```

- [ ] **Step 2: Run the narrow capability tests**

Run: `cargo test --features server capability`
Expected: FAIL or missing runtime-run coverage

- [ ] **Step 3: Expose the command publicly**

Remove the hidden/unimplemented status from `Run`, route it in `src/main.rs`, and make the command execute under the package's declared capability envelope.

- [ ] **Step 4: Add positive integration coverage**

Extend `phase4-group-d.toml` to install a small local CCS package with a declaration and prove:
- `conary capability list`
- `conary capability show <pkg>`
- `conary capability run <pkg> -- <safe-command>`

- [ ] **Step 5: Run verification**

Run:
- `cargo test --features server capability`
- `cargo test --features server`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/cli/capability.rs src/commands/capability.rs src/main.rs conary-core/src/capability/declaration.rs conary-core/src/capability/enforcement/mod.rs tests/integration/remi/manifests/phase4-group-d.toml
git commit -m "feat(capability): support runtime capability run"
```

## Chunk 2: Command Surface and Coverage Expansion

### Task 4: Standardize Command References in Test Manifests and CLI-Facing Help

**Files:**
- Modify: `src/cli/bootstrap.rs`
- Modify: `tests/integration/remi/manifests/phase2-group-c.toml`
- Modify: `tests/integration/remi/manifests/phase3-group-l.toml`
- Modify: `tests/integration/remi/manifests/phase4-group-b.toml`
- Modify: `tests/integration/remi/manifests/phase4-group-d.toml`
- Test: `src/main.rs`

- [ ] **Step 1: Add/adjust CLI parsing tests for the canonical command names**

```rust
#[test]
fn cli_accepts_bootstrap_cross_tools_name() {
    // parse "conary bootstrap cross-tools"
}
```

- [ ] **Step 2: Run the CLI tests**

Run: `cargo test --features server cli`
Expected: PASS once current canonical names are asserted

- [ ] **Step 3: Replace drifted manifest commands**

Update integration manifests to use:
- `bootstrap cross-tools`
- `bootstrap temp-tools`
- collection CRUD instead of `update-group`
- `capability run` instead of old wording

- [ ] **Step 4: Keep test descriptions honest**

Rewrite manifest comments/descriptions that still say “verify no panic” where the test now proves a successful workflow.

- [ ] **Step 5: Run verification**

Run: `cargo test --features server`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/cli/bootstrap.rs tests/integration/remi/manifests/phase2-group-c.toml tests/integration/remi/manifests/phase3-group-l.toml tests/integration/remi/manifests/phase4-group-b.toml tests/integration/remi/manifests/phase4-group-d.toml src/main.rs
git commit -m "test(cli): standardize feature manifests on current commands"
```

### Task 5: Add Positive Coverage for Config, Labels, and Triggers

**Files:**
- Modify: `src/commands/config.rs`
- Modify: `src/commands/label.rs`
- Modify: `src/commands/triggers.rs`
- Modify: `tests/integration/remi/manifests/phase4-group-a.toml`
- Modify: `tests/integration/remi/manifests/phase4-group-c.toml`
- Modify: `tests/integration/remi/manifests/phase4-group-d.toml`
- Create: `tests/fixtures/phase4-runtime-fixture/ccs.toml`
- Create: `tests/fixtures/phase4-runtime-fixture/stage/...`

- [ ] **Step 1: Build a small tracked-file fixture**

Create a local CCS fixture that installs:
- a binary or script for shell/run tests
- a tracked config file under `/etc/...`
- metadata safe for label/trigger/config scenarios

- [ ] **Step 2: Replace negative-only config tests with positive tracked-file tests**

Update `phase4-group-a.toml` so it installs the fixture, edits a tracked config, then proves:
- `config diff`
- `config backup`
- `config backups`
- `config restore`

- [ ] **Step 3: Add real label mutation tests**

In `phase4-group-c.toml`, add positive coverage for:
- `query label add`
- `query label delegate`
- `query label link`
- `query label show`

- [ ] **Step 4: Add real trigger mutation tests**

In `phase4-group-d.toml`, add positive coverage for:
- `system trigger enable`
- `system trigger disable`
- `system trigger add`
- `system trigger remove`

- [ ] **Step 5: Run verification**

Run:
- `cargo test --features server`
- `cargo run -p conary-test -- list`

Expected: PASS and manifests still parse/list cleanly

- [ ] **Step 6: Commit**

```bash
git add src/commands/config.rs src/commands/label.rs src/commands/triggers.rs tests/integration/remi/manifests/phase4-group-a.toml tests/integration/remi/manifests/phase4-group-c.toml tests/integration/remi/manifests/phase4-group-d.toml tests/fixtures/phase4-runtime-fixture
git commit -m "test(phase4): add positive config label and trigger coverage"
```

### Task 6: Prove `ccs shell`, `ccs run`, and Selective Component Installs

**Files:**
- Modify: `src/commands/ccs/install.rs`
- Modify: `src/commands/install/batch.rs`
- Modify: `tests/component.rs`
- Modify: `tests/integration/remi/manifests/phase4-group-c.toml`
- Modify: `tests/fixtures/phase4-runtime-fixture/ccs.toml`
- Modify: `tests/fixtures/phase4-runtime-fixture/stage/...`

- [ ] **Step 1: Add a fixture layout that exposes runtime vs devel/config split**

Use the local phase-4 fixture to include:
- `/usr/bin/...` runtime content
- `/usr/include/...` devel content
- tracked config

- [ ] **Step 2: Add failing component-selection tests**

```rust
#[test]
fn test_component_install_excludes_runtime_files_when_only_devel_selected() {
    // assert only the requested component lands
}
```

- [ ] **Step 3: Add positive phase-4 shell/run coverage**

Extend `phase4-group-c.toml` to:
- build the local fixture
- `ccs shell <pkg>` and verify the package content is present
- `ccs run <pkg> -- <command>` and verify the command succeeds

- [ ] **Step 4: Add positive selective-component coverage**

Use either the local fixture or Rust integration tests to prove a devel-only install leaves runtime files absent and does not spuriously run runtime-only scriptlets.

- [ ] **Step 5: Run verification**

Run:
- `cargo test --features server --test component`
- `cargo test --features server`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/commands/ccs/install.rs src/commands/install/batch.rs tests/component.rs tests/integration/remi/manifests/phase4-group-c.toml tests/fixtures/phase4-runtime-fixture/ccs.toml tests/fixtures/phase4-runtime-fixture/stage
git commit -m "test(ccs): cover shell run and selective component installs"
```

### Task 7: Add Positive Native Local-Install Coverage and Revisit Takeover

**Files:**
- Modify: `tests/workflow.rs`
- Modify: `tests/scriptlet_harness/test_scriptlets.py`
- Modify: `tests/integration/remi/manifests/phase4-group-e.toml`
- Create: `tests/fixtures/native/build-native-fixtures.sh`
- Create: `tests/fixtures/native/output/...`
- Modify: `tests/integration/remi/config.toml`
- Modify: `README.md`

- [ ] **Step 1: Choose one portable positive-path strategy**

Prefer one of:
- generated native fixtures consumed by `phase4-group-e.toml`
- or promoting the existing native harness into a stable, documented automated check

Stay with one path; do not duplicate coverage systems unless strictly necessary.

- [ ] **Step 2: Add positive local-install tests**

Prove on distro-appropriate packages that:
- local RPM install works on Fedora-based coverage
- local DEB install works on Ubuntu-based coverage
- local Arch package install works on Arch-based coverage

- [ ] **Step 3: Revisit takeover**

Attempt to add a reliable positive takeover/adoption path. If a whole-system takeover success case remains too brittle, narrow the active docs so takeover is explicitly alpha/best-effort and make adoption the positively proven path.

- [ ] **Step 4: Run verification**

Run:
- `cargo test --features server --test workflow`
- `cargo test --features server`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add tests/workflow.rs tests/scriptlet_harness/test_scriptlets.py tests/integration/remi/manifests/phase4-group-e.toml tests/fixtures/native tests/integration/remi/config.toml README.md
git commit -m "test(native): prove local package installs and clarify takeover"
```

## Chunk 3: Proof Cleanup, Docs, and Final Verification

### Task 8: Resolve the Remaining Proof-Oriented Partial Rows

**Files:**
- Modify: `src/commands/federation.rs`
- Modify: `src/commands/provenance.rs`
- Modify: `tests/integration/remi/manifests/phase4-group-d.toml`
- Modify: `docs/FEATURE-AUDIT-2026-03-28.md`
- Modify: `docs/INTEGRATION-TESTING.md`

- [ ] **Step 1: Decide per feature whether to prove more or narrow the claim**

Handle each remaining partial row explicitly:
- `trust`
- `automation`
- `provenance diff`
- `federation`

- [ ] **Step 2: Add positive tests where the implementation is already good enough**

Focus on the smallest test additions that prove the happy path instead of more smoke coverage.

- [ ] **Step 3: Narrow docs where the implementation is intentionally limited**

If a surface remains best-effort or admin-only, say so directly in active docs and in the audit row.

- [ ] **Step 4: Run verification**

Run:
- `cargo test --features server`
- `cargo run -p conary-test -- list`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/commands/federation.rs src/commands/provenance.rs tests/integration/remi/manifests/phase4-group-d.toml docs/FEATURE-AUDIT-2026-03-28.md docs/INTEGRATION-TESTING.md
git commit -m "test(feature-audit): resolve remaining proof gaps"
```

### Task 9: Final Active-Doc Alignment

**Files:**
- Modify: `README.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/modules/bootstrap.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/federation.md`
- Modify: `CLAUDE.md`
- Modify: `AGENTS.md`
- Modify: `docs/FEATURE-AUDIT-2026-03-28.md`

- [ ] **Step 1: Update command examples to current canonical names**

Touch every active doc that still mentions:
- bootstrap legacy names
- `update-group`
- capability `enforce`

- [ ] **Step 2: Rewrite feature maturity wording to match the final implementation**

Remove stale “experimental” tags where the new work made the feature real, and add explicit limits where a feature remains intentionally narrow.

- [ ] **Step 3: Refresh the audit rows**

Reclassify each active row so `doc drift` is gone and any remaining `partial`/`untested` statuses are deliberate and justified.

- [ ] **Step 4: Run doc sanity checks**

Run:
- `git diff --check`
- `rg -n "stage0|stage1|stage2|update-group|capability enforce" README.md docs CLAUDE.md AGENTS.md`

Expected: only historical/archive references remain

- [ ] **Step 5: Commit**

```bash
git add README.md docs/conaryopedia-v2.md docs/INTEGRATION-TESTING.md docs/modules/bootstrap.md docs/modules/ccs.md docs/modules/federation.md CLAUDE.md AGENTS.md docs/FEATURE-AUDIT-2026-03-28.md
git commit -m "docs: align active feature docs with implemented surface"
```

### Task 10: Full Verification and Branch Cleanup

**Files:**
- Modify: any files needed to fix final lint/test fallout

- [ ] **Step 1: Run the full test suite**

Run: `cargo test --features server`
Expected: PASS

- [ ] **Step 2: Run the full lint pass**

Run: `cargo clippy --features server -- -D warnings`
Expected: PASS

- [ ] **Step 3: Re-run doc and manifest sanity checks**

Run:
- `git diff --check`
- `cargo run -p conary-test -- list`

Expected: PASS

- [ ] **Step 4: Summarize residual intentional limitations**

If any feature still cannot be claimed as fully mature, make sure the final audit/doc text says so plainly before merge.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: finalize feature claims completion pass"
```
