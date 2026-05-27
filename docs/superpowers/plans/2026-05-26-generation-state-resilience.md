# Generation State Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Conary recover manager state from generation-bound metadata when the live SQLite database is damaged or missing.

**Architecture:** At generation publication time, write a compact state snapshot next to the generation artifact and fsync it with the same durability expectations as `/conary/current`. Add verification and dry-run reconstruction commands that build a temporary SQLite DB from the snapshot before any live replacement is allowed.

**Tech Stack:** Rust, rusqlite, serde JSON or SQL dump, generation publication model, existing durable filesystem helpers, focused temp-root tests.

---

## Scope

This plan implements Plan D from
`docs/superpowers/specs/2026-05-26-limited-preview-release-hardening-design.md`.
It builds on the existing generation publication durability work. It does not
replace SQLite as the live authority and does not make arbitrary historical
package repositories reconstructible from generation artifacts alone.

## File Structure

- Create `crates/conary-core/src/generation/state_snapshot.rs`: snapshot
  schema, writer, verifier, and reconstruction helpers.
- Modify `crates/conary-core/src/generation/mod.rs`: export snapshot module.
- Modify `apps/conary/src/commands/generation/publication.rs`: write snapshot
  after a generation is successfully built and before publication is marked
  complete.
- Modify `apps/conary/src/commands/generation/commands.rs`: add
  `system generation verify-state` and `system generation recover-db`.
- Modify `apps/conary/src/cli/generation.rs`: add CLI flags for dry-run and
  explicit apply.
- Modify `crates/conary-core/src/db/models/*`: expose read queries needed by
  the snapshot writer.
- Add tests under `crates/conary-core/tests/` or module-local tests using a
  temporary generation directory and SQLite DB.
- Modify `docs/ARCHITECTURE.md`, `docs/conaryopedia-v2.md`, and
  `README.md`: document the recovery boundary.
- Modify docs-audit inventory and ledger files.

## Review-Tightened Decisions

- The live SQLite DB remains authoritative during normal operation.
- The generation-bound snapshot is recovery metadata, not a second mutable
  source of truth.
- A minimal forward-compatible marker should land before the first tester wave
  so preview generations can be distinguished from older generations that do
  not carry reconstruction metadata.
- Reconstruction must default to dry-run into a temporary DB.
- Applying reconstructed state to the live DB requires an explicit flag and a
  backup of the damaged DB.
- Snapshot verification must fail closed if generation metadata, publication
  debt, or `/conary/current` selection disagree.

---

### Task 0: Forward-Compatible Snapshot Marker

**Files:**
- Create: `crates/conary-core/src/generation/state_snapshot.rs`
- Modify: `crates/conary-core/src/generation/mod.rs`
- Modify: `apps/conary/src/commands/generation/publication.rs`
- Test: focused generation publication or state-snapshot tests

- [ ] **Step 1: Add the minimal marker type**

Before full reconstruction lands, add a small serializable marker:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationStateMarker {
    pub format: String,
    pub snapshot_version: u32,
    pub generation_number: i64,
    pub state_number: i64,
    pub created_at: String,
    pub reconstruction: String,
}
```

Use:

```text
format = "conary.generation-state.v1"
snapshot_version = 1
reconstruction = "marker-only"
```

- [ ] **Step 2: Write the marker during generation publication**

Write the marker to:

```text
/conary/generations/<n>/state/conary-state.json
```

Use durable write and parent-directory sync helpers. If the marker cannot be
written, warn and record publication metadata, but do not block the first
marker-only slice unless the full snapshot writer has already become required.

- [ ] **Step 3: Add compatibility tests**

Add tests proving:

```text
marker JSON round-trips
marker-only snapshots are recognized as not sufficient for DB reconstruction
full snapshot code can later distinguish marker-only from reconstructible
```

Run:

```bash
cargo test -p conary-core generation::state_snapshot
```

### Task 1: Snapshot Schema

**Files:**
- Create: `crates/conary-core/src/generation/state_snapshot.rs`
- Modify: `crates/conary-core/src/generation/mod.rs`

- [ ] **Step 1: Define the snapshot envelope**

Create a serializable type equivalent to:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationStateSnapshot {
    pub format: String,
    pub schema_version: i32,
    pub generation_number: i64,
    pub state_number: i64,
    pub created_at: String,
    pub troves: Vec<SnapshotTrove>,
    pub files: Vec<SnapshotFile>,
    pub publications: Vec<SnapshotPublication>,
    pub repositories: Vec<SnapshotRepository>,
    pub checksum_sha256: Option<String>,
}
```

Use `format = "conary.generation-state.v1"`.

- [ ] **Step 2: Keep the first schema compact**

Include only the data needed to restore manager visibility:

```text
trove identity and install_source
file path/hash/owner/mode/package links
repository names and URLs
generation/state selection
pending or completed generation_publications rows
```

Do not embed package payload bytes; those remain in CAS/generation artifacts.

- [ ] **Step 3: Add module tests**

Add tests proving JSON round-trip and checksum calculation are deterministic.

Run:

```bash
cargo test -p conary-core generation::state_snapshot
```

Expected: tests pass.

### Task 2: Write Snapshot During Publication

**Files:**
- Modify: `apps/conary/src/commands/generation/publication.rs`
- Modify: `crates/conary-core/src/generation/state_snapshot.rs`
- Modify: `crates/conary-core/src/filesystem/durable.rs`

- [ ] **Step 1: Add snapshot path convention**

For generation directory `/conary/generations/<n>`, write:

```text
/conary/generations/<n>/state/conary-state.json
/conary/generations/<n>/state/conary-state.sha256
```

- [ ] **Step 2: Use durable write helpers**

Write to a temporary file, fsync the file, rename into place, and fsync the
parent directory.

- [ ] **Step 3: Fail publication if snapshot write fails**

If snapshot writing fails after DB package mutation committed, record
generation publication debt rather than marking publication complete.

### Task 3: Verify Snapshot Integrity

**Files:**
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`
- Modify: `crates/conary-core/src/generation/state_snapshot.rs`

- [ ] **Step 1: Add CLI command**

Add:

```bash
conary system generation verify-state --generation 3
conary system generation verify-state --current
```

- [ ] **Step 2: Verification checks**

Verification must check:

```text
snapshot file exists
format is conary.generation-state.v1
schema_version is supported
checksum matches
generation_number matches directory/metadata
referenced CAS hashes exist
generation artifacts have not been garbage-collected
selected /conary/current agrees when --current is used
```

- [ ] **Step 3: Add tests**

Use tempdirs to create valid and corrupt snapshots. Assert corrupt checksums
and missing CAS objects fail with clear errors.

### Task 4: Dry-Run DB Reconstruction

**Files:**
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`
- Modify: `crates/conary-core/src/generation/state_snapshot.rs`

- [ ] **Step 1: Add dry-run command**

Add:

```bash
conary system generation recover-db --generation 3 --dry-run
```

The command creates a temporary SQLite DB, runs migrations, imports the
snapshot, runs integrity checks, prints the target backup/apply plan, and
deletes the temporary DB unless `--keep-temp` is supplied.

- [ ] **Step 2: Require explicit apply**

Applying to the live DB must require:

```bash
conary --allow-live-system-mutation system generation recover-db --generation 3 --yes
```

The apply path must first move the damaged DB to:

```text
conary.db.recovery-backup.<timestamp>
```

- [ ] **Step 3: Fail closed on disagreement**

If the live DB exists and passes integrity checks, refuse apply unless the user
adds an explicit `--replace-healthy-db` debug flag. Do not add that flag to
normal docs.

### Task 5: Docs And Operator Guidance

**Files:**
- Modify: `README.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `docs/operations/post-generation-export-follow-up-roadmap.md`

- [ ] **Step 1: Document the recovery boundary**

Add:

```markdown
Generation state snapshots recover Conary manager visibility for packages and
generations represented by an existing generation artifact. They do not recover
missing package payloads, private keys, remote repository history, or native
package-manager transaction history.
```

- [ ] **Step 2: Add operator commands**

Document:

```bash
conary system generation verify-state --current
conary system generation recover-db --generation <n> --dry-run
conary --allow-live-system-mutation system generation recover-db --generation <n> --yes
```

### Task 6: Verification

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Run focused tests**

Run:

```bash
cargo test -p conary-core generation::state_snapshot
cargo test -p conary generation::commands
```

- [ ] **Step 2: Run workspace gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected: all pass.
