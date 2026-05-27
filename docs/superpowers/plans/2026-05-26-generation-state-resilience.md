# Generation State Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Conary recover manager state from SQLite-native backups when the live SQLite database is damaged or missing.

**Architecture:** Before live preview DB mutations, write a rotational rollback
backup of the current manager DB; after successful adoption-lane mutations,
write a post-success checkpoint so recovery can restore the known-good state
that testers actually reached. At generation publication time, write a
SQLite-native backup next to the generation artifact and fsync it with the same
durability expectations as `/conary/current`. Keep only a small manifest as
JSON; database contents stay in SQLite format to avoid schema drift.

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
- Modify `apps/conary/src/command_risk.rs`, `apps/conary/src/dispatch.rs`, and
  the adoption/unadoption/native-handoff paths so DB backup coverage is tied to
  the live-mutation inventory instead of scattered command folklore.
- Modify adoption/unadoption and other first-wave live preview mutation paths to
  write pre-mutation and post-success checkpoint backups.
- Modify `apps/conary/src/commands/generation/publication.rs`: write a
  generation-bound SQLite backup after a generation is successfully built and
  before publication is marked complete.
- Modify `apps/conary/src/commands/generation/commands.rs`: add
  `system generation verify-db-backup` and `system generation recover-db`.
- Modify `apps/conary/src/cli/system.rs` and a new or existing system command
  module: add non-generation backup list/verify/recover commands for
  adoption-only recovery.
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
- A rotational pre-mutation backup must land before the first tester wave as a
  rollback point, and a post-success checkpoint must land with it so
  adoption-only users can recover to the successful adopted/unadopted state
  before any generation exists.
- Backup paths must be derived from `ConaryRuntimeRoot::from_db_path(db_path)`;
  `/var/lib/conary/backups` is only the default live-root spelling.
- Generation-bound backups should use SQLite's online backup API or
  `VACUUM INTO` rather than hand-serialized JSON tables. Do not raw-copy only
  `conary.db`; WAL-mode sidecars must be handled by SQLite or explicitly
  quarantined during restore.
- Restore must default to dry-run verification against a copied DB.
- Applying a recovered backup to the live DB requires an explicit flag and a
  backup of the damaged DB plus its WAL/SHM sidecars.
- Backup verification must fail closed if generation metadata, publication
  debt, or `/conary/current` selection disagree.
- `verify` and `recover --dry-run` must not require a healthy live DB; they
  verify the selected backup copy first, then inspect the live DB only for apply
  planning.

---

### Task 0: Live-Mutation Backup Scope Inventory

**Files:**
- Read: `apps/conary/src/command_risk.rs`
- Read: `apps/conary/src/dispatch.rs`
- Read: `apps/conaryd/src/`
- Modify: this plan if the inventory changes the first-wave scope

- [x] **Step 1: Inventory all live DB mutation surfaces**

List every command classified as `DbMutation`, `ActiveHostMutation`, or
`AlwaysLive`, plus conaryd package-job execution paths. Mark each as:

```text
covered before first tester post
excluded from first-wave public docs
VM-only until backup coverage lands
follow-up before widened beta
```

At minimum, the first tester post must make an explicit decision for:

```text
system adopt --system
system adopt <pkg>
system adopt --refresh
system unadopt
system native-handoff
install/remove/update/autoremove
system generation build/publish/switch/rollback/gc
system state revert/rollback
system takeover
conaryd package install/remove/update jobs
```

- [x] **Step 2: Keep docs aligned with uncovered surfaces**

If a mutating surface is not covered by pre/post DB checkpoint backups, it must
be absent from the first public quickstart or clearly marked VM/non-critical
host only.

### Task 1: Adoption-Lane DB Checkpoint Backups

**Files:**
- Create: `crates/conary-core/src/db/backup.rs`
- Modify: `crates/conary-core/src/db/mod.rs`
- Modify: `apps/conary/src/commands/adopt/system.rs`
- Modify: `apps/conary/src/commands/adopt/packages.rs`
- Modify: `apps/conary/src/commands/adopt/refresh.rs`
- Modify: `apps/conary/src/commands/adopt/convert.rs`
- Modify: `apps/conary/src/commands/adopt/unadopt.rs`
- Modify: `apps/conary/src/commands/adopt/native_handoff.rs`
- Modify: system backup CLI/commands chosen for list/verify/recover
- Test: focused DB backup tests and adoption/unadoption command tests

- [x] **Step 1: Add a SQLite backup helper**

Create a helper that writes a consistent copy of the DB using SQLite's online
backup API or `VACUUM INTO`. The first version should produce:

```text
<runtime-root>/backups/conary.db.<timestamp>.<reason>.bak
```

If compression is added in the first slice, use:

```text
<runtime-root>/backups/conary.db.<timestamp>.<reason>.bak.gz
```

Reasons should include at least `pre-mutation` and `post-success`.
If the online backup API is chosen, add the required `rusqlite` feature in
`Cargo.toml`; otherwise prefer `VACUUM INTO`.

- [x] **Step 2: Add rotation**

Keep the most recent verified backups by count and age:

```text
default count: 5
default max age: 14 days
```

Rotation must run only after the new backup has been written durably, reopened,
and verified.

- [x] **Step 3: Use durable write semantics**

Write to a temporary path, fsync the file, rename into place, fsync the parent
directory, and write a checksum/manifest sidecar. Do not prune older backups
until the new backup and manifest pass verification.

- [x] **Step 4: Call the helper before and after first-wave apply paths**

Before adoption/unadoption/native-handoff apply paths mutate the live Conary DB,
write a pre-mutation backup. After the DB transaction and any state snapshot
that defines the successful command outcome, write a post-success checkpoint.
Dry-runs must not create backups.

- [x] **Step 5: Add non-generation recovery commands**

Add commands equivalent to:

```bash
conary system db-backup list
conary system db-backup verify --latest
conary system db-backup recover --latest --dry-run
conary --allow-live-system-mutation system db-backup recover --latest --yes
```

The exact CLI names may change, but adoption-only recovery cannot depend on
`system generation recover-db`.

- [x] **Step 6: Add verification**

After writing a backup, open the copied DB and run:

```text
PRAGMA integrity_check;
conary_core::db::schema::get_schema_version()
```

Compare against supported Conary schema versions, not SQLite's internal
`PRAGMA schema_version` cookie.

Run:

```bash
cargo test -p conary-core db::backup
cargo test -p conary --lib adopt
```

### Task 2: Generation-Bound SQLite Backup Manifest

**Files:**
- Modify: `crates/conary-core/src/db/backup.rs`
- Modify: `apps/conary/src/commands/generation/publication.rs`

- [x] **Step 1: Define the manifest envelope**

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

- [x] **Step 2: Add path convention**

For generation directory `/conary/generations/<n>`, write:

```text
/conary/generations/<n>/state/conary.db.backup
/conary/generations/<n>/state/conary.db.backup.sha256
/conary/generations/<n>/state/conary-db-backup.json
```

If compression lands in the first slice, the backup file may be
`conary.db.backup.gz`, and the manifest must name that compression.

- [x] **Step 3: Add module tests**

Add tests proving manifest JSON round-trips and checksum verification fails
when the backup file changes.

Run:

```bash
cargo test -p conary-core db::backup
```

Expected: tests pass.

### Task 3: Write Backup During Publication

**Files:**
- Modify: `apps/conary/src/commands/generation/publication.rs`
- Modify: `crates/conary-core/src/db/backup.rs`
- Modify: `crates/conary-core/src/filesystem/durable.rs`

- [x] **Step 1: Use the SQLite backup helper at the exact publication boundary**

Write the generation-bound backup after the generation artifact exists,
`publish_generation_link` succeeds, and `mark_generation_state_active` records
the selected generation in the DB, but before
`GenerationPublication::mark_complete_through`.

The backup may therefore contain publication debt in the
`CurrentPublished`/`Running` phase. Recovery must treat that phase as valid only
when the generation artifact, `/conary/current`, manifest, and checksum all
agree, then complete or repair publication debt as part of recovery.

- [x] **Step 2: Use durable write helpers**

Write to a temporary file, fsync the file, rename into place, and fsync the
parent directory.

- [x] **Step 3: Fail publication if backup write fails**

If backup writing fails after DB package mutation committed, record
generation publication debt rather than marking publication complete.

### Task 4: Verify Backup Integrity

**Files:**
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`
- Modify: `crates/conary-core/src/db/backup.rs`

- [x] **Step 1: Add CLI command**

Add:

```bash
conary system generation verify-db-backup --generation 3
conary system generation verify-db-backup --current
```

- [x] **Step 2: Verification checks**

Verification must check:

```text
backup and manifest files exist
format is conary.generation-db-backup.v1
db_schema_version is supported
checksum matches
SQLite integrity_check passes on a copied backup
Conary schema version is supported
generation_number matches directory/metadata
generation artifacts have not been garbage-collected
selected /conary/current agrees when --current is used
publication debt state is complete or a valid CurrentPublished recovery state
```

- [x] **Step 3: Add tests**

Use tempdirs to create valid and corrupt backups. Assert corrupt checksums,
bad manifests, and missing generation artifacts fail with clear errors.

### Task 5: Dry-Run DB Recovery

**Files:**
- Modify: `apps/conary/src/cli/generation.rs`
- Modify: `apps/conary/src/commands/generation/commands.rs`
- Modify: `crates/conary-core/src/db/backup.rs`

- [x] **Step 1: Add dry-run command**

Add:

```bash
conary system generation recover-db --generation 3 --dry-run
```

The command copies or decompresses the generation-bound backup into a temporary
SQLite DB, runs integrity checks, checks schema compatibility, prints the
target backup/apply plan, and deletes the temporary DB unless `--keep-temp` is
supplied.

It must not call the normal live-DB `db::open` path before verifying the backup,
because the live DB may be missing, corrupt, or too damaged to migrate.

- [x] **Step 2: Require explicit apply**

Applying to the live DB must require:

```bash
conary --allow-live-system-mutation system generation recover-db --generation 3 --yes
```

The apply path must first move the damaged DB to:

```text
conary.db.recovery-backup.<timestamp>
conary.db-wal.recovery-backup.<timestamp>
conary.db-shm.recovery-backup.<timestamp>
```

Acquire the command's normal mutation lock before quarantine, restore through a
temporary file, fsync the restored DB and parent directory, and make sure stale
WAL/SHM sidecars cannot be applied to the restored DB.

- [x] **Step 3: Fail closed on disagreement**

If the live DB exists and passes integrity checks, refuse apply unless the user
adds an explicit `--replace-healthy-db` debug flag. Do not add that flag to
normal docs.

### Task 6: Docs And Operator Guidance

**Files:**
- Modify: `README.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `docs/operations/post-generation-export-follow-up-roadmap.md`

- [x] **Step 1: Document the recovery boundary**

Add:

```markdown
SQLite-native backups recover Conary manager visibility for packages and
generations represented by the backed-up DB. They do not recover missing
package payloads, private keys, remote repository history, or native
package-manager transaction history.
```

- [x] **Step 2: Add operator commands**

Document:

```bash
conary system generation verify-db-backup --current
conary system generation recover-db --generation <n> --dry-run
conary --allow-live-system-mutation system generation recover-db --generation <n> --yes
conary system db-backup recover --latest --dry-run
```

### Task 7: Verification

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [x] **Step 1: Run focused tests**

Run:

```bash
cargo test -p conary-core db::backup
cargo test -p conary generation::commands
```

- [x] **Step 2: Run workspace gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected: all pass.
