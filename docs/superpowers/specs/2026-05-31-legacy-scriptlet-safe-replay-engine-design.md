# Legacy Scriptlet Safe Replay Engine Design

## Summary

Goal 6 is the first client-side consumption point for the passive legacy
scriptlet bundle work. Goals 1 through 5 created the bundle schema, extracted
native ABI evidence, classified commands, embedded bundles in converted CCS
archives, and prevented Remi from publicly serving non-public converted
artifacts. Goal 6 makes `conary install`, `conary ccs install`, update, remove,
batch, restore-aware install, and rollback paths read those bundles and enforce
their decisions before any live mutation.

The work is safety-first. A package that has `review`, `blocked`, or unknown
bundle entries must fail before hooks, file deployment, DB writes, generation
publication, or remove mutation. A lifecycle that needs raw native scriptlet
replay must also fail unless an explicit local feature gate is enabled and the
bundle's target compatibility check passes. Cross-distro raw replay is denied
by default.

Goal 6 does not broaden Remi publication, does not add curation promotion, does
not expand the target compatibility matrix beyond exact/source-native and
explicit allowed-target checks, and does not teach Conary to translate more
scriptlet commands. It only consumes the existing bundle truthfully and stores
the accepted bundle locally so remove and upgrade can enforce the same policy
after the original `.ccs` archive is gone.

## Source Context

Read these first when implementing:

- `AGENTS.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-passive-remi-bundle-embedding-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-publication-gate-design.md`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/ccs/manifest.rs`
- `crates/conary-core/src/scriptlet/mod.rs`
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

Relevant current code facts:

- `CcsManifest` already has `legacy_scriptlets:
  Option<LegacyScriptletBundle>`, and manifest validation calls
  `LegacyScriptletBundle::validate()`.
- `CcsPackage::scriptlets()` returns an empty slice. Converted CCS packages
  currently execute declarative CCS hooks, not flattened native scriptlets.
- `install_ccs_package_transactionally()` is the direct CCS transaction
  boundary. It already runs capability gates, upgrade checks, payload path
  validation, CCS hooks, existing flattened scriptlet phases, DB insertion, and
  post-commit hooks.
- `cmd_install` can install converted CCS packages through
  `install/conversion.rs`; those calls ultimately reach the same transactional
  CCS install path.
- Existing native package installs use `run_pre_install_phase()`,
  `finalize_install_without_snapshot()`, and `ScriptletEntry` rows for
  install/remove hooks. Goal 6 must not regress that path.
- Removal currently looks up flattened `ScriptletEntry` rows by trove and runs
  pre-remove before DB mutation and post-remove after generation publication.
- The only installed scriptlet persistence today is the `scriptlets` table,
  which stores one flattened phase row and cannot store the complete
  `LegacyScriptletBundle`, target decision, evidence digest, or per-entry
  replay policy.
- `SCHEMA_VERSION` is currently 70. Goal 6 needs a new local installed-state
  migration.
- `DistroPin` stores a current distro and `mixing_policy` with values already
  using the `strict`, `guarded`, and `permissive` vocabulary.
- `repository/distro.rs` maps supported distro IDs to package-family/version
  schemes. It does not yet expose a target identifier builder in the form
  `<format>/<distro>/<release>/<arch>`.
- `ScriptletExecutor` already handles package-format argument conventions,
  target-root execution, protected live-root sandbox preflight, per-scriptlet
  timeout, and post-commit warning classification, but it does not yet expose a
  bundle-entry execution API that preserves `native_invocation`,
  `interpreter_args`, stdin contracts, or entry-specific sandbox floors.

## Scope

Goal 6 includes:

- local install/update/remove/batch/restore/rollback preflight that consumes
  `CcsManifest.legacy_scriptlets`;
- an explicit legacy replay feature gate for entries with
  `decision = "legacy"`;
- target ID construction in the form `<format>/<distro>/<release>/<arch>`;
- target compatibility checks for `source-native`, `family-compatible`,
  `conary-portable`, `review-required`, and `blocked`;
- enforcement of bundle `foreign_replay_policy` before live mutation;
- host policy handling for `strict`, `guarded`, and `permissive`;
- persistence of the complete accepted `LegacyScriptletBundle` in local
  installed-package state;
- remove and upgrade lookup of that installed bundle;
- preflight of install, remove, and upgrade lifecycle entries before DB or file
  mutation;
- raw replay only for `legacy` entries whose target compatibility and feature
  gate both pass;
- continued execution of generated CCS hooks for `replaced` entries when such
  hooks were actually materialized by conversion;
- audit metadata that records target ID, bundle evidence digest, policy
  decisions, feature-gate usage, and any foreign replay override;
- tests proving same-source dry-run/install/remove planning works and
  cross-distro replay is rejected before mutation.

Goal 6 excludes:

- Remi curation promotion or publication broadening;
- Goal 7's expanded compatibility matrix and durable override workflow;
- translation of additional scriptlet commands into adapters;
- replay of `review` or `blocked` entries under any flag;
- package-manager recursion, networked scriptlets, or helper calls explicitly
  blocked by the bundle;
- broad shell rewriting or partial residual replay synthesis;
- automatic compatibility for derivatives merely because they share a package
  format;
- exact preservation of every native package-manager trigger behavior;
- a corpus-scale golden test run. The fixture plan is documented here, but the
  full corpus belongs to Goal 8a.

## Safety Model

Bundle consumption has two independent gates:

1. **Admission gate:** Is this bundle safe to install/remove/update at all?
   `review`, `blocked`, and unknown entries anywhere in the bundle fail here,
   before lifecycle selection. So do malformed bundles and target compatibility
   states that are `review-required`, `blocked`, or unknown. Unsupported raw
   trigger and file-trigger replay also fails here unless the entry is already
   `replaced`.
2. **Execution gate:** Is raw native replay allowed for this accepted entry at
   this lifecycle point? `legacy` entries require the local feature gate,
   compatible target metadata, sandbox preflight, and native-compatible
   invocation arguments. `replaced` entries do not replay raw script bodies.

The execution gate is lifecycle-scoped. A fresh install should not execute or
require feature-gate approval for a future `post-remove` legacy entry, but the
installed bundle must be persisted so the later remove operation can enforce
that lifecycle before mutation.

The admission gate always runs before:

- CCS pre-hooks;
- native pre-install/pre-remove scriptlets;
- file extraction/deployment into CAS or live roots;
- DB insert/delete/update;
- generation publication;
- state snapshot creation;
- post-commit hooks.

This is intentionally stricter than the current `--no-scripts` behavior for
native packages. For bundles, `--no-scripts` is not a bypass for unsafe or
incomplete conversion. It may suppress execution for packages that do not need
raw replay, but it must not turn a `review`, `blocked`, or `legacy` entry into a
silently accepted install. A future explicit "install incomplete anyway" escape
hatch would need its own design and audit trail.

## Operator Surface

Add a narrow local feature gate for raw legacy replay:

```text
--allow-legacy-replay
```

This flag is required whenever an accepted lifecycle contains at least one
`decision = "legacy"` entry that would execute. Without it, preflight returns a
clear error before mutation. The flag does not allow `review` or `blocked`
entries and does not weaken target compatibility.

Add a second, even narrower foreign replay override:

```text
--allow-foreign-legacy-replay
```

This flag is considered only when the host source-selection policy is
`permissive`, the bundle is not `foreign_replay_policy = "deny"`, the target is
explicitly compatible, and `--allow-legacy-replay` is also set. It records
changeset audit metadata. It is ignored for `strict` and insufficient for
`guarded`.

Add these flags to:

- `conary install`;
- `conary update`, which should also gain `--no-scripts` so update can express
  the same `NoScriptsWouldSkipRequiredReplay` semantics as install;
- `conary remove`;
- `conary autoremove`;
- `conary ccs install`;
- rollback commands that mutate installed troves, if they are allowed to replay
  legacy entries in Goal 6.

`conary ccs install` should also gain `--no-scripts` parity with
`conary install path.ccs`, or the implementation must explicitly document that
operators should use the root `install` command when they need hook suppression
semantics for a CCS archive. Prefer adding the flag for consistency because the
transaction options already carry `no_scripts`.

Internal callers, including model apply, batch install, conversion install, and
restore paths, should carry the same option struct and default both flags to
false. Automation must opt in intentionally.

Rollback is a safety boundary, not an exception. A rollback path that removes or
restores a trove with an installed legacy bundle must either receive the same
`LegacyReplayOptions` and enforce the same planner decisions, or fail closed
before DB, CAS, generation, or live-root mutation. Goal 6 should not silently
skip raw replay during rollback and still claim a semantically complete revert.

Autoremove should be all-or-nothing for legacy replay gates. It currently uses
a fixed-point loop where removing one orphan may reveal another orphan. Goal 6
must compute the candidate closure without mutation, or conservatively preflight
each round before deleting any package in that round and stop before the first
mutation if a legacy replay refusal appears. Do not silently skip only the
legacy-bearing packages.

Keep the existing `--sandbox=auto|always|never` surface, but bundle replay may
raise the effective sandbox floor. The caller can ask for stricter sandboxing
than the bundle requires. The caller cannot make a bundle-required protected
sandbox looser by passing `--sandbox=never`.

## Policy Inputs

Introduce a shared input type for install/remove planning:

```rust
pub struct LegacyReplayPolicyInput<'a> {
    pub replay_enabled: bool,
    pub foreign_replay_override: bool,
    pub no_scripts: bool,
    pub requested_sandbox_mode: SandboxMode,
    pub host_policy: HostForeignReplayPolicy,
    pub target: ReplayTarget<'a>,
}

pub enum HostForeignReplayPolicy {
    Strict,
    Guarded,
    Permissive,
}
```

`host_policy` should come from the current `DistroPin.mixing_policy` when a
pin exists. If no pin exists, default to `Strict`. This avoids accidental
foreign replay on unpinned hosts.

`ReplayTarget` is deterministic and testable:

```rust
pub struct ReplayTarget<'a> {
    pub format: &'a str,
    pub distro: &'a str,
    pub release: &'a str,
    pub arch: &'a str,
}
```

Its string form is exactly:

```text
<format>/<distro>/<release>/<arch>
```

Examples:

- `rpm/fedora/44/x86_64`
- `deb/ubuntu/26.04/x86_64`
- `arch/arch/rolling/x86_64`

Target resolution order:

1. use the current `DistroPin.distro` when present;
2. otherwise use a host `/etc/os-release` identity if available;
3. otherwise use `unknown/unknown/unknown/<arch>`.

`DistroPin.distro` normalization must be explicit:

- `fedora-44` becomes `rpm/fedora/44/<arch>`;
- `ubuntu-26.04` becomes `deb/ubuntu/26.04/<arch>`;
- `arch` becomes `arch/arch/rolling/<arch>`;
- Arch bundle source IDs normalize `source_release = None` or an empty release
  to `rolling` when `source_distro = "arch"`;
- generic family names such as `fedora`, `ubuntu`, `debian`, or `arch` map to
  their package format and distro family, but release is taken from host
  `/etc/os-release` only when the host identity matches that family;
- if the pin is generic and the host release cannot be trusted, use `unknown`
  for release rather than pretending a same-source match.

An `unknown` release never silently grants source-native compatibility. Raw
replay with an unknown target release is allowed only when the source target is
also unknown in the same position or the bundle's `allowed_targets` explicitly
contains the exact rendered target ID.

The architecture should be normalized through the existing repository selector
architecture normalization rules. Test code must be able to inject a
`ReplayTarget` directly so unit tests do not depend on the developer host.

Add helper functions in `crates/conary-core/src/repository/distro.rs`:

```rust
pub fn replay_target_from_distro_id(distro_id: &str, arch: &str) -> Option<ReplayTargetOwned>;
pub fn replay_target_id(target: &ReplayTarget<'_>) -> String;
pub fn source_target_from_bundle(bundle: &LegacyScriptletBundle) -> ReplayTargetOwned;
```

`ReplayTargetOwned` should be a simple owned version of `ReplayTarget`.

## Target Compatibility Rules

The policy planner should consume the full bundle plus the target context and
return an executable plan or a typed refusal:

```rust
pub enum LegacyReplayPreflight {
    NativeFree,
    FullyReplaced(LegacyReplayPlan),
    RequiresReplay(LegacyReplayPlan),
    Refused(LegacyReplayRefusal),
}

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

Use these package-level rules:

| target_compatibility | Goal 6 behavior |
| --- | --- |
| `conary-portable` | Allowed. No raw replay is required. `replaced` entries are satisfied by declarative CCS hooks. |
| `source-native` | Raw replay allowed only when target ID exactly matches the source target ID, or the target appears in `allowed_targets`. |
| `family-compatible` | Raw replay allowed only when target appears in `allowed_targets` or a minimal compatibility matrix entry allows it. Goal 6's matrix should only include exact-source fixtures; Goal 7 expands it. |
| `review-required` | Refused before mutation. |
| `blocked` | Refused before mutation. |
| unknown value | Refused before mutation. |

Then apply bundle `foreign_replay_policy`:

| bundle foreign_replay_policy | Goal 6 behavior |
| --- | --- |
| `deny` | Any non-source-native raw replay is refused before mutation. This is the default emitted by Goal 4. |
| `guarded` | Foreign replay may proceed only if host policy is `guarded` or `permissive`, target compatibility passed, and helper/sandbox preflight passed. |
| `permissive` | Foreign replay may proceed only if host policy is `permissive`, target compatibility passed, helper/sandbox preflight passed, and both explicit flags are present. |
| unknown value | Refused before mutation. |

Then apply host policy:

| host policy | Goal 6 behavior |
| --- | --- |
| `strict` | Reject every foreign raw replay before mutation. |
| `guarded` | Allow foreign raw replay only when the bundle permits guarded replay and target compatibility is explicit. No operator override bypass. |
| `permissive` | Still require `--allow-legacy-replay` and `--allow-foreign-legacy-replay`; record the override. |

This deliberately means that most Goal 4 bundles, which currently use
`foreign_replay_policy = "deny"`, remain same-source only. That is correct for
Goal 6. Goal 7 can broaden matrix entries and curation-derived bundle policies.

## Entry Decision Rules

Entry decisions are enforced before lifecycle execution:

| entry decision | Goal 6 behavior |
| --- | --- |
| `replaced` | Never replay the raw body. Let materialized CCS declarative hooks run through existing hook paths; otherwise treat the entry as satisfied by the adapter/payload evidence that made it `replaced`. |
| `legacy` | Replay the raw body only if admission, feature gate, target compatibility, and sandbox/native invocation preflight all pass. |
| `review` | Refuse the package operation before mutation. |
| `blocked` | Refuse the package operation before mutation. |
| unknown | Refuse the package operation before mutation. |

The no-double-application invariant is absolute: if an entry is `replaced`, its
body is not executed. If an entry is `legacy`, only that entry's raw body is
executed. Generated hooks may still run for different `replaced` entries in the
same phase, but the planner must never schedule a raw body and a generated hook
for the same entry ID.

Goal 6 must not synthesize new runtime actions for passive complete effects.
Some bootstrap adapters classify derived cache refreshes, system metadata
reloads, or payload-backed declarations as `replaced` without generating a CCS
hook. Those entries are satisfied by the conversion evidence and installed
payload, not by a new replay step in Goal 6.

Current CCS hooks are phase-level rather than entry-linked, so Goal 6 must not
claim ordering precision it cannot enforce. The planner should build a small
phase schedule over bundle entries and generated-hook placeholders wherever
`transaction_order.before` or `transaction_order.after` gives enough data to
serialize them. If a package mixes raw legacy entries and generated CCS hooks in
pre-mutation phases and the declared ordering cannot be represented safely, the
operation must fail closed with an ordering refusal before any hook or scriptlet
runs.

The coarse fallback order is allowed only for lifecycle groups with no
conflicting `transaction_order` constraints:

1. pre-mutation legacy pre entries whose decision is `legacy`;
2. existing CCS pre-hooks;
3. DB/CAS/generation transaction;
4. existing CCS post-hooks;
5. post-commit legacy post entries whose decision is `legacy`;
6. triggers.

The implementation plan should avoid ad hoc ordering inside
`install_ccs_package_transactionally()`: use the planner's schedule or refuse.

## Lifecycle Mapping

Map bundle lifecycle paths to Conary operation phases:

| Conary operation | Bundle entries considered |
| --- | --- |
| fresh install pre-mutation | `pre-install`, `pre-transaction` |
| fresh install post-commit | `post-install`, `post-transaction` |
| upgrade new package pre-mutation | `pre-upgrade`, fallback `pre-install` for RPM/DEB semantics |
| upgrade new package post-commit | `post-upgrade`, fallback `post-install` for RPM/DEB semantics |
| upgrade old package pre-mutation | installed old bundle `pre-remove` with upgrade-removal mode |
| upgrade old package post-commit | installed old bundle `post-remove` with upgrade-removal mode |
| remove pre-mutation | installed bundle `pre-remove` |
| remove post-commit | installed bundle `post-remove` |
| triggers/file-triggers | refused or review-only in Goal 6 unless the entry is `replaced`; raw trigger replay waits for a later goal |

For RPM and DEB, the source package manager often distinguishes upgrade from
install/remove through argv rather than a separate scriptlet slot. Those argv
values are runtime facts. The replay runner must derive dynamic lifecycle
arguments from the current `ExecutionMode` and operation context, including the
actual old version, new version, and native count/state arguments where the
package family expects them.

`entry.native_invocation.args` is a contract projection, not literal argv.
Goal 4 stores stable strings such as `1:old-version=old-version`,
`2:new-version=new-version`, or `1:count=package-instance-count`. The replay
runner must parse those contracts, derive the actual argv values from runtime
state, and never pass the contract strings themselves to the scriptlet. Raw
contract values (`raw:<value>`) may become literal arguments only when they are
state-independent and valid for the selected lifecycle.

If a contract is malformed, names a value that Goal 6 cannot supply, or
conflicts with package-format `ExecutionMode` rules, preflight refuses with a
typed args-contract conflict. Placeholder interpolation for future/manual
bundle formats is out of scope unless the implementation adds it explicitly and
tests it.

For Arch `.INSTALL` functions, the bundle's `arch_install.called_function`
should be authoritative when present. If it is absent, use the current Arch
wrapper mapping by phase.

## Installed Bundle Persistence

Add a schema migration, likely v71, for installed bundle state:

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

Store `bundle_toml` rather than only scalar fields so remove and upgrade can
recover the complete `LegacyScriptletBundle` even if the original archive is no
longer available. The row is deleted by `ON DELETE CASCADE` when the trove is
removed.

The cascade is safe only if remove and upgrade follow the existing flattened
scriptlet pattern: load, validate, and decode the installed bundle before
`Trove::delete`, then carry the complete planned pre-remove and post-remove
state in memory through the rest of the operation. Post-remove execution must
not query `installed_legacy_scriptlet_bundles` after the trove row has been
deleted. If the implementation cannot maintain this invariant, the schema must
drop `ON DELETE CASCADE` and explicitly delete the bundle row only after
post-remove replay and audit handling complete.

Add a model under
`crates/conary-core/src/db/models/installed_legacy_scriptlet_bundle.rs` with:

- `new(trove_id, changeset_id, target_id, replay_policy, replay_enabled, bundle)`;
- `insert_or_replace(&mut self, conn)`;
- `find_by_trove(conn, trove_id)`;
- `bundle(&self) -> anyhow::Result<LegacyScriptletBundle>`;
- `delete_by_trove(conn, trove_id)` for explicit cleanup tests.

The model should validate the bundle before insert and after decode. It should
reject rows whose scalar evidence digest disagrees with the decoded bundle.

Persist the installed bundle in the same DB transaction as the package trove.
For install, this means threading accepted bundle state into
`install_inner_with_stored_files()` after the trove ID is created and before
the transaction commits. For upgrade, delete of the old trove may cascade the
old bundle only after old pre-remove and post-remove plans have both been built
from the installed old bundle and copied into the upgrade execution state.

For bundle-carrying CCS packages, the installed bundle row is the authority for
remove and upgrade replay decisions. Do not also insert flattened
`ScriptletEntry` rows for lifecycle entries covered by `legacy_scriptlets`; that
would create a double-execution hazard when remove code consults both stores.
Native non-CCS package installs continue to populate the existing `scriptlets`
table as they do today.

Store `scriptlet_fidelity` as-is from the bundle summary. Do not use it as the
only indicator of raw replay need: Goal 4-produced bundles may still be
`fully-replaced`, `review-required`, or another non-`legacy-replay` value.
Planner code should inspect entry decisions or decision counts directly.

The `replay_enabled` column is install-time audit metadata, not a durable
permission grant. Every later operation that would execute a `legacy` entry
must still receive current `LegacyReplayOptions` and pass the execution gate.
Do not let a previous install-time flag silently authorize future remove,
upgrade, restore, or rollback replay.

Future goals may add an index on `(source_package, source_version)` for global
policy queries over installed bundles. Goal 6 only needs trove-ID lookup, so
the package index is intentionally deferred.

## Replay Planner Module

Add a core module that plans but does not touch the filesystem:

```text
crates/conary-core/src/ccs/legacy_replay.rs
```

Responsibilities:

- validate bundle admission;
- build source and target IDs;
- evaluate package-level target compatibility;
- evaluate bundle foreign replay policy and host policy;
- reject review/blocked/unknown entries across the whole bundle before
  selecting lifecycle entries;
- reject unsupported raw trigger/file-trigger entries before mutation;
- select accepted entries for a lifecycle event;
- determine whether raw replay is required;
- compute the sandbox floor and timeout for each planned legacy entry;
- produce structured refusal reasons.

Candidate public API:

```rust
pub fn plan_legacy_replay(
    bundle: Option<&LegacyScriptletBundle>,
    lifecycle: LegacyReplayLifecycle,
    input: &LegacyReplayPolicyInput<'_>,
) -> anyhow::Result<LegacyReplayPreflight>;
```

`None` should mean native CCS or no legacy bundle: return `NativeFree`.

Use typed refusals, not stringly errors, inside the core:

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

CLI code can format these into concise user errors and changeset audit
metadata.

## Replay Executor API

Extend `ScriptletExecutor` or add a small adapter in `conary-core` for bundle
entries:

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
    pub timeout_ms: u64,
}
```

Add methods:

```rust
impl ScriptletExecutor {
    pub fn preflight_legacy_entry(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        mode: &ExecutionMode,
    ) -> Result<()>;

    pub fn execute_legacy_entry_with_outcome(
        &self,
        execution: &LegacyScriptletExecution<'_>,
        mode: &ExecutionMode,
    ) -> ScriptletOutcome;
}
```

Execution differences from existing flattened scriptlets:

- decode and hash-check `body` through `LegacyScriptletEntry::body_bytes()` or a
  new public helper before execution;
- apply `timeout_ms` from the entry;
- prepend `interpreter_args` before the temporary script path where the
  interpreter supports it;
- parse `native_invocation.args` as argument contracts, derive native lifecycle
  argv from `ExecutionMode` and runtime old/new package context, and reject
  unsupported contract values instead of passing contract strings through;
- parse `native_invocation.environment` as `KEY=VALUE` or a `KEY` contract.
  Reject malformed names. A bare `KEY` means the native ABI expected the
  package manager to provide a value; Goal 6 should refuse it unless the runner
  has an explicit runtime value for that key. Reject dangerous environment keys
  such as `LD_PRELOAD`, `LD_LIBRARY_PATH`, `BASH_ENV`, `ENV`, `PYTHONPATH`, and
  `PATH` unless a later capability design permits them;
- provide a fixed safe execution `PATH`, such as `/usr/sbin:/usr/bin:/sbin:/bin`,
  rather than accepting a bundle-supplied override in Goal 6;
- treat `native_invocation.stdin` as a contract string, not literal input
  content. `None` means `Stdio::null()`. Contracts such as `debconf`, `paths`,
  or `unknown` are refused in Goal 6 unless a lifecycle-specific implementation
  explicitly supports them;
- treat `native_invocation.chroot` as a native root expectation contract.
  `install-root` and `package-manager-default` may map to Conary's effective
  target root. `host-root` and `unknown` are refused in Goal 6 unless a later
  compatibility design explicitly supports them;
- keep protected live-root sandbox setup failures fatal.

If the implementation cost of interpreter args/stdin-contract/env support is
too high for the first slice, the plan may split executor work into two commits.
It must not silently ignore fields from a `legacy` entry and still claim exact
replay. Ignoring an unsupported native invocation field should be a preflight
refusal.

The first executor slice should be explicit rather than hidden inside the
current `Scriptlet` trait path:

1. expose a public body-decoding helper on `LegacyScriptletEntry`;
2. implement `preflight_legacy_entry()` as a standalone validation path for
   interpreter args, stdin contracts, native environment, chroot, dynamic args,
   and timeout before touching the filesystem;
3. factor the existing executor internals only as needed so
   `execute_legacy_entry_with_outcome()` can share process setup with flattened
   scriptlets without pretending that bundle-only fields were honored.

## Install Integration

Add `LegacyReplayOptions` to:

- `CcsTransactionInstallOptions`;
- `InstallOptions`;
- internal conversion install options;
- batch/model/restore execution structs as needed.

Default:

```rust
LegacyReplayOptions {
    allow_legacy_replay: false,
    allow_foreign_legacy_replay: false,
}
```

Direct CCS install flow should become:

1. parse and validate CCS package;
2. enforce signature/capability/dependency gates;
3. determine upgrade status and old trove;
4. extract/classify payload enough to know component selection;
5. resolve `ReplayTarget` and host policy;
6. run legacy bundle admission and lifecycle preflight for the new bundle and,
   on upgrades, the old installed bundle;
7. if dry-run, print the dry-run summary and the bundle decision summary, then
   return without mutation;
8. validate payload paths and file ownership;
9. run pre-mutation legacy entries that are planned for raw replay;
10. run existing CCS pre-hooks for materialized replaced behavior;
11. execute the DB/CAS/generation transaction and persist the accepted bundle;
12. run existing CCS post-hooks for materialized replaced behavior;
13. run post-commit legacy entries that are planned for raw replay;
14. append scriptlet warning/audit metadata;
15. run triggers and finish.

The legacy preflight in step 6 must be inserted before the existing
`opts.dry_run` early return in
`apps/conary/src/commands/install/mod.rs::install_ccs_package_transactionally`.
Dry-run should print or otherwise report the `LegacyReplayPreflight` outcome,
then return without persisting the installed bundle or mutating DB/files.

The exact placement of steps 9 and 10 should be revisited during implementation
review against fixtures. The invariant is that all preflight happens before any
pre-hook or raw pre script mutates state.

`cmd_install --convert-to-ccs` and repository install paths that receive a
converted CCS package should use the same `install_ccs_package_transactionally`
path. Native package installs that do not have a `legacy_scriptlets` bundle
continue using existing flattened scriptlet execution.

Batch and restore flows are not allowed to bypass this gate. In particular:

- `apps/conary/src/commands/install/batch.rs` must evaluate the new bundle and
  any old installed bundle while planning each `PreparedPackage`, before
  `install_batch()` stores files, runs pre-install scriptlets, or opens live-root
  mutation;
- `apps/conary/src/commands/install/restore.rs::prepare_install_for_restore`
  must parse CCS bundles into a planned legacy replay state and
  `run_pre_install_for_prepared()` must refuse before `run_pre_install_phase()`
  or `install_inner()` if the plan is not admitted;
- `apps/conary/src/commands/state.rs` restore orchestration must thread
  `LegacyReplayOptions` through the prepared install path instead of defaulting
  to permissive behavior;
- collection install and collection update paths must propagate
  `LegacyReplayOptions`, `--no-scripts`, and sandbox choices into every member
  install/update rather than dropping them at the group boundary;
- automation plan execution must propagate `LegacyReplayOptions` into planned
  install and remove operations, defaulting to disabled unless the automation
  caller explicitly opts in;
- rollback helpers in `apps/conary/src/commands/system.rs` must preflight any
  trove removal/restoration that has an installed bundle before deleting or
  restoring rows. If the rollback command does not expose the explicit replay
  flags in Goal 6, it must refuse operations that require raw legacy replay.

Update has an extra mutation-order trap: `cmd_update()` currently creates an
update summary changeset before invoking `cmd_install()` for each selected
package. Goal 6 must either preflight the selected update packages' bundles
before that changeset is inserted, or move the changeset creation until after
bundle admission succeeds. A legacy replay refusal from update must not leave a
new changeset row behind merely because preflight happened inside the later
install call.

For multi-package operations such as batch install, collection install,
collection update, update-all, and autoremove, bundle admission should be
planned for the whole candidate set before the first package mutates state. If
one member is refused for legacy replay policy, fail the operation before
partially applying earlier members unless the command already has an explicit
best-effort/partial mode with clear audit output.

Model apply and replatforming should default `LegacyReplayOptions` to disabled.
If that causes a replatform install to fail, the error must name the package,
the required replay entries, and the safer operator choices, such as selecting
a different target distro or waiting for adapter coverage.

## Remove And Upgrade Integration

Remove must not rely on the original CCS archive. It should look up
`InstalledLegacyScriptletBundle` by trove ID before mutation:

1. select the trove as today;
2. load, validate, decode, and clone the installed legacy bundle row, if any;
3. resolve target and host policy;
4. run bundle admission and preflight for `pre-remove` and `post-remove`, and
   store both planned phases in `PreparedRemove`/`RemoveInnerResult`;
5. if dry-run support exists for the operation, report decisions and return;
6. execute planned legacy `pre-remove` entries before DB deletion;
7. perform current DB/generation removal;
8. execute planned legacy `post-remove` entries from the in-memory plan after
   publication, without querying the installed-bundle table again;
9. append warnings/audit metadata.

The concrete insertion point is before flattened scriptlet lookup and
execution. After `cmd_remove()` resolves the installed package and rejects
pinned or blocked system packages, `prepare_remove()` should load and preflight
the installed bundle before calling `ScriptletEntry::find_by_trove()` for the
legacy flattened path. A bundle admission failure must return before any
pre-remove scriptlet, file deletion, DB deletion, or generation publication.

For upgrade, the new package bundle and the old installed bundle both matter:

- new bundle: preflight and execute new pre/post install or upgrade entries;
- old installed bundle: preflight and execute old pre/post remove entries in
  upgrade-removal mode from an in-memory plan built before old trove deletion
  cascades the old bundle row;
- the new bundle is persisted with the new trove in the same transaction.

For upgrades, old-bundle lookup, pre-remove preflight, and old pre-remove
execution must happen before `Trove::delete()` in
`install_inner_with_stored_files()`. The implementation should place the
old-bundle planning hook after upgrade status is known in
`install_ccs_package_transactionally()` and before payload extraction/dry-run
summary paths can return.

This avoids a trap where a package installs with accepted legacy replay but its
remove path later lacks the bundle needed to replay or reject safely.

## Audit Metadata

Record a compact changeset metadata section for every bundle-aware operation,
even when no raw replay happened:

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
    "planned_entries": [
      {
        "entry_id": "rpm:%post",
        "phase": "post-install",
        "decision": "legacy",
        "effective_sandbox": "protected-live-root",
        "timeout_ms": 60000,
        "outcome": {
          "exit_code": 0,
          "signal": null,
          "duration_ms": 1234
        }
      }
    ]
  }
}
```

For refused dry-run or preflight failures, the error text is enough for CLI
users, but tests should assert no DB mutation occurred. For real operations
that pass preflight, outcome may be populated after execution. Post-commit
scriptlet failures should still use the existing scriptlet warning metadata
shape and include the bundle entry ID in the warning message.

Goal 7 can add durable curation/override audit tables. Goal 6 only needs enough
metadata to make local changesets explain why replay did or did not run.

## `--no-scripts` Semantics

Bundle-aware behavior:

- no bundle: keep existing `--no-scripts` behavior;
- native-free bundle: allowed, no replay;
- only `replaced` entries: allowed, but existing CCS hooks are suppressed when
  `--no-scripts` suppresses hooks;
- any `legacy` entry selected for the lifecycle: refuse before mutation with
  `NoScriptsWouldSkipRequiredReplay`;
- any `review` or `blocked` entry: refuse before mutation regardless of
  `--no-scripts`.

For a mixed bundle, evaluate the current lifecycle. If any selected entry
requires raw legacy replay, `--no-scripts` refuses the whole operation rather
than suppressing that replay. If the selected lifecycle has only `replaced` or
native-free behavior, `--no-scripts` behaves as a hook-suppression request and
does not bypass bundle admission.

The refusal should be actionable: name the package, list the lifecycle entries
that require raw replay, state that `--no-scripts` cannot skip required legacy
behavior, and suggest rerunning with explicit replay flags only when the target
is compatible.

This preserves safety and avoids installing a package whose conversion evidence
says raw scriptlet behavior is required while explicitly skipping that behavior.

## Helper And Capability Preflight

Goal 6 does not implement the full helper compatibility matrix, but it must not
pretend that foreign replay is safe without helper evidence. Initial preflight
checks:

- bundle validation passes;
- target ID is known for raw replay;
- interpreter exists in the effective execution root or target-root behavior is
  explicitly a skip only for dry-run tests;
- requested sandbox mode satisfies bundle sandbox requirements;
- bundle does not require network access;
- `native_invocation.chroot` is absent, `install-root`, or
  `package-manager-default`; `host-root` and `unknown` are refused in Goal 6;
- `native_invocation.stdin` is absent/none; `debconf`, `paths`, and `unknown`
  stdin contracts are refused in Goal 6;
- unsupported lifecycle types (`trigger`, `file-trigger`) are refused unless
  the entry is `replaced`;
- timeout is at least 1000 ms and no more than 300000 ms for Goal 6 replay;
- dangerous native environment variables are absent, including `PATH`
  overrides in Goal 6.

The compatibility matrix expansion in Goal 7 can add helper command existence,
version ranges, path conventions, service manager capabilities, SELinux or
AppArmor assumptions, and debconf/package-manager state checks.

## Testing Strategy

Use focused unit tests first, then integration tests that exercise install and
remove planning without requiring host mutation.

Core tests:

- `cargo test -p conary-core target_compatibility`
  - target IDs parse and render as `<format>/<distro>/<release>/<arch>`;
  - `fedora-44` maps to `rpm/fedora/44/<arch>`;
  - `ubuntu-26.04` maps to `deb/ubuntu/26.04/<arch>`;
  - `arch` maps to `arch/arch/rolling/<arch>`;
  - an Arch source bundle with `source_release = None` normalizes to
    `arch/arch/rolling/<arch>`;
  - generic pins such as `fedora` do not fabricate a release when host
    `/etc/os-release` is unavailable or mismatched;
  - unknown target IDs deny raw replay.
- `cargo test -p conary-core scriptlet`
  - legacy execution input derives runtime upgrade/remove args from
    `ExecutionMode` and old/new package versions;
  - `native_invocation.args` contract strings are parsed into runtime argv and
    unsupported or malformed contracts are refused;
  - interpreter args/stdin-contract/env preflight refuses unsupported unsafe
    fields;
  - `PATH` overrides are rejected and the executor supplies a fixed safe path;
  - timeout from bundle entry is used;
  - sandbox floor cannot be lowered by caller.
- `cargo test -p conary-core legacy_replay`
  - `review` and `blocked` entries refuse before execution planning;
  - `review`, `blocked`, and unknown decisions in any bundle entry refuse
    admission even if the entry is outside the current lifecycle;
  - a future-lifecycle `legacy` entry does not execute during the current
    lifecycle but is persisted for later enforcement;
  - `legacy` entries require `--allow-legacy-replay`;
  - `replaced` entries never schedule raw replay;
  - mixed generated-hook/raw-pre ordering refuses when
    `transaction_order` constraints cannot be represented;
  - `foreign_replay_policy = "deny"` rejects foreign target even under
    permissive host policy;
  - `strict`, `guarded`, and `permissive` host policies behave as specified;
  - `--no-scripts` refuses required legacy replay.

Database tests:

- migration v71 creates `installed_legacy_scriptlet_bundles`;
- model round-trips a complete `LegacyScriptletBundle`;
- malformed stored TOML or digest mismatch is rejected;
- malformed stored TOML errors are propagated in remove/upgrade planning rather
  than panicking;
- bundle-level and entry-level unknown `extra` fields survive the installed
  bundle model TOML round trip;
- deleting a trove cascades installed bundle state;
- remove/upgrade can read, decode, and carry the old bundle plan before old
  trove deletion, and post-remove uses the in-memory plan after cascade;
- regression fixture mirrors `remove_inner()`: deleting the trove row must not
  make planned post-remove replay unavailable.

Conary integration tests:

- bundle-aware CCS install with `review` entry fails before DB mutation;
- bundle-aware CCS install with `blocked` entry fails before DB mutation;
- same-source `legacy` fixture fails without `--allow-legacy-replay`;
- same-source `legacy` fixture dry-run passes with the feature gate and does
  not mutate DB;
- same-source `legacy` fixture install with the feature gate persists the
  installed bundle row;
- upgrade from an old legacy-bundle package to a new legacy-bundle package runs
  old pre-remove replay before new install replay and persists the new bundle;
- failed upgrade rolls back the old trove and old installed bundle row;
- remove after deleting the original CCS archive still uses the installed
  bundle row;
- remove of an installed bundle fixture consults the stored bundle;
- batch install refuses review/blocked/required-legacy bundles before file
  storage or pre-install execution;
- collection install/update and automation plan execution propagate replay
  options to every member operation;
- multi-package operations refuse legacy policy failures before partially
  applying earlier members, unless an explicit best-effort mode is documented;
- update refuses legacy replay before creating its update summary changeset;
- state restore refuses review/blocked/required-legacy bundles before
  `run_pre_install_phase()` or `install_inner()`;
- rollback refuses or gates trove removal/restoration with installed legacy
  bundles before deleting/restoring rows;
- cross-distro raw replay is rejected under default strict policy;
- strict host policy rejects foreign replay even when both operator flags are
  supplied;
- `replaced` fixture runs only CCS declarative hooks and does not replay raw
  body;
- `--no-scripts` cannot bypass required legacy replay.

Goal 6 tests that require `decision = "legacy"` entries should use synthetic
bundle fixtures. Goal 4 intentionally did not emit `legacy` decisions from the
conversion pipeline, so Goal 6 should not broaden converter classification just
to make fixtures appear organically. Real corpus promotion to legacy replay can
be handled in a later compatibility/curation goal.

The goal queue's broad `ccs_install` target is superseded by the more specific
`bundle_replay` and `foreign_replay` integration test modules in this design.
Existing `ccs_install` tests may still run as regression coverage. When new
Goal 6 integration tests are files under `apps/conary/tests/`, use cargo's
`--test` form for exact-file verification.

Use test-only runners or injected `LegacyReplayRunner` traits where needed.
Tests should not require actual host mutation or root-only chroot execution.
They should assert the planned calls and persisted state. Separate sandbox
tests can cover `ScriptletExecutor` preflight behavior.

`live_host_safety` remains a regression target for mutation-boundary safety.
Bundle-specific host-policy mapping should live in `target_compatibility` or
`legacy_replay` tests unless the implementation actually adds logic to
`apps/conary/src/live_host_safety.rs`.

Golden fixture plan for Goal 8a:

- no-scriptlet/native-free;
- user/group replaced;
- systemd daemon reload/service preset replaced;
- tmpfiles/sysusers/cache refresh replaced;
- alternatives registration replaced;
- unknown residual legacy replay;
- blocked package-manager recursion;
- foreign replay rejection;
- RPM trigger quarantine;
- DEB trigger/debconf quarantine;
- Arch `.INSTALL` replay.

## File Structure

Likely create:

- `crates/conary-core/src/ccs/legacy_replay.rs`
  - target compatibility and replay planning.
- `crates/conary-core/src/db/models/installed_legacy_scriptlet_bundle.rs`
  - installed bundle persistence model.
- `apps/conary/src/commands/install/legacy_replay.rs`
  - CLI-side integration helpers, runner abstraction, audit formatting.
- `apps/conary/tests/bundle_replay.rs`
  - end-to-end bundle-aware install/remove tests.
- `apps/conary/tests/foreign_replay.rs`
  - target-policy tests.

Likely modify:

- `crates/conary-core/src/ccs/mod.rs`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/scriptlet/mod.rs`
- `crates/conary-core/src/repository/distro.rs`
- `crates/conary-core/src/db/schema.rs`
- `crates/conary-core/src/db/migrations/v41_current.rs`
- `crates/conary-core/src/db/models/mod.rs`
- `apps/conary/src/cli/mod.rs`
- `apps/conary/src/cli/ccs.rs`
- `apps/conary/src/dispatch.rs`
- `apps/conary/src/commands/ccs/install.rs`
- `apps/conary/src/commands/install/mod.rs`
- `apps/conary/src/commands/install/inner.rs`
- `apps/conary/src/commands/install/conversion.rs`
- `apps/conary/src/commands/install/batch.rs`
- `apps/conary/src/commands/install/restore.rs`
- `apps/conary/src/commands/collection.rs`
- `apps/conary/src/commands/remove.rs`
- `apps/conary/src/commands/update.rs`
- `apps/conary/src/commands/state.rs`
- `apps/conary/src/commands/system.rs`
- `apps/conary/src/commands/automation.rs`
- `apps/conary/src/commands/model/apply.rs`
- `apps/conary/src/live_host_safety.rs`
- `docs/modules/ccs.md`
- `docs/modules/source-selection.md`

## Implementation Notes

- Migration v71 must bump `crates/conary-core/src/db/schema.rs::SCHEMA_VERSION`,
  register `migrations::migrate_v71`, and add schema tests that prove upgrade
  from v70 creates `installed_legacy_scriptlet_bundles`.
- Adding `LegacyReplayOptions` to `CcsTransactionInstallOptions` will break the
  current direct struct literals in `apps/conary/src/commands/ccs/install.rs`
  (dry-run and real install) and
  `apps/conary/src/commands/install/conversion.rs`. The implementation plan
  must update those call sites in the same commit as the struct change.
- Adding `LegacyReplayOptions` to `InstallOptions` will break direct struct
  literals in dispatch/update flows, model apply replatform calls, conversion
  install plumbing, and tests. The implementation plan should inventory these
  with `rg "InstallOptions \\{" apps/conary crates` and default both replay
  flags to false at every internal call site.
- `apps/conary/src/commands/install/conversion.rs` must thread replay options
  from the caller into the `CcsTransactionInstallOptions` literal used after a
  native package is converted to CCS.
- `PreparedPackage` and related batch structs must gain fields for accepted
  legacy replay plans and old installed bundle plans; storing only
  `pkg.scriptlets()` is insufficient for bundle-carrying CCS packages because
  `CcsPackage::scriptlets()` is empty.
- Batch, restore, and rollback call paths should get explicit tests because
  they do not all flow through the exact same direct CCS transaction boundary.
- The installed-bundle cascade regression test should mirror the real
  `remove_inner()` pattern: load and plan before `Trove::delete`, then prove
  post-remove execution still has its in-memory plan after the row is gone.

## Design Risks

1. **Ordering between generated CCS hooks and raw legacy entries.**
   Existing hooks are package-level, not entry-linked. The implementation plan
   must use a small phase scheduler when `transaction_order` can be represented
   or fail closed when it cannot.
2. **`ScriptletExecutor` fidelity.**
   The current executor derives runtime args and ignores stdin-contract/env
   fields. Goal 6 must preserve runtime upgrade/remove arguments and must not
   claim exact legacy replay while silently ignoring bundle invocation fields.
3. **Remove trap.**
   If installed bundles are not persisted atomically with the trove, or if
   remove/upgrade try to query the bundle after cascade deletion, remove and
   upgrade cannot enforce the same decisions later.
4. **`--no-scripts` expectations.**
   Existing users may expect script skipping to allow installs. For converted
   bundles, this would install known-incomplete semantics. The implementation
   should use crisp errors.
5. **Target identity drift.**
   Distro IDs like `fedora-44` and bundle source fields like
   `source_distro = "fedora", source_release = "44"` must normalize to the
   same target ID.
6. **Feature-gate propagation.**
   The flags must reach direct install, converted install, update,
   collection/group operations, batch, automation, model-apply, remove,
   autoremove, restore, and rollback paths consistently. Missing one path
   creates either an accidental bypass or an accidental refusal.
7. **Bundle storage size.**
   Storing full bundle TOML in SQLite is acceptable for Goal 6, but packages
   with many native trigger entries can produce large rows. Future goals can
   normalize or compress this state if installed-bundle queries become hot.

## Verification

Expected implementation verification:

```bash
cargo test -p conary ccs_install
cargo test -p conary-core scriptlet
cargo test -p conary-core legacy_replay
cargo test -p conary-core target_compatibility
cargo test -p conary --test bundle_replay
cargo test -p conary --test foreign_replay
cargo test -p conary live_host_safety
cargo test -p conary remove
cargo test -p conary update
cargo test -p conary batch
cargo test -p conary restore
cargo test -p conary rollback
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Review Questions

Ask implementation-readiness reviewers to check:

1. Is the explicit feature gate (`--allow-legacy-replay`) the right default, or
   should accepted same-source replay be enabled by bundle state alone?
2. Are `--no-scripts` semantics strict enough, or too strict, for converted
   bundles with required legacy entries?
3. Is the installed bundle table sufficient for remove and upgrade, including
   old bundle planning before old trove deletion and post-remove execution from
   in-memory state after cascade?
4. Should the first implementation add a phase scheduler for CCS hooks and raw
   legacy entries, and does it fail closed when `transaction_order` cannot be
   represented?
5. Are target ID normalization rules precise enough for current
   `fedora-44`, `ubuntu-26.04`, `arch`, and generic `fedora` identities?
6. Does the foreign replay matrix correctly combine bundle policy and host
   policy?
7. Are there CLI/internal call sites missing from the feature-gate propagation
   list?
8. Can `ScriptletExecutor` support native invocation args, interpreter args,
   stdin contracts, environment, and timeout without unsafe shortcuts,
   especially runtime upgrade/remove arguments?
9. Do dry-run tests prove preflight behavior without depending on root/chroot?
10. Does any path allow `review`, `blocked`, or unknown bundle entries to
    mutate DB or files before refusal?
11. Are batch, restore, and rollback paths fully covered by the same gate, or
    do any bypass direct CCS install preflight?
12. Do multi-package operations preflight all candidate packages before the
    first mutation, so legacy replay refusals cannot leave partial state?
