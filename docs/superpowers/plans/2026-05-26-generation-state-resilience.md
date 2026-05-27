# Generation State Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Conary recover manager state from SQLite-native backups when the live SQLite database is damaged or missing.

**Architecture:** Before live preview mutations, write a rotational SQLite backup of the current manager DB. At generation publication time, write a SQLite-native backup next to the generation artifact and fsync it with the same durability expectations as `/conary/current`. Keep only a small manifest as JSON; database contents stay in SQLite format to avoid schema drift.

**Tech Stack:** Rust, rusqlite online backup or `VACUUM INTO`, gzip compression if available in the chosen helper, small serde manifest, generation publication model, existing durable filesystem helpers, focused temp-root tests.

---

## Scope

This plan implements Plan D from
`docs/superpowers/specs/2026-05-26-limited-preview-release-hardening-design.md`.
It builds on the existing generation publication durability work. It does not
replace SQLite as the live authority and does not make arbitrary historical
package repositories reconstructible from generation artifacts alone.

This plan deliberately rejects a custom JSON mirror of troves/files/
publications as the primary recovery format. SQLite already owns schema,
indices, constraints, and migrations; backups should preserve that fidelity.

## File Structure

- Create `crates/conary-core/src/db/backup.rs`: SQLite backup writer,
  verifier, compression, rotation, and restore helpers.
- Modify `crates/conary-core/src/db/mod.rs`: export backup module.
- Modify adoption/unadoption and other live preview mutation paths to write a
  pre-mutation backup before changing the live Conary DB.
- Modify `apps/conary/src/commands/generation/publication.rs`: write a
  generation-bound SQLite backup after a generation is successfully built and
  before publication is marked complete.
- Modify `apps/conary/src/commands/generation/commands.rs`: add
  `system generation verify-db-backup` and `system generation recover-db`.
- Modify `apps/conary/src/cli/generation.rs`: add CLI flags for dry-run and
  explicit apply.
- Add tests under `crates/conary-core/tests/` or module-local tests using a
  temporary generation directory and SQLite DB.
- Modify `docs/ARCHITECTURE.md`, `docs/conaryopedia-v2.md`, and
  `README.md`: document the recovery boundary.
- Modify docs-audit inventory and ledger files.

## Review-Tightened Decisions

- The live SQLite DB remains authoritative during normal operation.
- Backups are recovery metadata, not a second mutable source of truth.
- A rotational pre-mutation backup must land before the first tester wave so
  adoption-only users can recover the manager DB before any generation exists.
- Generation-bound backups should use SQLite's online backup API or
  `VACUUM INTO` rather than hand-serialized JSON tables.
- Restore must default to dry-run verification against a copied DB.
- Applying a recovered backup to the live DB requires an explicit flag and a
  backup of the damaged DB.
- Backup verification must fail closed if generation metadata, publication
  debt, or `/conary/current` selection disagree.

---

### Task 0: Rotational Pre-Mutation DB Backups

**Files:**
- Create: `crates/conary-core/src/db/backup.rs`
- Modify: `crates/conary-core/src/db/mod.rs`
- Modify: `apps/conary/src/commands/adopt/system.rs`
- Modify: `apps/conary/src/commands/adopt/packages.rs`
- Modify: `apps/conary/src/commands/adopt/unadopt.rs`
- Modify: other live preview mutation entrypoints after the first adoption slice
- Test: focused DB backup tests and adoption/unadoption command tests

- [ ] **Step 1: Add a SQLite backup helper**

Create a helper that writes a consistent copy of the DB using SQLite's online
backup API or `VACUUM INTO`. The first version should produce:

```text
/var/lib/conary/backups/conary.db.<timestamp>.bak
```

If compression is added in the first slice, use:

```text
/var/lib/conary/backups/conary.db.<timestamp>.bak.gz
```

- [ ] **Step 2: Add rotation**

Keep the most recent backups by count and age:

```text
default count: 5
default max age: 14 days
```

- [ ] **Step 3: Call the helper before adoption and unadoption apply paths**

Before any adoption/unadoption apply path mutates the live Conary DB, write a
backup. Dry-runs must not create backups.

- [ ] **Step 4: Add verification**

After writing a backup, open the copied DB and run:

```text
PRAGMA integrity_check;
PRAGMA schema_version;
```

Run:

```bash
cargo test -p conary-core db::backup
cargo test -p conary --lib adopt
```

### Task 1: Generation-Bound SQLite Backup Manifest

**Files:**
- Modify: `crates/conary-core/src/db/backup.rs`
- Modify: `apps/conary/src/commands/generation/publication.rs`

- [ ] **Step 1: Define the manifest envelope**

The manifest is metadata only; it does not mirror database tables:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationDbBackupManifest {
    pub format: String,
    pub manifest_version: u32,
    pub db_schema_version: i32,
    pub generation_number: i64,
    pub state_number: i64,
    pub created_at: String,
    pub backup_file: String,
    pub compression: Option<String>,
    pub backup_sha256: String,
    pub sqlite_page_count: Option<i64>,
}
```

Use `format = "conary.generation-db-backup.v1"`.

- [ ] **Step 2: Add path convention**

For generation directory `/conary/generations/<n>`, write:

```text
/conary/generations/<n>/state/conary.db.backup
/conary/generations/<n>/state/conary.db.backup.sha256
/conary/generations/<n>/state/conary-db-backup.json
```

If compression lands in the first slice, the backup file may be
`conary.db.backup.gz`, and the manifest must name that compression.

- [ ] **Step 3: Add module tests**

Add tests proving manifest JSON round-trips and checksum verification fails
when the backup file changes.

Run:

```bash
cargo test -p conary-core db::backup
```

Expected: tests pass.

### Task 2: Write Backup During Publication

**Files:**
- Modify: `apps/conary/src/commands/generation/publication.rs`
- Modify: `crates/conary-core/src/db/backup.rs`
- Modify: `crates/conary-core/src/filesystem/durable.rs`

- [ ] **Step 1: Use the SQLite backup helper after generation build**

Write the generation-bound backup after the generation artifact exists and
before publication is marked complete.

- [ ] **Step 2: Use durable write helpers**

Write to a temporary file, fsync the file, rename into place, and fsync the
parent directory.

- [ ] **Step 3: Fail publication if backup write fails**

If backup writing fails after DB package mutation committed, record
generation publication debt rather than marking publication complete.

### Task 3: Verify Backup Integrity

**Files:**
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`
- Modify: `crates/conary-core/src/db/backup.rs`

- [ ] **Step 1: Add CLI command**

Add:

```bash
conary system generation verify-db-backup --generation 3
conary system generation verify-db-backup --current
```

- [ ] **Step 2: Verification checks**

Verification must check:

```text
backup and manifest files exist
format is conary.generation-db-backup.v1
db_schema_version is supported
checksum matches
SQLite integrity_check passes on a copied backup
generation_number matches directory/metadata
generation artifacts have not been garbage-collected
selected /conary/current agrees when --current is used
```

- [ ] **Step 3: Add tests**

Use tempdirs to create valid and corrupt backups. Assert corrupt checksums,
bad manifests, and missing generation artifacts fail with clear errors.

### Task 4: Dry-Run DB Recovery

**Files:**
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`
- Modify: `crates/conary-core/src/db/backup.rs`

- [ ] **Step 1: Add dry-run command**

Add:

```bash
conary system generation recover-db --generation 3 --dry-run
```

The command copies or decompresses the generation-bound backup into a temporary
SQLite DB, runs integrity checks, checks schema compatibility, prints the
target backup/apply plan, and deletes the temporary DB unless `--keep-temp` is
supplied.

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
SQLite-native backups recover Conary manager visibility for packages and
generations represented by the backed-up DB. They do not recover missing
package payloads, private keys, remote repository history, or native
package-manager transaction history.
```

- [ ] **Step 2: Add operator commands**

Document:

```bash
conary system generation verify-db-backup --current
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
cargo test -p conary-core db::backup
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
