# Legacy Scriptlet Compatibility Matrix And Override Audit Design

## Summary

Goal 7 tightens the target-compatibility and audit layer around the safe legacy
scriptlet replay engine that landed in Goal 6. Goal 6 made the Conary client
consume passive legacy scriptlet bundles, fail closed for unsafe entries,
persist accepted bundles locally, and gate raw replay behind explicit operator
flags. Goal 7 does not broaden raw replay. It adds the missing compatibility
ratchet: a package marked `family-compatible` is accepted only when an explicit
compatibility matrix entry authorizes the source target to replay on the host
target and the entry's shallow preflight checks pass.

The production matrix starts empty and conservative. Goal 7 tests may inject
synthetic matrix entries to prove the machinery, but the implementation must not
claim Fedora-to-Fedora, Ubuntu-to-Debian, Arch-to-Arch, or any other real
same-family portability until separate evidence exists. This goal is about
making compatibility explicit, auditable, and fail-closed.

Goal 7 also extends the existing changeset `legacy_scriptlet_replay` metadata
so accepted replay plans record the compatibility decision, matrix entry,
preflight results, and operator override status. Refused operations still fail
before mutation and do not create changeset rows merely to log the refusal; the
refusal message must carry stable reason IDs. Overrides that cross a mutation
boundary must be recorded in changeset metadata.

The primary implementation consequence is intentional: existing
`family-compatible` tests that passed in Goal 6 must now inject a synthetic
matrix entry or expect `compatibility-matrix-entry-missing`.

## Source Context

Read these first when implementing:

- `AGENTS.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/specs/archive/2026-05-31-legacy-scriptlet-safe-replay-engine-design.md`
- `docs/superpowers/plans/archive/2026-05-31-legacy-scriptlet-safe-replay-engine-plan.md`
- `crates/conary-core/src/ccs/legacy_replay.rs`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/repository/distro.rs`
- `crates/conary-core/src/scriptlet/mod.rs`
- `crates/conary-core/src/db/models/installed_legacy_scriptlet_bundle.rs`
- `apps/conary/src/commands/changeset_metadata.rs`
- `apps/conary/src/commands/install/mod.rs`
- `apps/conary/src/commands/remove.rs`
- `apps/conary/src/commands/update.rs`
- `apps/conary/src/live_host_safety.rs`
- `apps/conary/tests/bundle_replay.rs`
- `apps/conary/tests/foreign_replay.rs`

Relevant current code facts:

- `crates/conary-core/src/ccs/legacy_replay.rs` already exposes the pure
  `plan_legacy_replay()` entry point, `LegacyReplayPolicyInput`,
  `LegacyReplayPreflight`, `LegacyReplayPlan`, and typed refusal kinds.
- Goal 6 currently treats `TargetCompatibility::FamilyCompatible` as
  acceptable once bundle policy, host policy, and operator override checks pass.
  It does not require a matrix entry.
- Goal 6 production planning currently passes `source_target_from_bundle(...)`
  back into `LegacyReplayPolicyInput.target` in install, remove, and rollback
  helpers. That makes the host target equal the source target and can bypass
  cross-distro policy checks. Goal 7 must fix this integration bug by resolving
  the host target from the current system policy instead of using the bundle's
  source target as a stand-in.
- `TargetCompatibility::SourceNative` already requires the host target to match
  `source_target_from_bundle()` or appear in `bundle.allowed_targets`.
- `ForeignReplayPolicy` and `HostForeignReplayPolicy` already enforce deny,
  guarded, and permissive behavior before raw replay.
- `repository/distro.rs` already builds replay target IDs in the form
  `<format>/<distro>/<release>/<arch>`, including `arch/arch/rolling/<arch>`.
- `LegacyReplayAudit` in `apps/conary/src/commands/changeset_metadata.rs`
  records target ID, source target ID, target compatibility, bundle policy,
  host policy, feature gate, foreign override flag, evidence digest, planned
  entries, and outcomes.
- Install and remove paths already append that audit object to changeset
  metadata for accepted bundle-carrying operations.
- Refused install/remove/update operations intentionally fail before mutation,
  so they usually have no changeset row to carry metadata.
- No current model stores a reusable target-compatibility matrix entry or
  preflight check result.
- Existing `family-compatible` tests in `apps/conary/tests/foreign_replay.rs`
  and `crates/conary-core/src/ccs/legacy_replay.rs` construct bundles without a
  matrix entry. Goal 7 must update those tests to inject synthetic entries for
  the source/host pairs they intend to accept.

## Scope

Goal 7 includes:

- a pure compatibility matrix model in `conary-core`;
- matrix-aware target compatibility evaluation in the existing legacy replay
  planner;
- stable refusal kinds and reason codes for matrix and shallow preflight
  failures;
- synthetic test matrix entries proving same-family replay requires explicit
  authorization;
- shallow helper, path, service-manager, security-policy, and sandbox preflight
  checks driven by matrix entry requirements;
- richer accepted-plan audit metadata for compatibility decisions, matrix
  entries, preflight checks, and operator overrides;
- install, remove, update, batch, restore, autoremove, and rollback behavior
  remaining fail-closed through the existing Goal 6 integration points;
- docs that state converted CCS format does not imply raw scriptlet
  portability.

Goal 7 excludes:

- real production compatibility entries for Fedora, Ubuntu, Debian, Arch, or
  any derivative;
- helper command version probing beyond explicitly injected/testable evidence;
- new adapter coverage or command translation;
- golden native-vs-CCS behavior fixtures;
- trigger, file-trigger, debconf, purge, or abort-mode replay expansion;
- Remi publication, promotion, curation, or review artifact changes;
- database migrations for matrix storage;
- persistent operator-override policy;
- logging refused preflight attempts by creating synthetic changesets;
- treating `allowed_targets` as a substitute for a `family-compatible` matrix
  entry.

## Design Principles

### Compatibility Is Explicit

`family-compatible` is a claim that a package can replay raw native scriptlets
on a host target that does not exactly match the source target. Goal 7 requires
that claim to be backed by a matrix entry. The bundle value alone is not enough.

### Production Starts Empty

The default production matrix has no real compatibility entries. This means
Goal 7 will make current behavior stricter for `family-compatible` bundles:
they now refuse with a matrix-missing reason unless a caller injects or configures
an explicit matrix entry. Tests can inject entries to prove the code path.

### Shallow Checks Only

Goal 7 may check that a required helper command name is available, that a path
exists or is declared compatible, that the service manager matches, that a
security policy is unsupported, or that the sandbox floor is reachable. It must
not infer deep helper semantics, parse arbitrary command version output, or
claim that helper behavior matches a source package manager. Those belong to
future compatibility/golden-fixture work.

### Audit Only After Mutation Boundary Acceptance

Successful or post-commit degraded operations that cross a mutation boundary
must record compatibility and override details in changeset metadata. Refused
operations must fail before mutation and report stable reason IDs in the error,
but they must not create changesets solely to record an attempted override.

## Compatibility Matrix Model

Add a new core model, preferably in
`crates/conary-core/src/ccs/target_compatibility.rs` or a focused sibling of
`legacy_replay.rs`.

Logical types:

```rust
pub struct TargetCompatibilityMatrix {
    entries: Vec<TargetCompatibilityMatrixEntry>,
}

pub struct TargetCompatibilityMatrixEntry {
    pub id: String,
    pub source: TargetSelector,
    pub target: TargetSelector,
    pub requirements: MatrixPreflightRequirements,
    pub digest: Option<String>,
    pub rationale: String,
}

pub struct TargetSelector {
    pub format: String,
    pub distro: String,
    pub release: TargetSelectorRelease,
    pub arch: TargetSelectorArch,
}

pub enum TargetSelectorRelease {
    Exact(String),
    Any,
}

pub enum TargetSelectorArch {
    Exact(String),
    Any,
}
```

The implementation may choose a slightly different Rust shape, but the behavior
must remain:

- a matrix entry matches a concrete source target and concrete host target;
- exact values must match exactly;
- wildcard release or architecture is allowed only when the entry says so;
- matching is deterministic and returns at most one winning entry after
  specificity is considered;
- exact selectors are more specific than `Any`; entries with more exact
  dimensions win over wildcard entries;
- two matching entries with the same specificity are ambiguous;
- duplicate or same-specificity overlapping entries are rejected by
  `TargetCompatibilityMatrix::new(...) -> Result<Self, ...>`;
- the planner should still treat an ambiguous runtime match as
  `compatibility-matrix-entry-ambiguous` rather than panicking, so an invalid
  matrix cannot turn into an unsafe accept;
- entry IDs are stable strings suitable for audit metadata;
- the matrix can compute a deterministic digest from its entries for audit,
  independent of caller insertion order.

Two target selectors overlap when their `format` and `distro` match exactly and
their `release` and `arch` dimensions are either identical or one side is
`Any`. Two matrix entries overlap when both their source selectors and target
selectors overlap. Specificity is the count of exact release and architecture
selectors across the source and target sides. If overlapping entries have the
same specificity, the constructor must reject them. If overlapping entries have
different specificity, the more specific entry wins for matching concrete
targets.

The production constructor should be visibly conservative, for example:

```rust
TargetCompatibilityMatrix::production_default()
```

and that default must contain no real same-family entries in Goal 7.

Tests should use:

```rust
TargetCompatibilityMatrix::for_testing(vec![...])
```

or equivalent to inject synthetic entries. The test constructor should validate
eagerly and panic on invalid selectors, duplicate IDs, or ambiguous overlaps so
bad test matrices fail at fixture construction.

Matrix entry selectors use the same normalized release names as
`source_target_from_bundle()`. For Arch, use `rolling` as the release value. A
bundle with `source_distro = "arch"` and `source_release = None` normalizes to
`arch/arch/rolling/<arch>` and must match
`TargetSelectorRelease::Exact("rolling")`.

## Matrix Entry Requirements

The matrix entry requirements should describe only shallow preflight inputs:

```rust
pub struct MatrixPreflightRequirements {
    pub required_helpers: Vec<RequiredHelper>,
    pub required_paths: Vec<RequiredPath>,
    pub service_manager: Option<ServiceManagerRequirement>,
    pub security_policy: Option<SecurityPolicyRequirement>,
    pub sandbox_floor: Option<SandboxMode>,
}
```

The preflight environment should also have an explicit shape:

```rust
pub struct CompatibilityPreflightEnvironment {
    pub helpers: Vec<ObservedHelper>,
    pub paths: Vec<ObservedPath>,
    pub service_manager: Option<String>,
    pub security_policies: Vec<String>,
    pub effective_sandbox: SandboxMode,
}

pub struct ObservedHelper {
    pub name: String,
    pub version: Option<String>,
}

pub struct ObservedPath {
    pub path: String,
    pub present: bool,
}
```

The implementation may prefer maps or sets for efficient lookup, but the data
must remain deterministic and serializable for tests. Production Goal 7 callers
should use an empty helper/path/security-policy environment plus the effective
sandbox mode already chosen by the CLI path.

Expected checks:

- `required_helpers`: verify the helper is declared present in the preflight
  environment. Version constraints may exist as data, but Goal 7 should only
  enforce them when version evidence is explicitly provided by the test or
  caller. Missing version evidence for a version-constrained helper should
  refuse with a stable reason rather than shelling out.
- `required_paths`: verify a path is declared present or compatible in the
  preflight environment. Do not walk the live filesystem directly from the core
  planner.
- `service_manager`: compare declared host service manager, such as `systemd`
  or `none`.
- `security_policy`: fail closed for unsupported policy requirements such as
  SELinux or AppArmor assumptions unless explicitly declared compatible by the
  preflight environment.
- `sandbox_floor`: compare against the requested/effective sandbox mode already
  used by Goal 6.

The core planner should receive an injected preflight environment rather than
performing host I/O. Goal 7 production integration should pass an explicit
empty or conservative fact set. Host fact discovery can be added by a follow-up
goal once real matrix entries exist.

## Reason IDs And Refusals

Goal 7 should extend `LegacyReplayRefusalKind` with matrix-specific variants.
This keeps the planner consistent with Goal 6's typed refusal pattern and avoids
tests matching arbitrary strings. Each new variant should expose a stable
`reason_code()` value for audit/error rendering.

Required new refusal kinds:

- `CompatibilityMatrixEntryMissing`
- `CompatibilityMatrixEntryAmbiguous`
- `CompatibilityHelperMissing`
- `CompatibilityHelperVersionMissing`
- `CompatibilityHelperVersionUnsupported`
- `CompatibilityPathMissing`
- `CompatibilityServiceManagerMismatch`
- `CompatibilitySecurityPolicyUnsupported`
- `CompatibilitySandboxFloorUnsupported`

Required stable reason codes:

- `compatibility-matrix-entry-missing`
- `compatibility-matrix-entry-ambiguous`
- `compatibility-helper-missing`
- `compatibility-helper-version-missing`
- `compatibility-helper-version-unsupported`
- `compatibility-path-missing`
- `compatibility-service-manager-mismatch`
- `compatibility-security-policy-unsupported`
- `compatibility-sandbox-floor-unsupported`

These are compatibility preflight failures, not adapter classification reasons.
They should not be mixed with blocked-class IDs from the conversion adapter
registry.

Error messages should include:

- package/source target ID when available;
- host target ID;
- reason code;
- matrix entry ID if one matched;
- the first failing requirement ID or name.

Error messages must not include local filesystem cache paths, CCS archive
paths, or review artifact paths.

## Planner Integration

Extend `LegacyReplayPolicyInput` to carry optional compatibility inputs:

```rust
pub struct LegacyReplayPolicyInput<'a> {
    pub replay_enabled: bool,
    pub foreign_replay_override: bool,
    pub no_scripts: bool,
    pub requested_sandbox_mode: SandboxMode,
    pub host_policy: HostForeignReplayPolicy,
    pub target: ReplayTarget<'a>,
    pub compatibility_matrix: TargetCompatibilityMatrix,
    pub compatibility_environment: CompatibilityPreflightEnvironment,
}
```

The matrix and preflight environment are intentionally owned by the policy input
rather than borrowed. This keeps the lifetime parameter tied only to
`ReplayTarget<'a>` and avoids spreading matrix/environment lifetimes into other
core structs. The important behavior is that every call site uses the same
production-default matrix and an explicit preflight environment, while tests can
inject synthetic matrix entries and environment facts.

Update target compatibility evaluation:

- `source-native`: accepted only when `target_id == source_target_id` or the
  target appears in `bundle.allowed_targets`. Goal 7 does not change this.
- `conary-portable`: passes target compatibility without a matrix entry because
  it claims raw replay is not needed. If a `conary-portable` bundle nonetheless
  carries selected `legacy` entries, those entries still require the raw replay
  feature gate and foreign replay policy checks, but they do not additionally
  require a matrix entry. This is a bundle-contradiction scenario that
  conversion policy should prevent.
- `family-compatible`: require a matching matrix entry. If no entry matches,
  refuse with `compatibility-matrix-entry-missing`. If multiple entries match,
  refuse with `compatibility-matrix-entry-ambiguous`. If one entry matches, run
  its shallow preflight checks before evaluating host/foreign override policy.
- `review-required`, `blocked`, and unknown compatibility remain refused before
  matrix lookup.

The matrix check should run before the foreign override check. Otherwise an
operator could be prompted for an override that still cannot pass compatibility
preflight.

The `target` field in `LegacyReplayPolicyInput` is the host target, not the
bundle source target. Production call sites must not construct it with
`source_target_from_bundle(bundle)`. Add a CLI-side helper that derives the host
target from `DistroPin::get_current(conn)` and
`replay_target_from_distro_id(pin.distro, std::env::consts::ARCH)`. If there is
no pin, the pin is unparsable, or host detection is unavailable, fall back to a
conservative `unknown/unknown/unknown/<arch>` target and strict host policy.
This may refuse more packages, but it must not silently turn a foreign replay
into a source-native one. The source target should still be computed from the
bundle for comparison and audit.

## Audit Metadata

Extend `LegacyReplayPlan` with a compatibility decision object:

```rust
pub struct LegacyReplayCompatibilityDecision {
    pub decision: String,
    pub reason_code: String,
    pub matrix_entry_id: Option<String>,
    pub matrix_digest: Option<String>,
    pub preflight_checks: Vec<LegacyReplayPreflightCheck>,
    pub override_required: bool,
    pub override_used: bool,
}
```

Add this as a required field on `LegacyReplayPlan`:

```rust
pub struct LegacyReplayPlan {
    pub target_id: String,
    pub source_target_id: String,
    pub bundle_evidence_digest: Option<String>,
    pub lifecycle_entries: Vec<PlannedLegacyEntry>,
    pub sandbox_floor: SandboxMode,
    pub ccs_hooks_allowed: bool,
    pub raw_replay_required: bool,
    pub compatibility_decision: LegacyReplayCompatibilityDecision,
}
```

Constructor sites that must be updated include
`crates/conary-core/src/ccs/legacy_replay.rs::build_plan()`,
`apps/conary/src/commands/install/mod.rs::test_legacy_plan()`,
`apps/conary/src/commands/install/batch.rs::test_legacy_plan()`,
`apps/conary/src/commands/install/restore.rs::test_legacy_plan()`, and the
test helper literal in `apps/conary/src/commands/remove.rs`.

Extend changeset metadata under the existing `legacy_scriptlet_replay` object.
Do not add a second top-level object.

Add a compatibility sub-object to `LegacyReplayAudit` with serde defaults:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct LegacyReplayCompatibilityAudit {
    pub decision: String,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_digest: Option<String>,
    pub override_required: bool,
    pub override_used: bool,
    #[serde(default)]
    pub preflight_checks: Vec<LegacyReplayPreflightCheckAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LegacyReplayAudit {
    pub bundle_present: bool,
    pub target_id: String,
    pub source_target_id: String,
    pub target_compatibility: String,
    pub foreign_replay_policy: String,
    pub host_policy: String,
    pub feature_gate: String,
    pub foreign_override: bool,
    pub evidence_digest: Option<String>,
    #[serde(default)]
    pub compatibility: LegacyReplayCompatibilityAudit,
    #[serde(default)]
    pub planned_entries: Vec<LegacyReplayPlannedEntryAudit>,
}
```

`LegacyReplayCompatibilityAudit::default()` should represent older metadata
that had no compatibility object, for example `decision = "unknown"` and
`reason_code = "compatibility-audit-unavailable"`. Add a unit test proving old
Goal 6 metadata JSON without the compatibility field still deserializes.

Suggested JSON shape:

```json
{
  "legacy_scriptlet_replay": {
    "bundle_present": true,
    "target_id": "rpm/fedora/44/x86_64",
    "source_target_id": "rpm/fedora/45/x86_64",
    "target_compatibility": "family-compatible",
    "foreign_replay_policy": "guarded",
    "host_policy": "guarded",
    "feature_gate": "enabled",
    "foreign_override": true,
    "evidence_digest": "sha256:...",
    "compatibility": {
      "decision": "accepted",
      "reason_code": "compatibility-matrix-entry-accepted",
      "matrix_entry_id": "test-fedora45-to-fedora44",
      "matrix_digest": "sha256:...",
      "override_required": true,
      "override_used": true,
      "preflight_checks": [
        {
          "id": "helper-systemctl",
          "kind": "helper",
          "status": "passed",
          "reason_code": "compatibility-helper-present"
        }
      ]
    },
    "planned_entries": []
  }
}
```

`preflight_checks` must avoid leaking local paths. A path requirement may record
the policy path from the matrix, such as `/usr/bin/systemctl`, but must not
record cache roots, temporary extracted CCS paths, review artifact paths, or
host-local scratch paths.

When no raw replay is required and no matrix entry was needed, the audit should
still include a compatibility object with an accepted/source-native or
native-free reason code. This keeps query/history rendering predictable.

`LegacyReplayAuditContext` in install and the remove audit context must carry
the compatibility decision from `LegacyReplayPlan` through to
`LegacyReplayAudit`. If no plan exists for the current lifecycle but a bundle is
present, use a native-free or source-native compatibility decision derived from
the admission result.

## Operator Semantics

Goal 7 keeps the Goal 6 CLI flags:

- `--allow-legacy-replay`
- `--allow-foreign-legacy-replay`
- `--no-scripts`

It does not add a new persistent override command. A future goal may add a
curated operator policy file or admin-managed compatibility matrix, but Goal 7
should not invent it.

Behavior by policy:

- Strict host policy refuses foreign raw replay even with a matrix entry and
  operator override.
- Guarded host policy accepts foreign raw replay only when bundle policy is
  `guarded` or stronger, an explicit matrix entry matches, all preflight checks
  pass, `--allow-legacy-replay` is present, and
  `--allow-foreign-legacy-replay` is present.
- Permissive host policy still requires explicit replay and foreign replay
  flags. It accepts `permissive` bundle policy only after matrix and preflight
  checks pass.

`--no-scripts` continues to refuse any lifecycle that requires raw legacy
replay. It does not bypass matrix checks or convert raw replay into a skipped
operation.

## Integration Points

### Core

Add the matrix model and preflight environment in `conary-core`. The core
planner should remain pure and deterministic. Tests should not depend on the
developer host's `/etc/os-release`, PATH, service manager, SELinux state, or
filesystem layout.

### CLI

Every call site that builds `LegacyReplayPolicyInput` must pass a compatibility
policy provider containing the production matrix and an explicit preflight
environment. Production dispatch should pass the empty production matrix and a
conservative environment. Tests may inject synthetic matrix entries through
internal options or helper constructors, but Goal 7 must not add a user-facing
CLI flag or persistent local policy file for matrix injection.

Add shared helpers so the production call sites in install, remove, rollback,
and update-adjacent helpers do not duplicate policy literals:

- a core helper or constructor for the conservative production matrix and
  preflight environment;
- a CLI-side helper that resolves the current host replay target and
  `HostForeignReplayPolicy` from the active distro pin;
- a policy-input builder that combines operator flags, sandbox mode, host
  target, host policy, production matrix, and production preflight environment.

The implementation should include regression coverage proving the helper does
not use `source_target_from_bundle(bundle)` as the host target for install,
remove, or rollback planning.

Process-based integration tests may need to inject a synthetic matrix into the
`conary` binary. If so, use a test-harness-only channel such as
`CONARY_TEST_COMPATIBILITY_MATRIX_JSON` or `__CONARY_TEST_MATRIX_JSON`, gate it
behind `cfg(debug_assertions)` plus the existing
`CONARY_TEST_SKIP_GENERATION_MOUNT=1` integration-test marker or an equivalent
test-only guard, and keep it out of user-facing help/docs. Invalid injected JSON
must fail closed. The production release binary must continue to use the empty
production matrix unless a future goal adds an explicit operator policy
mechanism.

### Install/Update/Remove/Batch/Restore/Autoremove/Rollback

Goal 7 should reuse Goal 6 integration points. It should not add a second
preflight stage after mutation. If a matrix refusal occurs, it must occur at the
same pre-mutation boundaries that Goal 6 already established.

### History And Query Surfaces

`conary query history` or related history renderers may continue to display the
raw metadata JSON indirectly, but docs should define the metadata shape so
future query polish can expose it cleanly. Goal 7 does not need a new CLI query
subcommand.

## Testing Strategy

Core unit tests:

- `family-compatible` bundle without a matrix entry refuses with
  `compatibility-matrix-entry-missing`.
- Production default matrix has no entries.
- `TargetCompatibilityMatrix::new(...)` rejects duplicate IDs and
  same-specificity overlapping entries.
- If an invalid matrix reaches the planner, duplicate matching entries refuse
  with `compatibility-matrix-entry-ambiguous` instead of accepting.
- Matching synthetic matrix entry accepts only when all shallow requirements
  pass.
- Missing helper refuses with `compatibility-helper-missing`.
- Missing helper version evidence refuses with
  `compatibility-helper-version-missing` when the matrix entry requires a
  version.
- Unsupported helper version refuses with
  `compatibility-helper-version-unsupported`.
- Missing path refuses with `compatibility-path-missing`.
- Service manager mismatch refuses with
  `compatibility-service-manager-mismatch`.
- Security policy requirement refuses with
  `compatibility-security-policy-unsupported` unless declared compatible.
- Sandbox floor mismatch refuses with
  `compatibility-sandbox-floor-unsupported`.
- Strict host policy refuses foreign replay even when a matrix entry matches
  and the operator supplied both flags.
- Existing same-family foreign replay tests inject a synthetic matrix entry
  before expecting host-policy refusals such as
  `ForeignReplayDeniedByHostPolicy` or
  `ForeignReplayOverrideRequired`; otherwise the missing matrix refusal is the
  correct first failure.
- Guarded/permissive paths require both replay flags and record the override in
  the accepted plan.
- Host target resolution uses the distro pin or a conservative unknown target,
  never the bundle's source target, in production install/remove/rollback
  planning.

CLI/integration tests:

- CCS install with a synthetic `family-compatible` bundle and no matrix entry
  refuses before DB mutation.
- Command-level install helper test with an injected test matrix entry records
  compatibility metadata in the changeset audit. This should use internal test
  plumbing, not a public CLI matrix flag.
- Process-based `bundle_replay` coverage can use the gated test matrix
  injection channel when it needs to exercise the binary end to end.
- Remove helper test for an installed family-compatible bundle uses the same
  matrix checks before pre-remove execution.
- Update preflight refuses before creating the update changeset row when the
  selected package needs a missing matrix entry.
- `--no-scripts` still refuses raw replay before matrix override acceptance.
- Existing `apps/conary/tests/foreign_replay.rs` cases are updated to inject
  synthetic matrix entries when they expect guarded/permissive acceptance.
- Metadata serialization excludes local paths.
- Old Goal 6 changeset metadata without a compatibility sub-object
  deserializes successfully.

Docs checks:

- Active goal queue points at this design.
- `docs/modules/ccs.md` or `docs/modules/source-selection.md` explains that
  converted CCS packages are not automatically raw-scriptlet portable.

## Risks And Mitigations

### Risk: Matrix Becomes A Backdoor

If matrix entries are too easy to add, they could become a silent replacement
for proof. Goal 7 mitigates this by shipping an empty production matrix, making
test entries synthetic, and requiring a follow-up design before any real entry
lands.

### Risk: Preflight Check Scope Creep

Helper version probing, SELinux/AppArmor introspection, and service-manager
behavior checks can become a large compatibility project. Goal 7 limits core
preflight to injected facts and stable reason IDs.

### Risk: Refusal Audit Expectations Conflict With No-Mutation Policy

Recording refused overrides in changesets would create DB rows for operations
that intentionally fail before mutation. Goal 7 does not do that. It records
accepted overrides in changeset metadata and surfaces refused override attempts
through stable errors.

### Risk: Struct Literal Fallout

Adding fields to `LegacyReplayPolicyInput`, `LegacyReplayPlan`, and
`LegacyReplayAudit` will break direct struct literals in tests and command
helpers. The implementation plan must inventory these with `rg` before editing.

Known literal sites include:

- `LegacyReplayPolicyInput`: core test helper in
  `crates/conary-core/src/ccs/legacy_replay.rs`, production install planning in
  `apps/conary/src/commands/install/mod.rs`, remove planning in
  `apps/conary/src/commands/remove.rs`, rollback checks in
  `apps/conary/src/commands/system.rs`, and
  `apps/conary/tests/foreign_replay.rs`.
- `LegacyReplayPlan`: `build_plan()` plus test helpers in
  `apps/conary/src/commands/install/mod.rs`,
  `apps/conary/src/commands/install/batch.rs`,
  `apps/conary/src/commands/install/restore.rs`, and
  `apps/conary/src/commands/remove.rs`.
- `LegacyReplayAudit`: install/remove audit builders and
  `apps/conary/src/commands/changeset_metadata.rs` tests.

### Risk: Audit Schema Compatibility

The changeset metadata schema remains `conary.changeset.metadata.v1`.
New fields must use `#[serde(default)]` where needed so older metadata remains
readable.

### Risk: Hidden Test Matrix Injection Becomes Policy

End-to-end tests may require a way to pass synthetic matrix entries into the
spawned `conary` binary. That injection seam must be named as test-only, guarded
from release behavior, and excluded from CLI help. It is not an operator policy
feature and must not weaken the empty production matrix default.

## Acceptance Criteria

Goal 7 is complete when:

- `family-compatible` raw replay refuses without an explicit matrix entry;
- matching matrix entries are deterministic and validated against duplicates;
- shallow preflight failures produce stable reason IDs;
- strict/guarded/permissive host policy still behaves as Goal 6 specified;
- production host-target resolution no longer treats the bundle source target
  as the host target;
- accepted mutation changesets record compatibility decision, matrix entry,
  matrix digest, preflight checks, and override usage;
- refused operations still fail before mutation without creating audit-only
  changesets;
- no production matrix entry grants real same-family raw replay;
- docs state that converted CCS format does not imply raw scriptlet
  portability;
- verification commands from the Goal 7 queue pass:

```bash
cargo test -p conary-core target_compatibility
cargo test -p conary --test bundle_replay
cargo test -p conary --test foreign_replay
cargo test -p conary --bin conary live_host_safety
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Future Work

Goal 8a should add golden native-vs-CCS behavior fixtures and lifecycle
expansion. Goal 8b should retire regex authority after adapter parity evidence
exists. A follow-up compatibility goal may add real production matrix entries,
host fact discovery, helper version probing, or an operator-managed
compatibility policy file, but none of those belong in Goal 7.
