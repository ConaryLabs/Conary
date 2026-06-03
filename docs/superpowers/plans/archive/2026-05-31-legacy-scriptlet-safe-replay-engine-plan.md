# Legacy Scriptlet Safe Replay Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Goal 6 by making the local Conary client consume `CcsManifest.legacy_scriptlets`, fail closed for unsafe bundles, optionally replay explicitly approved same-source legacy entries, and persist accepted bundles locally so remove, upgrade, restore, rollback, and batch paths enforce the same policy after the original CCS archive is gone.

**Architecture:** Add a pure `conary-core` replay planner, a v71 installed-bundle persistence model, a bundle-entry executor adapter, and CLI/options plumbing for explicit replay gates. Integrate the planner before every mutation boundary in install, update, remove, autoremove, CCS install, batch, restore, rollback, model apply, collection, and automation paths. Store the accepted bundle with the installed trove, use that installed bundle as remove/upgrade authority, and keep raw replay strictly lifecycle-scoped.

**Tech Stack:** Rust, Clap, `rusqlite`, `serde`, `toml`, existing `LegacyScriptletBundle`, `CcsManifest`, `ScriptletExecutor`, `ExecutionMode`, `SandboxMode`, `DistroPin`, `Trove`, `Changeset`, CCS install transaction paths, and the existing Conary integration-test harness.

---

## Source Context

Read before implementation:

- `AGENTS.md`
- `docs/superpowers/specs/2026-05-31-legacy-scriptlet-safe-replay-engine-design.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-passive-remi-bundle-embedding-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-publication-gate-design.md`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/ccs/manifest.rs`
- `crates/conary-core/src/scriptlet/mod.rs`
- `crates/conary-core/src/scriptlet/runtime.rs`
- `crates/conary-core/src/repository/distro.rs`
- `crates/conary-core/src/db/schema.rs`
- `crates/conary-core/src/db/migrations/v41_current.rs`
- `crates/conary-core/src/db/models/scriptlet_entry.rs`
- `crates/conary-core/src/db/models/trove.rs`
- `apps/conary/src/cli/mod.rs`
- `apps/conary/src/cli/ccs.rs`
- `apps/conary/src/dispatch.rs`
- `apps/conary/src/live_host_safety.rs`
- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/commands/install/mod.rs`
- `apps/conary/src/commands/install/scriptlets.rs`
- `apps/conary/src/commands/install/inner.rs`
- `apps/conary/src/commands/install/conversion.rs`
- `apps/conary/src/commands/install/batch.rs`
- `apps/conary/src/commands/install/restore.rs`
- `apps/conary/src/commands/collection.rs`
- `apps/conary/src/commands/remove.rs`
- `apps/conary/src/commands/update.rs`
- `apps/conary/src/commands/state.rs`
- `apps/conary/src/commands/system.rs`
- `apps/conary/src/commands/query/scripts.rs`
- `docs/modules/ccs.md`
- `docs/modules/source-selection.md`
- `docs/modules/query.md`

## Scope Rules

- Do not change Remi serving, publication, curation, or review-artifact behavior in this goal.
- Do not expand the adapter registry or translate additional scriptlet commands.
- Do not make Goal 4 conversion emit new `decision = "legacy"` entries unless a separate design says so. Goal 6 tests may use synthetic/manual bundles with `legacy` decisions.
- Do not replay `review`, `blocked`, or unknown decisions under any flag.
- Do not replay raw triggers or file triggers in Goal 6. Refuse them unless the entry is already `replaced`.
- Do not silently ignore unsupported `native_invocation` fields on a `legacy` entry. Refuse before mutation.
- Do not use `scriptlet_fidelity = "legacy-replay"` as the replay indicator. Inspect entry decisions or decision counts.
- Do not treat `replay_enabled` in the installed bundle table as durable permission. Every future lifecycle still needs current CLI/options approval.
- Do not insert flattened `ScriptletEntry` rows for bundle-covered lifecycle entries in bundle-carrying CCS packages. The installed bundle row is the authority.
- Keep native non-CCS package installs on the existing `scriptlets` table path.
- Prefer exact, typed refusals over stringly policy checks.
- Keep mutation boundaries explicit: preflight before hooks, file extraction/deployment, DB writes/deletes, generation publication, state snapshots, or post-commit hooks.

## Current Code Watchpoints

- `apps/conary/src/commands/install/mod.rs::install_ccs_package_transactionally` has an `opts.dry_run` early return after package preparation. Bundle admission must be before that return, and dry-run must report the legacy replay outcome.
- `apps/conary/src/commands/remove.rs::prepare_remove` currently reads flattened `ScriptletEntry` rows. Installed bundle lookup and preflight must happen before that flattened lookup or any pre-remove execution.
- `apps/conary/src/commands/install/inner.rs::install_inner_with_stored_files` deletes old troves during upgrades. Old installed bundles must be loaded, decoded, and planned before `Trove::delete()`.
- `apps/conary/src/commands/update.rs::cmd_update` creates an update summary changeset before invoking installs. Goal 6 uses Strategy A: after update selection and package download/manifest parsing, preflight every selected bundle before creating that changeset row. A refusal must leave zero new update changeset rows.
- The update delta path currently calls `DeltaApplier::apply_delta()`, which stores the reconstructed package in CAS before the normal install pipeline. Goal 6 admission must happen before that CAS mutation too.
- `apps/conary/src/cli/mod.rs` update currently lacks `--no-scripts`. Add it with the legacy replay flags.
- `apps/conary/src/cli/ccs.rs` CCS install currently lacks `--no-scripts`. Prefer adding it for parity.
- `CcsTransactionInstallOptions` has direct struct literals in `apps/conary/src/commands/ccs/install.rs` and `apps/conary/src/commands/install/conversion.rs`.
- `InstallOptions`, `ConvertedCcsInstallOptions`, remove/autoremove options, restore/batch prepared structs, collection/model/automation call sites, and tests have direct literals. Use `rg "InstallOptions \\{" apps/conary crates` and related searches before editing.
- `PreparedPackage` in batch install currently carries flattened scriptlets but not legacy bundle plans. Add explicit plan fields rather than re-reading archive state later.
- Rollback/state/system helpers can mutate installed troves. If they cannot safely accept replay flags in Goal 6, they must fail closed when a required legacy replay would occur.

## File Structure

Create:

- `crates/conary-core/src/ccs/legacy_replay.rs`
  - Pure target normalization, admission, lifecycle selection, replay planning, and typed refusal logic.
- `crates/conary-core/src/db/models/installed_legacy_scriptlet_bundle.rs`
  - Local installed bundle persistence model.
- `apps/conary/src/commands/install/legacy_replay.rs`
  - CLI-side integration helpers for option-to-policy conversion, plan summaries, audit metadata formatting, and runner glue around the core planner.

Modify:

- `crates/conary-core/src/ccs/mod.rs`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/db/schema.rs`
- `crates/conary-core/src/db/migrations/v41_current.rs`
- `crates/conary-core/src/db/models/mod.rs`
- `crates/conary-core/src/repository/distro.rs`
- `crates/conary-core/src/scriptlet/mod.rs`
- `crates/conary-core/src/scriptlet/runtime.rs`
- `apps/conary/src/cli/mod.rs`
- `apps/conary/src/cli/ccs.rs`
- `apps/conary/src/dispatch.rs`
- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/commands/install/mod.rs`
- `apps/conary/src/commands/install/legacy_replay.rs`
- `apps/conary/src/commands/install/inner.rs`
- `apps/conary/src/commands/install/scriptlets.rs`
- `apps/conary/src/commands/install/conversion.rs`
- `apps/conary/src/commands/install/batch.rs`
- `apps/conary/src/commands/install/restore.rs`
- `apps/conary/src/commands/remove.rs`
- `apps/conary/src/commands/update.rs`
- `apps/conary/src/commands/collection.rs`
- `apps/conary/src/commands/model/apply.rs`
- `apps/conary/src/commands/state.rs`
- `apps/conary/src/commands/system.rs`
- `apps/conary/src/commands/automation.rs`
- `apps/conary/src/commands/query/scripts.rs`
- `docs/modules/ccs.md`
- `docs/modules/source-selection.md`
- `docs/modules/query.md`

Test:

- Existing colocated unit tests in each modified module.
- New integration tests under `apps/conary/tests/`, using synthetic CCS fixtures with `legacy_scriptlets`.

## Task 1: Core Replay Target And Planner

**Files:**

- Create: `crates/conary-core/src/ccs/legacy_replay.rs`
- Modify: `crates/conary-core/src/ccs/mod.rs`
- Modify: `crates/conary-core/src/repository/distro.rs`
- Modify: `crates/conary-core/src/ccs/legacy_scriptlets.rs`

- [ ] **Step 1: Write failing target-ID tests**

Add tests near `repository/distro.rs` or in a new `legacy_replay` test module:

```rust
#[test]
fn replay_target_ids_normalize_known_distro_pins() {
    assert_eq!(
        replay_target_from_distro_id("fedora-44", "x86_64").unwrap().to_id(),
        "rpm/fedora/44/x86_64"
    );
    assert_eq!(
        replay_target_from_distro_id("ubuntu-26.04", "x86_64").unwrap().to_id(),
        "deb/ubuntu/26.04/x86_64"
    );
    assert_eq!(
        replay_target_from_distro_id("arch", "x86_64").unwrap().to_id(),
        "arch/arch/rolling/x86_64"
    );
}

#[test]
fn arch_source_release_none_normalizes_to_rolling() {
    let bundle = synthetic_bundle_with_source("arch", None, "x86_64");
    assert_eq!(source_target_from_bundle(&bundle).to_id(), "arch/arch/rolling/x86_64");
}
```

Also test that generic family pins such as `fedora` do not fabricate a release when no trusted matching host release is available, and that unknown target releases do not silently match source-native bundles.

- [ ] **Step 2: Implement target helpers**

Add the owned and borrowed target types in `crates/conary-core/src/repository/distro.rs` or a small repository-owned target module, then import them from `ccs::legacy_replay`. Do not make the repository module depend on `ccs::legacy_replay`; target ID normalization is source-selection policy data, not CCS-specific state.

```rust
pub struct ReplayTarget<'a> {
    pub format: &'a str,
    pub distro: &'a str,
    pub release: &'a str,
    pub arch: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayTargetOwned {
    pub format: String,
    pub distro: String,
    pub release: String,
    pub arch: String,
}
```

Add helpers:

```rust
pub fn replay_target_from_distro_id(distro_id: &str, arch: &str) -> Option<ReplayTargetOwned>;
pub fn replay_target_id(target: &ReplayTarget<'_>) -> String;
pub fn source_target_from_bundle(bundle: &LegacyScriptletBundle) -> ReplayTargetOwned;
```

Rules:

- `fedora-44` -> `rpm/fedora/44/<arch>`
- `ubuntu-26.04` -> `deb/ubuntu/26.04/<arch>`
- `debian-13` -> `deb/debian/13/<arch>`
- `arch` -> `arch/arch/rolling/<arch>`
- `source_distro = "arch"` and `source_release = None` or empty -> `rolling`
- unknown or mismatched releases stay `unknown`; they do not grant source-native compatibility.

If implementation reads host `/etc/os-release` for generic pins, the parser must handle missing files, missing fields, quoted values, and unknown values without panicking. Normalize `ID` and distro-family comparisons to lowercase before matching. Tests should inject host identity fixtures rather than reading the developer machine.

- [ ] **Step 3: Write failing planner admission tests**

Add tests in `crates/conary-core/src/ccs/legacy_replay.rs`:

```rust
#[test]
fn review_blocked_and_unknown_entries_refuse_admission_anywhere_in_bundle() {
    // Put a review entry in a future lifecycle and plan fresh install.
    // Expected: Refused(ReviewEntry), not NativeFree.
}

#[test]
fn future_lifecycle_legacy_entry_is_not_selected_for_current_install() {
    // post-remove legacy entry during fresh install should not require
    // --allow-legacy-replay now, but admission should still pass and the
    // installed bundle later persists for remove enforcement.
}

#[test]
fn selected_legacy_entry_requires_feature_gate() {
    // post-install legacy entry, allow_legacy_replay=false.
    // Expected: Refused(LegacyReplayFeatureDisabled).
}

#[test]
fn no_scripts_refuses_selected_required_legacy_replay() {
    // post-install legacy entry, no_scripts=true.
    // Expected: Refused(NoScriptsWouldSkipRequiredReplay).
}

#[test]
fn replaced_entries_never_schedule_raw_replay() {
    // replaced entries should produce FullyReplaced or NativeFree and
    // raw_replay_required=false.
}

#[test]
fn scriptlet_fidelity_legacy_replay_does_not_override_entry_decisions() {
    // A bundle whose scalar fidelity says "legacy-replay" but whose selected
    // entries are all replaced should produce FullyReplaced, not RequiresReplay.
}

#[test]
fn upgrade_lifecycle_selection_uses_upgrade_slots_and_fallbacks() {
    // UpgradeNewPre selects pre-upgrade when present and falls back to
    // pre-install for RPM/DEB-style packages when pre-upgrade is absent.
}

#[test]
fn old_upgrade_remove_lifecycle_selects_installed_bundle_remove_entries() {
    // UpgradeOldPreRemove selects pre-remove from the old installed bundle.
    // UpgradeOldPostRemove selects post-remove from the in-memory old plan.
}

#[test]
fn rollback_lifecycle_refuses_when_replay_is_unavailable() {
    // RollbackRestore/RollbackRemove paths that cannot safely execute raw
    // replay should return RollbackReplayUnavailable before mutation.
}
```

Include tests for unknown `target_compatibility`, `foreign_replay_policy = "deny"` on foreign target, strict host policy, guarded/permissive host policy behavior, raw trigger refusal, timeout bounds, and ordering conflicts.

All target-resolution tests that depend on host identity must inject a `ReplayTarget` directly or use explicit test fixtures. No core planner test should depend on the developer host's `/etc/os-release`.

- [ ] **Step 4: Implement replay planner API**

Create:

```rust
pub struct LegacyReplayPolicyInput<'a> {
    pub replay_enabled: bool,
    pub foreign_replay_override: bool,
    pub no_scripts: bool,
    pub requested_sandbox_mode: SandboxMode,
    pub host_policy: HostForeignReplayPolicy,
    pub target: ReplayTarget<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostForeignReplayPolicy {
    Strict,
    Guarded,
    Permissive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LegacyReplayLifecycle {
    FreshInstallPre,
    FreshInstallPost,
    UpgradeNewPre,
    UpgradeNewPost,
    UpgradeOldPreRemove,
    UpgradeOldPostRemove,
    RemovePre,
    RemovePost,
    RollbackRestore,
    RollbackRemove,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LegacyReplayPreflight {
    NativeFree,
    FullyReplaced(LegacyReplayPlan),
    RequiresReplay(LegacyReplayPlan),
    Refused(LegacyReplayRefusal),
}
```

`LegacyReplayPlan` must include:

```rust
pub struct LegacyReplayPlan {
    pub target_id: String,
    pub source_target_id: String,
    pub bundle_evidence_digest: Option<String>,
    pub lifecycle_entries: Vec<PlannedLegacyEntry>,
    pub sandbox_floor: SandboxMode,
    pub ccs_hooks_allowed: bool,
    pub raw_replay_required: bool,
}
```

`LegacyReplayRefusalKind` must include at least:

```rust
pub enum LegacyReplayRefusalKind {
    ReviewEntry,
    BlockedEntry,
    UnknownDecision,
    LegacyReplayFeatureDisabled,
    NoScriptsWouldSkipRequiredReplay,
    TargetCompatibilityReviewRequired,
    TargetCompatibilityBlocked,
    TargetMismatch,
    ForeignReplayDeniedByBundle,
    ForeignReplayDeniedByHostPolicy,
    ForeignReplayOverrideRequired,
    SandboxRequirementUnsupported,
    TriggerReplayUnsupported,
    NativeArgsContractUnsupported,
    UnsatisfiedTransactionOrder,
    RollbackReplayUnavailable,
    TimeoutOutOfRange,
    MalformedBundle,
}
```

Expose:

```rust
pub fn plan_legacy_replay(
    bundle: Option<&LegacyScriptletBundle>,
    lifecycle: LegacyReplayLifecycle,
    input: &LegacyReplayPolicyInput<'_>,
) -> anyhow::Result<LegacyReplayPreflight>;
```

`None` means no bundle and returns `NativeFree`.

- [ ] **Step 5: Implement lifecycle selection and ordering**

Implement lifecycle mapping from the design:

- fresh install pre: `pre-install`, `pre-transaction`
- fresh install post: `post-install`, `post-transaction`
- upgrade new pre: `pre-upgrade`, fallback `pre-install`
- upgrade new post: `post-upgrade`, fallback `post-install`
- upgrade old pre-remove: installed old bundle `pre-remove`
- upgrade old post-remove: installed old bundle `post-remove`
- remove pre: installed bundle `pre-remove`
- remove post: installed bundle `post-remove`

If a bundle contains raw trigger/file-trigger entries and they are not `replaced`, refuse with `TriggerReplayUnsupported`.

If a package mixes raw legacy entries and generated CCS hooks in pre-mutation phases and `transaction_order.before`/`after` cannot be safely serialized, refuse with `UnsatisfiedTransactionOrder`. Do not hide this inside the install command.

Use this conservative Goal 6 heuristic:

- if a pre-mutation lifecycle contains at least one `legacy` raw replay entry and at least one `replaced` entry that will execute through a collective CCS hook block, inspect their `transaction_order` constraints;
- if a `legacy` entry asserts it must run after a `replaced` entry, refuse with `UnsatisfiedTransactionOrder`;
- if a `replaced` entry asserts it must run after a `legacy` entry and the collective `HookExecutor` block cannot interleave that individual hook with raw replay, refuse with `UnsatisfiedTransactionOrder`;
- allow the coarse fallback order only when no explicit before/after constraint contradicts raw-first-then-hooks execution.

- [ ] **Step 6: Add body decoding helper tests**

Add tests in `legacy_scriptlets.rs` for a new public helper on `LegacyScriptletEntry`:

- text body returns UTF-8 bytes and matches `body_sha256`
- base64 body decodes to original bytes and matches `body_sha256`
- hash mismatch returns an error
- unknown `body_encoding` returns an error

- [ ] **Step 7: Make `LegacyScriptletEntry::body_bytes()` public**

`crates/conary-core/src/ccs/legacy_scriptlets.rs` already has a private `fn body_bytes(&self)`. Change that existing helper to a public method rather than adding a duplicate:

```rust
impl LegacyScriptletEntry {
    pub fn body_bytes(&self) -> anyhow::Result<Vec<u8>> {
        // Decode text/base64 and verify body_sha256.
    }
}
```

Use the existing bundle validation/hash helpers where practical. Do not duplicate ad hoc SHA formatting.

- [ ] **Step 8: Verify Task 1**

Run:

```text
cargo test -p conary-core legacy_replay
cargo test -p conary-core target
cargo test -p conary-core legacy_scriptlets
cargo fmt --check
```

Expected: planner, target, and body decoding tests pass; no formatting changes are pending.

## Task 2: Installed Bundle Persistence

**Files:**

- Create: `crates/conary-core/src/db/models/installed_legacy_scriptlet_bundle.rs`
- Modify: `crates/conary-core/src/db/models/mod.rs`
- Modify: `crates/conary-core/src/db/schema.rs`
- Modify: `crates/conary-core/src/db/migrations/v41_current.rs`

- [ ] **Step 1: Write failing schema migration tests**

Before writing the migration, verify no other branch or recently merged work already claimed v71:

```text
rg "migrate_v71|SCHEMA_VERSION" crates/conary-core/src/db
```

Add or extend schema tests to prove:

- `SCHEMA_VERSION` migrates from 70 to 71;
- `installed_legacy_scriptlet_bundles` exists after migration;
- the table has `trove_id` unique, `bundle_toml`, target metadata, policy metadata, evidence digest, replay audit fields, and `installed_changeset_id`;
- the FK uses `ON DELETE CASCADE`, but removal code will preload plans before trove deletion.

Do not add a Remi migration. This is client-local installed state.

- [ ] **Step 2: Add migration v71**

In `v41_current.rs`, add:

```sql
CREATE TABLE installed_legacy_scriptlet_bundles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trove_id INTEGER NOT NULL UNIQUE REFERENCES troves(id) ON DELETE CASCADE,
    source_format TEXT NOT NULL,
    source_family TEXT NOT NULL,
    source_distro TEXT,
    source_release TEXT,
    source_arch TEXT,
    source_package TEXT NOT NULL,
    source_version TEXT NOT NULL,
    target_id TEXT NOT NULL,
    target_compatibility TEXT NOT NULL,
    foreign_replay_policy TEXT NOT NULL,
    scriptlet_fidelity TEXT NOT NULL,
    publication_status TEXT NOT NULL,
    evidence_digest TEXT,
    replay_policy TEXT NOT NULL,
    replay_enabled INTEGER NOT NULL DEFAULT 0,
    bundle_toml TEXT NOT NULL,
    installed_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
    installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_installed_legacy_scriptlet_bundles_trove
    ON installed_legacy_scriptlet_bundles(trove_id);

CREATE INDEX idx_installed_legacy_scriptlet_bundles_evidence
    ON installed_legacy_scriptlet_bundles(evidence_digest);
```

Update `SCHEMA_VERSION` to 71 and register `migrate_v71`.

- [ ] **Step 3: Write failing model tests**

Add tests for:

- insert and find by trove;
- insert-or-replace updates existing trove row;
- `bundle()` decodes TOML and validates the bundle;
- scalar `evidence_digest` mismatch with decoded bundle fails;
- malformed `bundle_toml` returns an error, not a panic;
- extra fields at bundle and entry level survive TOML round trip;
- deleting a trove cascades the bundle row in a direct DB test;
- a malformed stored row can be loaded as a model but `bundle()` returns a typed error so remove/upgrade callers can refuse safely.

- [ ] **Step 4: Implement installed bundle model**

Add:

```rust
pub struct InstalledLegacyScriptletBundle {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub source_format: String,
    pub source_family: String,
    pub source_distro: Option<String>,
    pub source_release: Option<String>,
    pub source_arch: Option<String>,
    pub source_package: String,
    pub source_version: String,
    pub target_id: String,
    pub target_compatibility: String,
    pub foreign_replay_policy: String,
    pub scriptlet_fidelity: String,
    pub publication_status: String,
    pub evidence_digest: Option<String>,
    pub replay_policy: String,
    pub replay_enabled: bool,
    pub bundle_toml: String,
    pub installed_changeset_id: Option<i64>,
    pub installed_at: Option<String>,
}
```

Methods:

```rust
impl InstalledLegacyScriptletBundle {
    pub fn new(
        trove_id: i64,
        installed_changeset_id: Option<i64>,
        target_id: String,
        replay_policy: String,
        replay_enabled: bool,
        bundle: &LegacyScriptletBundle,
    ) -> anyhow::Result<Self>;

    pub fn insert_or_replace(&mut self, conn: &rusqlite::Connection) -> anyhow::Result<()>;
    pub fn find_by_trove(conn: &rusqlite::Connection, trove_id: i64) -> anyhow::Result<Option<Self>>;
    pub fn bundle(&self) -> anyhow::Result<LegacyScriptletBundle>;
    pub fn delete_by_trove(conn: &rusqlite::Connection, trove_id: i64) -> anyhow::Result<usize>;
}
```

Implementation notes:

- Validate before insert and after decode.
- Serialize the full bundle to pretty TOML if that is the repo convention.
- Store `scriptlet_fidelity` and `publication_status` from the bundle summary/scalars as-is.
- Do not infer replay need from `scriptlet_fidelity`.
- `replay_enabled` records install-time flag usage only.

- [ ] **Step 5: Re-export model**

Update `db/models/mod.rs` so callers can use:

```rust
use conary_core::db::models::InstalledLegacyScriptletBundle;
```

- [ ] **Step 6: Verify Task 2**

Run:

```text
cargo test -p conary-core installed_legacy_scriptlet_bundle
cargo test -p conary-core schema
cargo fmt --check
```

Expected: v71 schema and model round trips pass.

## Task 3: Legacy Entry Executor Contracts

**Files:**

- Modify: `crates/conary-core/src/scriptlet/mod.rs`
- Modify: `crates/conary-core/src/scriptlet/runtime.rs`
- Modify: `crates/conary-core/src/ccs/legacy_replay.rs`
- Modify: `crates/conary-core/src/ccs/legacy_scriptlets.rs`

- [ ] **Step 1: Write failing native invocation contract tests**

Add tests covering:

- upgrade args contracts derive actual old/new versions from runtime context;
- remove args contracts derive actual removal state/count when supported;
- `raw:<value>` becomes a literal arg only when state-independent;
- malformed contract strings refuse with `NativeArgsContractUnsupported`;
- unsupported runtime values refuse with `NativeArgsContractUnsupported`;
- `native_invocation.stdin = "debconf"`, `"paths"`, or `"unknown"` refuses in Goal 6;
- `native_invocation.chroot = "host-root"` or `"unknown"` refuses in Goal 6;
- bare environment keys refuse unless the runner supplies an explicit runtime value;
- `LD_PRELOAD`, `LD_LIBRARY_PATH`, `BASH_ENV`, `ENV`, `PYTHONPATH`, and `PATH` are rejected;
- executor supplies a fixed safe PATH like `/usr/sbin:/usr/bin:/sbin:/bin`;
- timeout less than 1000 ms or greater than 300000 ms refuses.

- [ ] **Step 2: Define execution input types**

Add:

```rust
pub struct LegacyScriptletExecution<'a> {
    pub entry_id: &'a str,
    pub phase: &'a str,
    pub interpreter: &'a str,
    pub interpreter_args: &'a [String],
    pub body: String,
    pub body_encoding: Option<&'a str>,
    pub native_args: &'a [String],
    pub native_environment: &'a [String],
    pub stdin_contract: Option<&'a str>,
    pub chroot_contract: Option<&'a str>,
    pub timeout_ms: u64,
}
```

If existing lifetime/ownership style favors owned values, use owned fields instead. Do not use `Cow` unless there is a real borrowed path in the code.

Add a runtime context type if existing `ExecutionMode` is not enough:

```rust
pub struct LegacyInvocationRuntime<'a> {
    pub mode: &'a ExecutionMode,
    pub old_version: Option<&'a str>,
    pub new_version: Option<&'a str>,
    pub package_instance_count: Option<u32>,
}
```

- [ ] **Step 3: Implement standalone preflight**

Add:

```rust
impl ScriptletExecutor {
    pub fn preflight_legacy_entry(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        runtime: &LegacyInvocationRuntime<'_>,
    ) -> anyhow::Result<()>;
}
```

Preflight must complete before touching the filesystem. It checks interpreter, interpreter args, native args contracts, environment, stdin contract, chroot contract, timeout, sandbox floor, and protected live-root requirements.

- [ ] **Step 4: Implement native args derivation**

Implement contract parsing:

- `1:old-version=old-version`
- `2:new-version=new-version`
- `1:count=package-instance-count`
- `raw:<value>`

Do not pass contract strings to scripts. If a needed runtime value is absent, refuse.

If existing package-format `ExecutionMode` already knows the native arg convention, use that as the source of truth and validate the bundle contract against it.

- [ ] **Step 5: Implement legacy execution with outcome**

Add:

```rust
impl ScriptletExecutor {
    pub fn execute_legacy_entry_with_outcome(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        runtime: &LegacyInvocationRuntime<'_>,
    ) -> ScriptletOutcome;
}
```

Implementation requirements:

- call preflight first;
- decode body via `LegacyScriptletEntry::body_bytes()`;
- write script body to a temporary file under the same safety conventions as current scriptlets;
- prepend interpreter args before the script path;
- use derived native args after the script path according to existing executor conventions;
- pass fixed safe PATH and allowed environment only;
- use `Stdio::null()` unless an explicitly supported stdin contract exists;
- apply entry timeout;
- reuse existing warning/outcome classification.

If process setup refactoring becomes large, keep the first implementation conservative and local. The key invariant is no silent field dropping.

- [ ] **Step 6: Verify Task 3**

Run:

```text
cargo test -p conary-core scriptlet
cargo test -p conary-core legacy_replay
cargo fmt --check
```

Expected: contract parsing, preflight, and execution tests pass.

## Task 4: CLI And Option Plumbing

**Files:**

- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/cli/ccs.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/commands/ccs/install.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/conversion.rs`
- Modify: `apps/conary/src/commands/install/batch.rs`
- Modify: `apps/conary/src/commands/install/restore.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Modify: `apps/conary/src/commands/collection.rs`
- Modify: `apps/conary/src/commands/model/apply.rs`
- Modify: `apps/conary/src/commands/state.rs`
- Modify: `apps/conary/src/commands/system.rs`
- Modify: `apps/conary/src/commands/automation.rs`

- [ ] **Step 1: Inventory option literals**

Run and save the relevant sites in the task notes:

```text
rg "InstallOptions \\{" apps/conary crates
rg "CcsTransactionInstallOptions \\{" apps/conary crates
rg "ConvertedCcsInstallOptions \\{" apps/conary crates
rg "RemoveOptions \\{" apps/conary crates
rg "pub async fn cmd_remove|pub async fn cmd_autoremove" apps/conary/src/commands/remove.rs
rg "BatchInstaller::new" apps/conary/src
rg "cmd_remove\\(" apps/conary/src
rg "cmd_autoremove\\(" apps/conary/src
rg "Autoremove" apps/conary/src
rg "cmd_install|cmd_update|cmd_remove" apps/conary/src/commands apps/conary/src/dispatch.rs
rg "\\.\\.Default::default\\(\\)" apps/conary/src
```

Known required updates include:

- `apps/conary/src/commands/ccs/install.rs` direct `CcsTransactionInstallOptions` literals;
- `apps/conary/src/commands/install/conversion.rs` converted install literal;
- `apps/conary/src/commands/model/apply.rs` replatform installs;
- collection install/update wrappers;
- automation/state restore wrappers;
- tests that construct these structs directly.

Also inspect any `..Default::default()` option literals so a site does not compile while silently losing the intended disabled-by-default replay policy.

There is no existing `RemoveOptions` struct at the time of writing; `cmd_remove()` and `cmd_autoremove()` use positional arguments. Prefer adding a single `LegacyReplayOptions` parameter rather than more positional bools. If the implementation introduces a `RemoveOptions` struct instead, update every dispatch/model/automation/autoremove call site in the same slice.

- [ ] **Step 2: Write failing CLI parsing tests**

Add Clap/CLI tests proving:

- `conary install` accepts `--allow-legacy-replay` and `--allow-foreign-legacy-replay`;
- `conary update` accepts both replay flags and `--no-scripts`;
- `conary remove` accepts both replay flags;
- `conary autoremove` accepts both replay flags;
- `conary ccs install` accepts both replay flags and `--no-scripts`;
- default parsed options have both replay flags false.

- [ ] **Step 3: Add `LegacyReplayOptions`**

Create an app-level options struct close to current install/remove option types:

```rust
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LegacyReplayOptions {
    pub allow_legacy_replay: bool,
    pub allow_foreign_legacy_replay: bool,
}
```

Thread it into:

- `InstallOptions`;
- `CcsTransactionInstallOptions`;
- converted CCS install options;
- remove options;
- autoremove options;
- batch/restore prepared install options;
- rollback/state restore execution options if they can mutate troves.

Default both flags to false everywhere.

- [ ] **Step 4: Add CLI flags**

Add flags:

```text
--allow-legacy-replay
--allow-foreign-legacy-replay
```

Add `--no-scripts` to update and CCS install.

For `apps/conary/src/cli/mod.rs::Commands::Update`, add the explicit Clap field:

```rust
/// Skip running package scriptlets (install/remove hooks)
#[arg(long)]
pub no_scripts: bool,
```

Thread this field through update dispatch into `cmd_update()` and the `InstallOptions` values that update constructs internally.

Keep help text explicit:

- `--allow-legacy-replay`: allow same-source raw legacy scriptlet replay when the bundle, target, sandbox, and local policy all pass.
- `--allow-foreign-legacy-replay`: additionally allow explicitly compatible foreign raw replay only under permissive host policy.
- `--no-scripts`: suppress hooks where safe, but does not bypass required legacy replay.

- [ ] **Step 5: Propagate options through dispatch and internal callers**

Update:

- install dispatch;
- update dispatch;
- remove/autoremove dispatch;
- CCS install dispatch;
- conversion install path;
- model apply/replatform path;
- batch install;
- restore install;
- collection install and update;
- automation plan execution;
- rollback/state/system paths.

Model apply and automation must default disabled unless the caller explicitly carries the flags.

- [ ] **Step 6: Add host policy resolution helper**

Add a small helper in app code or core to map `DistroPin.mixing_policy`:

- no pin -> `HostForeignReplayPolicy::Strict`
- `strict` -> `Strict`
- `guarded` -> `Guarded`
- `permissive` -> `Permissive`
- unknown -> `Strict`

Add tests proving unknown policy fails closed.

- [ ] **Step 7: Verify Task 4**

Run:

```text
cargo check -p conary
cargo test -p conary cli
cargo fmt --check
```

Expected: option structs compile at every literal site, CLI tests pass.

## Task 5: CCS Install And Fresh Install Integration

**Files:**

- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/legacy_replay.rs`
- Modify: `apps/conary/src/commands/install/scriptlets.rs`
- Modify: `apps/conary/src/commands/install/inner.rs`
- Modify: `apps/conary/src/commands/install/conversion.rs`
- Modify: `apps/conary/src/commands/ccs/install.rs`
- Modify: `crates/conary-core/src/db/models/installed_legacy_scriptlet_bundle.rs`

- [ ] **Step 1: Add legacy plan carrier fields and synthetic CCS fixture helpers**

Before wiring fresh install execution, add the data carriers that later batch/restore paths will reuse. This prevents Task 7 from back-patching core prepared-package type flow.

Add optional fields to `PreparedPackage`, restore prepared structs, or shared prepared-install state as appropriate:

- new bundle pre plan;
- new bundle post plan;
- old installed bundle pre-remove plan if upgrade;
- old installed bundle post-remove plan if upgrade;
- accepted bundle to persist;
- target and policy audit data.

For Task 5 these may be `None` outside the direct CCS install path, but the fields should exist early so batch, restore, and update integration can pass accepted plans forward instead of re-opening CCS archives or re-querying installed bundles after preflight.

Then create test helpers that build CCS packages with:

- no legacy bundle;
- native-free/replaced-only bundle;
- `review` entry;
- `blocked` entry;
- same-source `legacy` post-install entry;
- future-lifecycle `legacy` post-remove entry;
- raw trigger `legacy` entry;
- unsupported native invocation contract.

These should be synthetic fixtures. Do not require the conversion pipeline to emit `legacy` decisions.

- [ ] **Step 2: Write failing fresh install tests**

Add integration tests under `apps/conary/tests/` proving:

- review bundle fails before DB mutation;
- blocked bundle fails before DB mutation;
- unknown decision fails before DB mutation;
- raw trigger legacy entry fails before DB mutation;
- legacy post-install entry without flag fails before DB mutation;
- same-source legacy post-install entry with `--allow-legacy-replay` runs once;
- replaced entry does not raw replay;
- `--no-scripts` with selected legacy entry refuses with `NoScriptsWouldSkipRequiredReplay`;
- `--no-scripts` with replaced-only bundle suppresses CCS hooks as current semantics require;
- `conary ccs install --no-scripts` also suppresses materialized CCS hooks on a bundle-carrying replaced-only package;
- dry-run runs bundle admission before returning and persists no installed bundle row;
- future post-remove legacy entry during install does not execute now, but the bundle is persisted for later.

For mutation assertions, check no new rows in the relevant `troves`, `changesets`, `file_entries`, `scriptlets`, and `installed_legacy_scriptlet_bundles` tables after a refused operation.

- [ ] **Step 3: Insert preflight before dry-run return**

In `install_ccs_package_transactionally()`:

1. parse and validate package as today;
2. load `manifest.legacy_scriptlets.as_ref()`;
3. determine target and host policy;
4. determine fresh install vs upgrade context;
5. call `plan_legacy_replay()` for new bundle pre and post lifecycle groups;
6. if upgrading, load and plan old installed bundle before any old trove deletion;
7. if `opts.dry_run`, include legacy replay decision summary and return with no mutation.

This must be before the existing dry-run early return.

- [ ] **Step 4: Execute planned pre and post legacy entries**

For fresh installs:

- execute pre-mutation legacy entries before CCS pre-hooks;
- run CCS pre-hooks only for materialized replaced behavior and only when `no_scripts` allows hooks;
- execute transaction and persist installed bundle;
- run CCS post-hooks as today when allowed;
- execute post-commit legacy entries from the plan;
- record warnings/outcomes using the existing scriptlet warning style plus bundle entry IDs.

If ordering constraints cannot represent this safely, the planner must refuse before any hook runs.

- [ ] **Step 5: Persist accepted installed bundle with trove**

After the trove ID exists and before the install transaction commits, insert `InstalledLegacyScriptletBundle` for bundle-carrying CCS packages.

Use the existing transaction context rather than inventing a caller-side post-transaction hook:

1. Add `accepted_bundle_to_persist: Option<AcceptedLegacyBundleInstall<'a>>` or equivalent to `TransactionContext<'a>` in `apps/conary/src/commands/install/mod.rs`.
2. Populate that field from the accepted replay preflight state before calling `install_inner_with_stored_files()`.
3. In `apps/conary/src/commands/install/inner.rs::install_inner_with_stored_files()`, immediately after `let trove_id = trove.insert(tx)?;`, create and insert the `InstalledLegacyScriptletBundle` row using the same transaction.
4. Keep enough accepted bundle metadata in the context to build the row without re-reading the CCS archive.

This keeps persistence atomic with the trove and avoids restructuring the inner/outer transaction boundary.

The row should include:

- target ID;
- target compatibility;
- foreign replay policy;
- scriptlet fidelity;
- publication status;
- evidence digest;
- replay policy string;
- whether the install-time replay flag was enabled;
- complete `bundle_toml`;
- installed changeset ID if available.

- [ ] **Step 6: Avoid flattened `ScriptletEntry` double execution**

When a CCS package has `legacy_scriptlets`, do not insert flattened `ScriptletEntry` rows for entries covered by the bundle.

Native non-CCS package installs keep the current flattened path.

Add tests proving `ScriptletEntry::find_by_trove()` does not see duplicate bundle-covered entries after installing a bundle-carrying CCS package.

- [ ] **Step 7: Add audit metadata**

Add compact changeset metadata for bundle-aware operations:

```json
{
  "legacy_scriptlet_replay": {
    "bundle_present": true,
    "target_id": "rpm/fedora/44/x86_64",
    "source_target_id": "rpm/fedora/44/x86_64",
    "target_compatibility": "source-native",
    "foreign_replay_policy": "deny",
    "host_policy": "strict",
    "feature_gate": "enabled",
    "foreign_override": false,
    "evidence_digest": "sha256:...",
    "planned_entries": []
  }
}
```

Include outcome data for executed entries when available:

```json
{"exit_code": 0, "signal": null, "duration_ms": 1234}
```

- [ ] **Step 8: Verify Task 5**

Run:

```text
cargo test -p conary --test bundle_replay
cargo test -p conary ccs_install
cargo test -p conary-core installed_legacy_scriptlet_bundle
cargo fmt --check
```

Use `cargo test -p conary --test bundle_replay` if the tests are placed in `apps/conary/tests/bundle_replay.rs`; use the module filter only for unit tests.

## Task 6: Remove, Upgrade, And Rollback Integration

**Files:**

- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/inner.rs`
- Modify: `apps/conary/src/commands/state.rs`
- Modify: `apps/conary/src/commands/system.rs`
- Modify: `apps/conary/src/commands/query/scripts.rs`

- [ ] **Step 1: Write failing remove tests**

Add integration tests proving:

- removing a bundle-carrying package with selected legacy remove entry and no flag refuses before mutation;
- removing with `--allow-legacy-replay` runs pre-remove and post-remove entries once;
- remove works after the original `.ccs` archive has been deleted from cache;
- post-remove uses the in-memory plan after trove deletion, not a DB lookup after cascade;
- a remove-path cascade regression mirrors the real flow: load and plan both remove phases, delete the trove, prove the DB row is gone, then execute/assert the in-memory post-remove plan still has the needed entry data;
- `--no-scripts` with required remove replay refuses;
- malformed installed `bundle_toml` returns a typed error and does not panic;
- after successful remove, the cascade removes the installed bundle row.

- [ ] **Step 2: Load installed bundle before flattened scriptlets**

In remove flow:

1. resolve the installed trove;
2. load `InstalledLegacyScriptletBundle::find_by_trove()`;
3. decode and validate bundle;
4. plan `RemovePre` and `RemovePost`;
5. store both plans in remove preparation state;
6. only then consult flattened `ScriptletEntry` rows for native non-bundle paths.

The insertion point is before `ScriptletEntry::find_by_trove()` in `prepare_remove()`.

Add explicit plan fields to `apps/conary/src/commands/remove.rs`:

```rust
struct PreparedRemove {
    // existing fields...
    planned_pre_remove: Option<LegacyReplayPlan>,
    planned_post_remove: Option<LegacyReplayPlan>,
}

pub(crate) struct RemoveInnerResult {
    // existing fields...
    planned_post_remove: Option<LegacyReplayPlan>,
}
```

Carry `planned_post_remove` from `prepare_remove()` through `commit_remove_db()` into `RemoveInnerResult`, then consume it in `run_post_remove_scriptlet()`. This is required because the installed bundle row may be gone after cascade deletion.

- [ ] **Step 3: Execute remove pre/post from in-memory plans**

Execute planned pre-remove entries before DB deletion. Execute planned post-remove entries after current removal/generation publication using the in-memory plan.

Do not query `installed_legacy_scriptlet_bundles` after `Trove::delete()`, because `ON DELETE CASCADE` may already have removed the row.

- [ ] **Step 4: Write failing upgrade tests**

Add tests proving:

- upgrade from old bundle to new bundle loads old bundle before old trove deletion;
- old pre-remove runs before old `Trove::delete()`;
- new pre-install/pre-upgrade runs before new payload mutation;
- old post-remove and new post-install/post-upgrade run in the expected order;
- if upgrade fails mid-transaction, old trove and old installed bundle are preserved by rollback;
- old and new bundle rows do not coexist for the same final trove after success.

- [ ] **Step 5: Integrate old-bundle upgrade plan**

In `install_ccs_package_transactionally()` after upgrade status is known and before payload mutation/dry-run return:

- load old installed bundle by old trove ID;
- plan old `UpgradeOldPreRemove` and `UpgradeOldPostRemove`;
- store plans in the install transaction state;
- execute old pre-remove before `install_inner_with_stored_files()` deletes the old trove;
- execute old post-remove from memory after generation publication.

If the current structure makes exact old post-remove ordering hard, stop and split the refactor rather than querying after cascade.

- [ ] **Step 6: Wire rollback/state safety**

Find rollback/state/system operations that remove or restore troves.

Start with:

```text
rg "Trove::delete|trove.insert" apps/conary/src/commands/system.rs apps/conary/src/commands/state.rs
```

List every trove-mutating rollback/state entry point in the task notes before editing.

Rules:

- if they can receive `LegacyReplayOptions`, enforce the same planner decisions before mutation;
- if they cannot in Goal 6, fail closed with `RollbackReplayUnavailable` whenever an installed bundle would require raw legacy replay;
- do not silently skip raw replay while claiming a complete rollback.

For every rollback mutation path, load the installed bundle for any trove that would be removed/restored and run the planner before DB, CAS, generation, or live-root mutation. If raw replay is selected and the rollback command did not explicitly thread `--allow-legacy-replay`, return `LegacyReplayRefusalKind::RollbackReplayUnavailable` before mutation. If a path cannot reliably determine whether a bundle exists, fail closed rather than assuming native-free.

Add tests:

- rollback refuses legacy replay without flags;
- strict host policy still rejects foreign replay even with both flags;
- rollback dry-run or preview path does not mutate installed bundle rows.

- [ ] **Step 7: Update query scripts behavior**

`apps/conary/src/commands/query/scripts.rs` should distinguish:

- flattened native scriptlets from `scriptlets`;
- installed bundle entries from `installed_legacy_scriptlet_bundles`;
- replay decision and lifecycle phase in output.

Keep this read-only and do not expose hidden local paths.

- [ ] **Step 8: Verify Task 6**

Run:

```text
cargo test -p conary --test bundle_replay remove
cargo test -p conary --test bundle_replay upgrade
cargo test -p conary rollback
cargo test -p conary query_scripts
cargo fmt --check
```

Expected: remove/upgrade/rollback do not bypass installed bundle policy.

## Task 7: Batch, Restore, Update, Autoremove, And Multi-Package Gates

**Files:**

- Modify: `apps/conary/src/commands/install/batch.rs`
- Modify: `apps/conary/src/commands/install/restore.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/collection.rs`
- Modify: `apps/conary/src/commands/model/apply.rs`
- Modify: `apps/conary/src/commands/state.rs`
- Modify: `apps/conary/src/commands/automation.rs`

- [ ] **Step 1: Write failing batch and restore tests**

Add tests proving:

- batch install refuses a non-public/admission-failed bundle before any package in the batch mutates state;
- batch install with one legacy-required package and no flag refuses the entire batch before partial install;
- a three-package batch where package 2 fails bundle admission leaves package 1 and package 3 unmodified, with no trove/file/scriptlet rows inserted for the attempted batch;
- prepared package state carries legacy replay plans rather than re-reading archive state after mutation begins;
- restore install refuses a bundle requiring raw replay when options default disabled;
- restore install with a review/blocked entry refuses before `run_pre_install_phase()` or `install_inner()`.

- [ ] **Step 2: Use the prepared legacy plan fields added in Task 5**

Task 5 adds the optional prepared legacy plan fields early. In Task 7, wire those fields through real batch and restore preparation rather than adding them for the first time.

The prepared structs should already be able to carry:

- new bundle pre plan;
- new bundle post plan;
- old installed bundle pre-remove plan if upgrade;
- old installed bundle post-remove plan if upgrade;
- accepted bundle to persist;
- target and policy audit data.

Do not re-open the CCS archive or re-query the installed bundle after preflight. If any required prepared field was not added in Task 5, add it before continuing with Task 7 integration.

- [ ] **Step 3: Integrate batch and restore preflight**

In batch planning:

- parse bundles while preparing each package;
- plan all packages before the first mutation;
- if any package refuses, return the first structured refusal with package name and lifecycle entry IDs;
- only proceed to store files/run scriptlets after all candidate packages pass.

In restore:

- thread `LegacyReplayOptions` from state/system command;
- default disabled;
- refuse before live root or DB mutation if replay would be required.

- [ ] **Step 4: Write failing update tests**

Add tests proving:

- update gains `--no-scripts` and both replay flags;
- update with selected package requiring legacy replay refuses before creating the update summary changeset when flags are absent;
- update with three selected packages where one package has a blocked bundle leaves zero new changeset rows and no partial package mutation;
- update with review/blocked bundle refuses before mutation;
- update with accepted same-source legacy replay records audit metadata;
- top-level update-all refuses before partial updates when one selected package fails admission;
- collection/group update preserves its current explicit best-effort semantics, but each member still fails closed before its own mutation and reports legacy replay refusals clearly.

- [ ] **Step 5: Fix update preflight ordering**

In `cmd_update()`:

- resolve selected packages;
- download and parse enough of each selected package to read its `CcsManifest.legacy_scriptlets` and derive accepted legacy replay plans;
- for delta candidates, do not call `DeltaApplier::apply_delta()` before bundle admission because it stores the reconstructed package in CAS. Add a non-mutating delta preview helper that returns verified reconstructed bytes, or fall back to resolving/downloading the full candidate package for preflight. Only write the reconstructed package to CAS after admission passes;
- preflight bundles for all selected update packages before inserting the update summary changeset at the current `Changeset::new(...).insert(tx)` site;
- if any bundle refuses, return the structured refusal immediately and leave zero new update changeset rows;
- if all pass, create the update summary changeset and proceed with the install loop;
- carry accepted plans into the later install call rather than re-reading package archives;
- preserve current behavior for packages without bundles.

This plan intentionally chooses preflight-before-changeset rather than moving changeset creation after a partial admission filter. Refused packages should not be silently dropped from the changeset; the whole update fails before the changeset exists.

`cmd_update_group()` is already documented as best-effort per-package. Do not silently change that user contract in Goal 6. Instead, thread `LegacyReplayOptions` and `no_scripts` into each member update, fail each refused member before its own mutation, and make the group summary count/report legacy replay refusals distinctly from download or install failures.

- [ ] **Step 6: Write failing autoremove tests**

Add tests proving:

- autoremove with any candidate that needs raw legacy remove replay refuses before removing any package when flags are absent;
- autoremove does not silently skip legacy-bearing packages;
- per-round fixed-point logic preflights each round before first deletion in that round;
- accepted autoremove with flags executes planned remove replay once.

- [ ] **Step 7: Integrate autoremove and remove options**

Thread `LegacyReplayOptions` through `cmd_autoremove()`.

For each fixed-point round:

- use the existing dependency solver/fixed-point logic to compute the candidate closure for the round without deleting from the DB;
- preflight every candidate that has an installed bundle;
- if any candidate refuses, stop before deleting any candidate in that round;
- otherwise remove as today with the accepted remove plans.

This may do more upfront checking than a best-effort autoremove, but it preserves the all-or-nothing safety invariant and avoids silently skipping legacy-bearing packages.

- [ ] **Step 8: Model, collection, and automation safety**

Update:

- model apply/replatform installs;
- collection install/update;
- automation plan execution;
- state restore orchestration.

Default replay disabled. If a replatform/model apply operation fails because raw replay is required, the error must name the package and say the safe choices are selecting a different target or waiting for adapter coverage.

- [ ] **Step 9: Verify Task 7**

Run:

```text
cargo test -p conary batch
cargo test -p conary restore
cargo test -p conary update
cargo test -p conary autoremove
cargo test -p conary model
cargo fmt --check
```

Expected: multi-package operations fail before partial mutation when bundle policy refuses.

## Task 8: Foreign Replay, No-Scripts, Audit, And Documentation Polish

**Files:**

- Modify: `apps/conary/src/live_host_safety.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/source-selection.md`
- Modify: `docs/modules/query.md`
- Modify: `docs/ARCHITECTURE.md` if it has lifecycle/install scriptlet status text.

- [ ] **Step 1: Write foreign replay tests**

Add tests proving:

- same-source raw replay can proceed with `--allow-legacy-replay`;
- foreign replay is denied under host `strict` even with both flags;
- host `guarded` only allows explicit guarded-compatible bundle policy and target compatibility;
- host `permissive` still requires both flags;
- bundle `foreign_replay_policy = "deny"` rejects foreign target even under permissive host policy;
- `--allow-foreign-legacy-replay` without `--allow-legacy-replay` is insufficient;
- unknown target release refuses raw replay.

- [ ] **Step 2: Strengthen `--no-scripts` tests**

Add tests for:

- no bundle keeps existing `--no-scripts` behavior;
- native-free bundle allowed;
- replaced-only bundle allowed but existing CCS hooks suppressed;
- selected legacy entry refuses with `NoScriptsWouldSkipRequiredReplay`;
- review/blocked entry refuses regardless of `--no-scripts`;
- mixed bundle with future lifecycle legacy entry does not refuse current lifecycle solely because of future raw replay, but the bundle is persisted.

- [ ] **Step 3: Verify audit metadata**

Add tests asserting changeset audit metadata includes:

- bundle presence;
- target ID and source target ID;
- target compatibility;
- foreign replay policy;
- host policy;
- feature gate usage;
- foreign override;
- evidence digest;
- planned entries;
- entry outcome after execution when available.

Do not expose local file paths.

- [ ] **Step 4: Documentation updates**

Update docs to reflect current behavior after Goal 6 lands:

- `docs/modules/ccs.md`: CCS bundles can be consumed locally; raw replay requires explicit flags; review/blocked refuse.
- `docs/modules/source-selection.md`: strict/guarded/permissive host policy relationship to foreign legacy replay.
- `docs/modules/query.md`: query scripts can show installed bundle entries and their replay decisions.
- `docs/ARCHITECTURE.md`: check for mentions of scriptlets, install lifecycle, CCS hooks, remove hooks, or native scriptlet replay. If any section still says or implies that CCS packages never consume native scriptlet bundles after Goal 6, update it.

Keep Remi docs unchanged unless they explicitly mention client-side replay as future-only in a way that Goal 6 makes stale.

- [ ] **Step 5: Final verification**

Run:

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p conary-core legacy_replay
cargo test -p conary-core legacy_scriptlets
cargo test -p conary-core installed_legacy_scriptlet_bundle
cargo test -p conary-core scriptlet
cargo test -p conary --test bundle_replay
cargo test -p conary --test foreign_replay
cargo test -p conary
cargo run -p conary-test -- list
git diff --check
```

If integration test files use different names, use `cargo test -p conary --test <actual-test-file>` rather than module-filter syntax.

Expected: all targeted and package tests pass; `cargo run -p conary-test -- list` succeeds and includes any Goal 6 harness suites if they are registered there; no formatting or whitespace errors.

## Implementation Sequencing Notes

Recommended `/goal` slices:

1. Task 1 only: pure planner, target IDs, body decoding.
2. Task 2 only: v71 installed bundle persistence.
3. Task 3 only: executor preflight and legacy entry execution contracts.
4. Task 4 only: CLI/options plumbing and compile repair across direct literals. If it is too large, split it into 4a option structs/defaults and 4b CLI flags/dispatch wiring; Task 5 needs 4a at minimum.
5. Task 5 only: CCS/fresh install integration.
6. Task 6 only: remove/upgrade/rollback integration.
7. Task 7 only: batch/restore/update/autoremove/multi-package integration.
8. Task 8 only: foreign replay hardening, audit, docs, final verification.

Do not start Task 5 before Tasks 1 through 4 compile. Task 5 needs the planner, persistence model, executor contracts, and option plumbing.

Do not start Task 6 before Task 2. Remove and upgrade must rely on installed bundle persistence, not the original CCS archive.

Do not collapse Task 7 into Task 5 opportunistically. The multi-package paths are where partial mutation bugs hide.

## Review Checklist Before Implementation

- [ ] The plan uses synthetic legacy fixtures and does not require Goal 4 conversion to emit `legacy`.
- [ ] Dry-run preflight is before the existing install dry-run return.
- [ ] Update preflight is before update changeset creation, and a refusal leaves zero new update changeset rows.
- [ ] Update delta candidates do not write reconstructed packages to CAS before bundle admission.
- [ ] Collection/group update preserves its explicit best-effort contract while each member fails closed before its own mutation.
- [ ] Remove/upgrade preload installed bundles before `Trove::delete()`.
- [ ] Post-remove uses in-memory plans, not DB lookup after cascade.
- [ ] `--no-scripts` is not a bypass for selected `legacy` entries.
- [ ] Update and CCS install gain `--no-scripts` parity.
- [ ] Internal callers default replay flags false.
- [ ] Foreign replay requires compatible target, permissive host policy, bundle permission, and both flags.
- [ ] Bundle-covered CCS entries do not also get flattened `ScriptletEntry` rows.
- [ ] Rollback either enforces the same gate or fails closed.
- [ ] Final verification includes both core planner tests and conary integration tests.
