# Generation File Capability Xattr Propagation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let generation-aware CCS installs preserve `[[file_capabilities]]` as `security.capability` xattrs in generation images before removing the current fail-closed refusal.

**Architecture:** Persist installed CCS file-capability authority in the database, convert that persisted authority into generation runtime input xattrs, encode those xattrs into EROFS file stats, and surface generation metadata/inspection evidence. Only after that path exists may generation-aware installs accept file capabilities, and deferred generation publication with file capabilities remains refused until there is a generated artifact carrying the xattr authority.

**Tech Stack:** Rust, rusqlite migrations/models, Linux file capability xattr encoding, composefs EROFS builder, CCS install transaction tests, generation builder/export tests, conary-test QEMU generation gates, docs-audit checks.

## Global Constraints

- Keep raw legacy scriptlet replay out of generation authority.
- Keep `apps/conary/src/commands/install/transaction.rs` fail-closed for generation-aware installs with nonempty `ccs_file_capabilities` until this plan's persistence, runtime-input, EROFS, and verification tasks are complete.
- Do not depend on mutable-live-root `setcap` for generation-aware installs; generation authority must come from persisted metadata and the generated image.
- Preserve existing mutable live-root behavior: CCS file capabilities are still applied with the controlled `setcap` invocation after selected file deployment and before DB commit.
- Reject generation-aware installs with file capabilities when `--defer-generation` is requested; accepting the package before a generation artifact exists would make the authority unverifiable.
- Reject or fail generation build if a persisted file-capability target is absent, excluded from the generation root, non-regular, or cannot be encoded as a supported Linux `security.capability` xattr.
- Store only manifest-validated Linux file capability names and flags. Inheritable file capabilities remain unsupported because manifest validation already rejects them.
- Keep public-ready policy separate from generation preservation. High-risk known capabilities can be preserved in private-review/local installs, but Workstream A still controls public Remi serving.
- Do not merge this implementation independently of Workstream A. The branch must include the file-capability public-policy allowlist slice before any generation-aware file-capability install acceptance reaches the main integration branch.
- Maintain backward compatibility for older generation metadata by making any new inspection fields optional/defaulted.
- Run the generation interaction gate before removing the fail-closed install refusal in a shipped branch.

---

## File Structure

- Create `crates/conary-core/src/db/models/installed_file_capability.rs`
  - Own CRUD helpers for persisted installed CCS file-capability authority.
- Modify `crates/conary-core/src/db/models/mod.rs`
  - Export the new model.
- Modify `crates/conary-core/src/db/schema.rs`
  - Bump `SCHEMA_VERSION` from `76` to `77`, route migration `77`, and add the `apply_migration` dispatch arm.
- Modify `crates/conary-core/src/db/migrations/v41_current.rs`
  - Add `installed_file_capabilities`.
- Modify `apps/conary/src/commands/install/inner.rs`
  - Persist selected installed CCS file capability rows with the package file rows.
- Modify `apps/conary/src/commands/install/transaction.rs`
  - Replace the unconditional generation-aware refusal with a staged preflight that only allows non-deferred generation-aware file-capability installs after xattr preservation exists.
- Create `crates/conary-core/src/generation/builder/file_capabilities.rs`
  - Encode CCS `FileCapability`/installed authority into Linux `security.capability` xattr bytes.
- Modify `crates/conary-core/src/generation/builder.rs`
  - Register/re-export the new helper module as needed.
- Modify `crates/conary-core/src/generation/builder/runtime_inputs.rs`
  - Load persisted capability rows for current generation inputs and attach xattrs to matching regular file refs.
- Modify `crates/conary-core/src/generation/builder/erofs.rs`
  - Add xattrs to `FileEntryRef` and to file stats when building EROFS images.
- Modify `crates/conary-core/src/generation/builder/create.rs`
  - Use the DB-aware runtime-input collector and carry xattr counts into build/metadata results.
- Modify `crates/conary-core/src/generation/builder/rebuild.rs`
  - Update the DB-aware runtime-input collector call if the rebuild path collects generation inputs.
- Modify existing `FileEntryRef` literal call sites as needed:
  - `crates/conary-core/src/bootstrap/image.rs`
  - `apps/conary/src/commands/bootstrap/seed.rs`
  - `crates/conary-core/src/generation/delta.rs`
  - `crates/conary-core/src/derivation/compose.rs`
  - `crates/conary-core/src/generation/builder/boot_assets.rs`
  - `crates/conary-core/src/generation/builder/root_validation.rs`
  - `crates/conary-core/benches/erofs_build.rs`
  - existing tests in `crates/conary-core/src/generation/builder/erofs.rs` and `runtime_inputs.rs`
- Modify `crates/conary-core/src/generation/metadata.rs`
  - Add optional/defaulted generation metadata fields for security capability xattr counts.
- Modify `crates/conary-core/src/generation/artifact.rs`
  - Preserve/load the new optional metadata fields through artifact load checks without changing artifact manifest hashes unexpectedly except through normal metadata digest updates.
- Modify `apps/conary/src/commands/generation/commands.rs`
  - Surface generation inspection output for expected file-capability xattr authority.
- Modify `apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml`
  - Add or extend a QEMU proof for generated `security.capability` xattr activation.
- Modify `apps/conary/tests/integration/remi/manifests/phase3-group-p-iso-export.toml` if ISO carrier proof needs an explicit check.
- Modify `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, `docs/ARCHITECTURE.md`, `docs/INTEGRATION-TESTING.md`, and `docs/operations/post-generation-export-follow-up-roadmap.md`
  - Replace "generation file capabilities are unsupported" claims with the landed preservation boundary and proof status.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Register this implementation plan and any public claim updates.
- Regenerate `docs/superpowers/documentation-accuracy-audit-inventory.tsv`.

## Task 1: Persist Installed File Capability Authority

**Files:**
- Create: `crates/conary-core/src/db/models/installed_file_capability.rs`
- Modify: `crates/conary-core/src/db/models/mod.rs`
- Modify: `crates/conary-core/src/db/schema.rs`
- Modify: `crates/conary-core/src/db/migrations/v41_current.rs`

**Interfaces:**
- Produces: `InstalledFileCapability`
- Produces: `InstalledFileCapability::replace_for_trove(conn, trove_id, capabilities)`
- Produces: `InstalledFileCapability::find_all_ordered(conn)`
- Produces: `InstalledFileCapability::find_by_trove(conn, trove_id)`

- [ ] **Step 1: Write failing migration/model tests**

Add tests proving migration `77` creates `installed_file_capabilities`, rejects unknown troves through foreign keys, cascades on trove deletion, and round-trips a row for `/usr/bin/server` with `["cap_net_bind_service"]`, `permitted = true`, `effective = true`, `inheritable = false`.

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core installed_file_capability
cargo test -p conary-core migrate_v77
```

Expected: FAIL because the table/model do not exist yet.

- [ ] **Step 3: Implement schema and model**

Add a table shaped like:

```sql
CREATE TABLE installed_file_capabilities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    capabilities_json TEXT NOT NULL,
    permitted INTEGER NOT NULL DEFAULT 1,
    effective INTEGER NOT NULL DEFAULT 1,
    inheritable INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(trove_id, path)
);
CREATE INDEX idx_installed_file_capabilities_trove ON installed_file_capabilities(trove_id);
CREATE INDEX idx_installed_file_capabilities_path ON installed_file_capabilities(path);
```

Add `77 => migrations::migrate_v77(conn)` to `apply_migration` in `crates/conary-core/src/db/schema.rs`.

The model must deserialize `capabilities_json` as a sorted, deduplicated nonempty string list and validate by reusing `conary_core::ccs::manifest::FileCapability::validate()`. `InstalledFileCapability::replace_for_trove` must delete existing rows for the trove before inserting the normalized set, or use an equivalent `INSERT OR REPLACE` strategy, so repeated installs/upgrades do not trip the `UNIQUE(trove_id, path)` constraint.

`capabilities_json` is acceptable for this slice because generation input loading needs per-trove/per-path rows, not capability-name SQL queries. A future reporting slice can normalize capability names into child rows if SQL-level capability queries become product behavior.

- [ ] **Step 4: Verify the model tests pass**

Run:

```bash
cargo test -p conary-core installed_file_capability
cargo test -p conary-core migrate_v77
```

Expected: PASS.

- [ ] **Step 5: Commit Task 1**

Commit message:

```bash
git commit -m "security: persist installed file capability authority"
```

## Task 2: Persist Selected CCS File Capabilities During Install

**Files:**
- Modify: `apps/conary/src/commands/install/inner.rs`
- Modify: `apps/conary/src/commands/install/transaction.rs` only if helper placement needs access to `TransactionContext`

**Interfaces:**
- Consumes: `TransactionContext::ccs_file_capabilities`
- Consumes: installed file metadata from `install_inner_with_stored_files`
- Produces: persisted installed capability rows only for package payload paths installed in the transaction

- [ ] **Step 1: Write failing install persistence tests**

Add tests showing a CCS install with `ccs_file_capabilities = ["/usr/bin/server"]` persists exactly one installed capability row when `/usr/bin/server` is in `stored_files`, and persists none for a manifest capability whose path was not selected/installed.

Also add an upgrade test showing the old trove's installed capability row is removed by cascade and the new trove owns the current row.

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary --lib installed_file_capability
cargo test -p conary --lib install_inner
```

Expected: FAIL until persistence is wired into install.

- [ ] **Step 3: Implement install persistence**

After file rows are inserted and `installed_file_metadata` is known, persist normalized file-capability rows for `ctx.ccs_file_capabilities` whose paths appear in the installed payload map. Use the same selected-path boundary as mutable-live-root application so DB authority and live-root `setcap` authority agree.

Do not apply `setcap` in generation-aware installs. This task only records authority.

- [ ] **Step 4: Verify install persistence**

Run:

```bash
cargo test -p conary --lib installed_file_capability
cargo test -p conary --lib install_inner
```

Expected: PASS.

- [ ] **Step 5: Commit Task 2**

Commit message:

```bash
git commit -m "security: record ccs file capabilities during install"
```

## Task 3: Encode `security.capability` Runtime Inputs

**Files:**
- Create: `crates/conary-core/src/generation/builder/file_capabilities.rs`
- Modify: `crates/conary-core/src/generation/builder.rs`
- Modify: `crates/conary-core/src/generation/builder/runtime_inputs.rs`
- Modify: `crates/conary-core/src/generation/builder/create.rs`
- Modify: `crates/conary-core/src/generation/builder/rebuild.rs` if it calls the runtime-input collector
- Modify: all existing `FileEntryRef` literal call sites listed in File Structure

**Interfaces:**
- Produces: `SECURITY_CAPABILITY_XATTR: &str = "security.capability"`
- Produces: `encode_security_capability_xattr(...) -> crate::Result<Vec<u8>>`
- Produces: `FileEntryRef.xattrs: BTreeMap<String, Vec<u8>>`
- Produces: `collect_runtime_generation_inputs(conn: &rusqlite::Connection, troves: &[Trove], files: Vec<FileEntry>)`

- [ ] **Step 1: Write failing xattr encoder tests**

Add tests for:

- `LINUX_FILE_CAPABILITY_NAMES` has `cap_net_bind_service` at index 10 and `cap_bpf` at index 39, matching Linux `CAP_*` numbering;
- `cap_net_bind_service` sets bit 10 in the permitted set and sets the effective flag;
- a high-bit capability such as `cap_bpf` lands in the second capability word;
- `permitted = false` with `effective = true` is rejected by manifest validation;
- inheritable rows remain rejected;
- duplicate capability names are normalized before encoding.

Use Linux vfs capability xattr bytes directly instead of invoking `setcap`; tests must not require root or host xattr support.

- [ ] **Step 2: Write failing runtime-input tests**

Add tests showing:

- a persisted capability row on a generation-eligible regular file attaches `security.capability` to that `FileEntryRef`;
- capability rows for missing paths, excluded paths, symlinks, directories, or non-generation sources fail closed with package/path context;
- generation inputs without capability rows remain byte-for-byte compatible at the public struct level except for an empty `xattrs` map.

- [ ] **Step 3: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core generation::builder::file_capabilities
cargo test -p conary-core generation::builder::runtime_inputs
```

Expected: FAIL until encoder/runtime-input wiring exists.

- [ ] **Step 4: Implement encoder and runtime-input wiring**

Implement the encoder from the ordered `LINUX_FILE_CAPABILITY_NAMES` table so capability bit positions match Linux `CAP_*` numbering. Emit revision-2 vfs capability data with no namespace rootid unless the repo already has a stronger local reason to choose revision 3. Serialize `magic_etc` and all capability bitmap words with explicit little-endian byte order (`to_le_bytes()`), never native-endian serialization. Keep the implementation deterministic.

Add `xattrs: BTreeMap<String, Vec<u8>>` to `FileEntryRef` with default-empty construction in every existing literal call site. Prefer a small constructor/helper for new test fixtures if that keeps the churn tidy, but every existing literal must compile explicitly.

Change `collect_runtime_generation_inputs` to accept `conn: &rusqlite::Connection` and update `create.rs`, `rebuild.rs` if applicable, and unit tests to pass a connection. The collector should load installed capability rows, attach encoded xattrs to matching regular file refs, and error if any persisted row cannot be represented in the generation root.

Before attaching xattrs, iterate persisted capability rows and fail immediately if a path is excluded by generation `is_excluded`, belongs to a non-regular file type in the generation root, belongs to a non-generation source, or has no matching installed file entry. Include package name and path in the error.

- [ ] **Step 5: Verify encoder/runtime-input tests**

Run:

```bash
cargo test -p conary-core generation::builder::file_capabilities
cargo test -p conary-core generation::builder::runtime_inputs
```

Expected: PASS.

- [ ] **Step 6: Commit Task 3**

Commit message:

```bash
git commit -m "security: feed file capabilities into generation inputs"
```

## Task 4: Preserve Xattrs In EROFS And Inspection Metadata

**Files:**
- Modify: `crates/conary-core/src/generation/builder/erofs.rs`
- Modify: `crates/conary-core/src/generation/builder/create.rs`
- Modify: `crates/conary-core/src/generation/metadata.rs`
- Modify: `crates/conary-core/src/generation/artifact.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`

**Interfaces:**
- Consumes: `FileEntryRef.xattrs`
- Produces: EROFS regular-file stat xattrs
- Produces: optional/defaulted generation metadata count for `security.capability` xattrs
- Produces: generation inspection output line `  Cap xattrs: {count}` when the count is nonzero

- [ ] **Step 1: Write failing EROFS/stat tests**

Add tests proving the builder passes `FileEntryRef.xattrs` into the `composefs::tree::Stat` for regular files. If the `composefs` dependency exposes a suitable inspection API, assert the built file inode carries the exact expected xattr bytes. If it does not, assert deterministic EROFS output changes when the same file gains a `security.capability` xattr and rely on the Task 6 QEMU activation proof for end-to-end inode validation.

For `#[cfg(not(feature = "composefs-rs"))]`, add a support predicate test that reports xattr image support as unavailable.

- [ ] **Step 2: Write failing metadata/inspection tests**

Add tests showing new generation metadata can report `security_capability_xattr_count`, older metadata without the field still deserializes, and `conary system generation info` includes a concise count when the field is nonzero.

- [ ] **Step 3: Run the focused failing tests**

Run:

```bash
cargo test -p conary-core generation::builder::erofs
cargo test -p conary-core generation::metadata
cargo test -p conary --lib generation::commands
```

Expected: FAIL until xattrs and metadata are wired.

- [ ] **Step 4: Implement EROFS xattr preservation**

Set regular-file `Stat.xattrs` from `FileEntryRef.xattrs`. Keep directory and symlink xattrs empty in this slice.

Carry the `security.capability` xattr count directly from `RuntimeGenerationInputs` into `GenerationMetadata` in `create.rs`; do not add a `BuildResult` field unless implementation reality makes that simpler. Avoid changing artifact manifest schema unless artifact load needs to validate the new metadata digest.

- [ ] **Step 5: Verify EROFS/metadata tests**

Run:

```bash
cargo test -p conary-core generation::builder::erofs
cargo test -p conary-core generation::metadata
cargo test -p conary --lib generation::commands
```

Expected: PASS.

- [ ] **Step 6: Commit Task 4**

Commit message:

```bash
git commit -m "security: preserve file capability xattrs in generations"
```

## Task 5: Allow Non-Deferred Generation-Aware File Capability Installs

**Files:**
- Modify: `apps/conary/src/commands/install/transaction.rs`
- Modify: `apps/conary/src/commands/install/ccs_transaction.rs` if user-facing context needs clearer diagnostics

**Interfaces:**
- Replaces: `reject_unsupported_generation_file_capabilities`
- Produces: `preflight_generation_file_capabilities(ctx) -> Result<()>`

- [ ] **Step 1: Write failing install preflight tests**

Add tests proving:

- generation-aware install with file capabilities is allowed when generation xattr support is compiled in and `defer_generation = false`;
- generation-aware install with file capabilities is rejected when `defer_generation = true`;
- generation-aware install with file capabilities is rejected when xattr image support is unavailable;
- mutable live-root installs are unchanged.

- [ ] **Step 2: Run the focused failing tests**

Run:

```bash
cargo test -p conary --lib generation_file_capabilities
cargo test -p conary --lib reject_unsupported_generation_file_capabilities
```

Expected: FAIL until the preflight changes.

- [ ] **Step 3: Implement staged preflight**

Replace the old unconditional refusal with a preflight that validates:

- nonempty `ccs_file_capabilities` are manifest-valid;
- generation-aware installs with those capabilities are non-deferred;
- the generation builder reports `security.capability` xattr support through a small support predicate backed by `cfg!(feature = "composefs-rs")`;
- mutable live-root installs still skip this generation-only preflight and continue applying `setcap` through `file_capabilities.rs`.

The existing `MutableLiveRoot` branch in `transaction.rs` already prevents the `setcap` applier from running for generation-aware installs. Add a regression test proving the mutable live-root `file_capabilities.rs` path still applies selected capabilities after this preflight changes.

- [ ] **Step 4: Verify install preflight**

Run:

```bash
cargo test -p conary --lib generation_file_capabilities
cargo test -p conary --lib reject_unsupported_generation_file_capabilities
```

Expected: PASS.

- [ ] **Step 5: Commit Task 5**

Commit message:

```bash
git commit -m "security: allow generation file capabilities after xattr preflight"
```

## Task 6: Prove Activation, Rollback, Export, And Docs Truth

**Files:**
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase3-group-p-iso-export.toml` if ISO proof is updated
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/operations/post-generation-export-follow-up-roadmap.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Regenerate: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`

**Interfaces:**
- Produces: QEMU evidence that a selected generation exposes the expected xattr on the target executable
- Produces: rollback evidence that returning to a prior generation restores the prior file-capability state
- Produces: docs that no longer claim generation-aware file capabilities are impossible once the code proves them

- [ ] **Step 1: Add failing interaction proof**

Add a QEMU scenario that builds at least two generations:

1. a prior generation without a file capability on the fixture executable;
2. a later generation with `cap_net_bind_service=+ep` on the fixture executable.

The proof must boot or switch into the later generation and verify the expected file capability using `getcap -n` when available, falling back to a direct `security.capability` xattr inspection helper only if `getcap` is absent from the fixture image. It must then switch or boot back to the earlier generation and verify the capability is absent.

If the existing carrier flow cannot switch generations in-place, export/boot separate artifacts for the capability-present and capability-absent generations and record that as the rollback-equivalent proof in the implementation report.

- [ ] **Step 1a: Check the doc proof floor before public-claim edits**

Before changing docs, run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
```

Fix any pre-existing failure before adding new claims. If a script is missing in the execution environment, record that as an environmental blocker and do a manual doc review for the same surface.

- [ ] **Step 2: Run focused unit and integration gates**

Run:

```bash
cargo test -p conary-core generation::builder
cargo test -p conary-core generation::export
cargo test -p conary --lib install
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: PASS.

- [ ] **Step 3: Run generation interaction gates**

Run:

```bash
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3
cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3
```

Expected: PASS, or record an environmental blocker only if the fixture image/tooling is unavailable after the code-level proof is green. Do not remove the generation-aware fail-closed behavior from the final branch without either passing interaction evidence or an explicit maintainer decision.

- [ ] **Step 4: Update docs and audit metadata**

Update docs to say generation-aware file capabilities are supported only when:

- installed CCS file-capability authority is persisted;
- generation input collection attaches `security.capability`;
- the generation image format preserves xattrs;
- generation publication is non-deferred;
- inspection output reports the expected xattr count.

Keep public Remi file-capability policy language from the first slice unchanged.

- [ ] **Step 5: Run docs and formatting gates**

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

- [ ] **Step 6: Commit Task 6**

Commit message:

```bash
git commit -m "docs: document generation file capability xattr support"
```

## Final Slice Review

After all tasks are complete, run an agentic review over the full slice:

```bash
scripts/agentic-plan-review.sh docs/superpowers/plans/2026-07-08-generation-file-capability-xattrs-plan.md --review-kind implementation --feature generation --feature install --feature ccs --context docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md --context docs/SCRIPTLET_SECURITY.md --context docs/modules/ccs.md --context docs/INTEGRATION-TESTING.md --context docs/operations/post-generation-export-follow-up-roadmap.md
```

Then request subagent code review over the branch diff from the previous slice tip through the final implementation commit. Patch every Critical and Important finding, rerun focused verification, and record the result in `.superpowers/sdd/progress.md`.
