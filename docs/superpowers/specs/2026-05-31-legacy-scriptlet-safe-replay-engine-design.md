# Legacy Scriptlet Safe Replay Engine Design

## Summary

Goal 6 is the first client-side consumption point for the passive legacy
scriptlet bundle work. Goals 1 through 5 created the bundle schema, extracted
native ABI evidence, classified commands, embedded bundles in converted CCS
archives, and prevented Remi from publicly serving non-public converted
artifacts. Goal 6 makes `conary install`, `conary ccs install`, update, remove,
and restore-aware install paths read those bundles and enforce their decisions
before any live mutation.

The work is safety-first. A package that has `review` or `blocked` bundle
entries must fail before hooks, file deployment, DB writes, generation
publication, or remove mutation. A package that needs raw native scriptlet
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
- `apps/conary/src/commands/remove.rs`
- `apps/conary/src/commands/update.rs`
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
  `interpreter_args`, `stdin`, or entry-specific sandbox floors.

## Scope

Goal 6 includes:

- local install/update/remove preflight that consumes
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
- continued execution of generated CCS hooks for `replaced` entries;
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
   `review` and `blocked` entries fail here. So do malformed bundles and target
   compatibility states that are `review-required`, `blocked`, or unknown.
2. **Execution gate:** Is raw native replay allowed for this accepted entry at
   this lifecycle point? `legacy` entries require the local feature gate,
   compatible target metadata, sandbox preflight, and native-compatible
   invocation arguments. `replaced` entries do not replay raw script bodies.

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
- `conary update`;
- `conary remove`;
- `conary autoremove`;
- `conary ccs install`.

Internal callers, including model apply, batch install, conversion install, and
restore paths, should carry the same option struct and default both flags to
false. Automation must opt in intentionally.

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
| `replaced` | Never replay the raw body. Let the corresponding CCS declarative hooks/effects run through existing CCS hook paths. |
| `legacy` | Replay the raw body only if admission, feature gate, target compatibility, and sandbox/native invocation preflight all pass. |
| `review` | Refuse the package operation before mutation. |
| `blocked` | Refuse the package operation before mutation. |
| unknown | Refuse the package operation before mutation. |

The no-double-application invariant is absolute: if an entry is `replaced`, its
body is not executed. If an entry is `legacy`, only that entry's raw body is
executed. Generated hooks may still run for different `replaced` entries in the
same phase, but the planner must never schedule a raw body and a generated hook
for the same entry ID.

Because current CCS hooks are phase-level rather than entry-linked, Goal 6
should record an implementation limitation: it can prevent raw replay for
`replaced` entries, but it cannot yet interleave generated hooks and legacy
entry replay by `transaction_order` at sub-phase precision. The initial order
is:

1. pre-mutation legacy pre entries whose decision is `legacy`;
2. existing CCS pre-hooks;
3. DB/CAS/generation transaction;
4. existing CCS post-hooks;
5. post-commit legacy post entries whose decision is `legacy`;
6. triggers.

If this order is judged too broad during implementation review, the plan should
move generated-hook execution behind a small phase scheduler instead of adding
ad hoc ordering in `install_ccs_package_transactionally()`.

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
install/remove through argv rather than a separate scriptlet slot. The replay
runner should prefer `entry.native_invocation.args` when the bundle provides
it. If that list is empty, derive arguments from `ScriptletExecutor`'s existing
package-format `ExecutionMode` rules.

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
the transaction commits. For upgrade, delete of the old trove should cascade
the old bundle only after old pre-remove preflight and execution have had a
chance to read the installed old bundle.

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
- select entries for a lifecycle event;
- reject review/blocked/unknown entries;
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
    pub body: Cow<'a, str>,
    pub body_encoding: Option<&'a str>,
    pub native_args: &'a [String],
    pub native_environment: &'a [String],
    pub stdin: Option<&'a str>,
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
- use `native_invocation.args` when present, otherwise derive args from
  `ExecutionMode`;
- parse `native_invocation.environment` as `KEY=VALUE`, reject malformed names,
  and reject dangerous environment keys such as `LD_PRELOAD`, `LD_LIBRARY_PATH`,
  `BASH_ENV`, `ENV`, and `PYTHONPATH` unless a later capability design permits
  them;
- support `stdin` by piping it to the child when present instead of always
  using `Stdio::null()`;
- reject `native_invocation.chroot` in Goal 6 unless it is empty or `/`,
  because Conary already controls the target root boundary;
- keep protected live-root sandbox setup failures fatal.

If the implementation cost of interpreter args/stdin/env support is too high
for the first slice, the plan may split executor work into two commits. It must
not silently ignore fields from a `legacy` entry and still claim exact replay.
Ignoring an unsupported native invocation field should be a preflight refusal.

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
10. run existing CCS pre-hooks for replaced behavior;
11. execute the DB/CAS/generation transaction and persist the accepted bundle;
12. run existing CCS post-hooks;
13. run post-commit legacy entries that are planned for raw replay;
14. append scriptlet warning/audit metadata;
15. run triggers and finish.

The exact placement of steps 9 and 10 should be revisited during implementation
review against fixtures. The invariant is that all preflight happens before any
pre-hook or raw pre script mutates state.

`cmd_install --convert-to-ccs` and repository install paths that receive a
converted CCS package should use the same `install_ccs_package_transactionally`
path. Native package installs that do not have a `legacy_scriptlets` bundle
continue using existing flattened scriptlet execution.

## Remove And Upgrade Integration

Remove must not rely on the original CCS archive. It should look up
`InstalledLegacyScriptletBundle` by trove ID before mutation:

1. select the trove as today;
2. load installed legacy bundle row, if any;
3. resolve target and host policy;
4. run bundle admission and preflight for `pre-remove` and `post-remove`;
5. if dry-run support exists for the operation, report decisions and return;
6. execute planned legacy `pre-remove` entries before DB deletion;
7. perform current DB/generation removal;
8. execute planned legacy `post-remove` entries after publication;
9. append warnings/audit metadata.

For upgrade, the new package bundle and the old installed bundle both matter:

- new bundle: preflight and execute new pre/post install or upgrade entries;
- old installed bundle: preflight and execute old pre/post remove entries in
  upgrade-removal mode before old trove deletion cascades the old bundle row;
- the new bundle is persisted with the new trove in the same transaction.

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
        "timeout_ms": 60000
      }
    ]
  }
}
```

For refused dry-run or preflight failures, the error text is enough for CLI
users, but tests should assert no DB mutation occurred. For real operations
that pass preflight but produce post-commit scriptlet failures, use the existing
scriptlet warning metadata shape and include the bundle entry ID in the warning
message.

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
- `native_invocation.chroot` is absent or `/`;
- unsupported lifecycle types (`trigger`, `file-trigger`) are refused unless
  the entry is `replaced`;
- timeout is non-zero and within bundle validation limits;
- dangerous native environment variables are absent.

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
  - unknown target IDs deny raw replay.
- `cargo test -p conary-core scriptlet`
  - legacy execution input preserves native args;
  - interpreter args/stdin/env preflight refuses unsupported unsafe fields;
  - timeout from bundle entry is used;
  - sandbox floor cannot be lowered by caller.
- `cargo test -p conary-core legacy_replay`
  - `review` and `blocked` entries refuse before execution planning;
  - `legacy` entries require `--allow-legacy-replay`;
  - `replaced` entries never schedule raw replay;
  - `foreign_replay_policy = "deny"` rejects foreign target even under
    permissive host policy;
  - `strict`, `guarded`, and `permissive` host policies behave as specified;
  - `--no-scripts` refuses required legacy replay.

Database tests:

- migration v71 creates `installed_legacy_scriptlet_bundles`;
- model round-trips a complete `LegacyScriptletBundle`;
- malformed stored TOML or digest mismatch is rejected;
- deleting a trove cascades installed bundle state;
- upgrade can read old bundle before old trove deletion.

Conary integration tests:

- bundle-aware CCS install with `review` entry fails before DB mutation;
- bundle-aware CCS install with `blocked` entry fails before DB mutation;
- same-source `legacy` fixture fails without `--allow-legacy-replay`;
- same-source `legacy` fixture dry-run passes with the feature gate and does
  not mutate DB;
- same-source `legacy` fixture install with the feature gate persists the
  installed bundle row;
- remove of an installed bundle fixture consults the stored bundle;
- cross-distro raw replay is rejected under default strict policy;
- `replaced` fixture runs only CCS declarative hooks and does not replay raw
  body;
- `--no-scripts` cannot bypass required legacy replay.

Use test-only runners or injected `LegacyReplayRunner` traits where needed.
Tests should not require actual host mutation or root-only chroot execution.
They should assert the planned calls and persisted state. Separate sandbox
tests can cover `ScriptletExecutor` preflight behavior.

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
- `apps/conary/src/commands/remove.rs`
- `apps/conary/src/commands/update.rs`
- `apps/conary/src/commands/automation.rs`
- `apps/conary/src/commands/model/apply.rs`
- `apps/conary/src/live_host_safety.rs`
- `docs/modules/ccs.md`
- `docs/modules/source-selection.md`

## Design Risks

1. **Ordering between generated CCS hooks and raw legacy entries.**
   Existing hooks are package-level, not entry-linked. The implementation plan
   must either accept the coarse order in this design or introduce a small
   phase scheduler.
2. **`ScriptletExecutor` fidelity.**
   The current executor derives args and ignores stdin/env fields. Goal 6 must
   not claim exact legacy replay while silently ignoring bundle invocation
   fields.
3. **Remove trap.**
   If installed bundles are not persisted atomically with the trove, remove and
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
   The flags must reach direct install, converted install, update, batch,
   model-apply, remove, autoremove, and restore paths consistently. Missing one
   path creates either an accidental bypass or an accidental refusal.

## Verification

Expected implementation verification:

```bash
cargo test -p conary ccs_install
cargo test -p conary-core scriptlet
cargo test -p conary-core legacy_replay
cargo test -p conary-core target_compatibility
cargo test -p conary bundle_replay
cargo test -p conary foreign_replay
cargo test -p conary live_host_safety
cargo test -p conary remove
cargo test -p conary update
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
   old bundle lookup before old trove deletion?
4. Should the first implementation add a phase scheduler for CCS hooks and raw
   legacy entries, or is the coarse phase order acceptable for Goal 6?
5. Are target ID normalization rules precise enough for current
   `fedora-44`, `ubuntu-26.04`, and `arch` identities?
6. Does the foreign replay matrix correctly combine bundle policy and host
   policy?
7. Are there CLI/internal call sites missing from the feature-gate propagation
   list?
8. Can `ScriptletExecutor` support native invocation args, interpreter args,
   stdin, environment, and timeout without unsafe shortcuts?
9. Do dry-run tests prove preflight behavior without depending on root/chroot?
10. Does any path allow `review`, `blocked`, or unknown bundle entries to
    mutate DB or files before refusal?
