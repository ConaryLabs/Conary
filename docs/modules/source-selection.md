---
last_updated: 2026-04-07
revision: 1
summary: Canonical guide to Conary source-selection inputs, runtime mirrors, and install or replatform behavior
---

# Source Selection Module (conary-core/src/repository/ + conary-core/src/model/)

Source selection is the policy layer that decides which repositories are
eligible to satisfy a request and how allowed candidates are ranked once they
are eligible.

Conary now uses one shared source-selection model across install, update,
model diff/apply, and replatform planning instead of keeping separate policy
logic in each flow.

## Data Flow

```text
system.toml [system]
  |
  +-- SystemConfig
  |     profile
  |     selection_mode
  |     allowed_distros
  |     pin / distro / mixing
  |     convergence
  |
  +-- model apply mirrors explicit runtime state
          |
          +-- DistroPin table
          |     current source pin + mixing policy
          |
          +-- settings table
                source.selection-mode
                source.allowed-distros
                       |
                       v
              load_effective_policy()
                       |
                       v
                EffectiveSourcePolicy
                       |
                       +-- ResolutionPolicy eligibility
                       +-- SelectionMode ranking
                       +-- root install / SAT ordering / update / replatform
```

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `SystemConfig` | `model/parser.rs` | Model-layer source-policy config from `[system]` |
| `SourcePinConfig` | `model/parser.rs` | Explicit source pin plus mixing strength |
| `ConvergenceIntent` | `model/parser.rs` | How aggressively Conary should move packages toward Conary-managed state |
| `SelectionMode` | `repository/resolution_policy.rs` | Candidate ranking mode: `policy` or `latest` |
| `ResolutionPolicy` | `repository/resolution_policy.rs` | Request scope, mixing, selection mode, and allowlist used by the resolver |
| `EffectiveSourcePolicy` | `repository/effective_policy.rs` | Runtime policy assembled from DB state plus inferred primary flavor |
| `ReplatformExecutionPlan` | `model/replatform.rs` | Executable and blocked replatform transactions derived from planned replacements |

## Model Inputs

The user-facing source-policy surface lives under `[system]` in `system.toml`.

Important fields:

- `profile`: preset source-selection intent. The default profile is
  `balanced/latest-anywhere`.
- `selection_mode`: explicit ranking override. Valid values are `policy` and
  `latest`.
- `allowed_distros`: allowlist of source identifiers Conary may use when
  selecting packages.
- `pin`: richer source pin with distro plus mixing strength.
- `distro` / `mixing`: compatibility fields that still map into the effective
  pin when `pin` is not set.
- `convergence`: how aggressively package ownership should move during source
  transitions.

`SystemConfig::effective_selection_mode()` prefers the explicit
`selection_mode` field and falls back to a profile-derived value.
`SystemConfig::runtime_selection_mode_mirror()` only mirrors an explicit
override or an explicitly written profile into runtime state.

## Runtime Mirrors

Conary persists the runtime source-policy mirror in SQLite:

- `DistroPin`: current pinned source plus mixing policy
- `settings["source.selection-mode"]`: persisted ranking override
- `settings["source.allowed-distros"]`: JSON-encoded allowlist

`load_effective_policy()` merges those tables into one `EffectiveSourcePolicy`
and derives the primary distro flavor used for strict or guarded mixing.

## Transitional Defaults

Two defaults currently coexist on purpose:

- Model-backed configuration defaults to `profile = "balanced/latest-anywhere"`,
  which maps to `SelectionMode::Latest`.
- Runtime policy loading defaults to `SelectionMode::Policy` when
  `source.selection-mode` is unset.

That means:

- a freshly parsed model has a latest-oriented source-selection intent unless
  it is overridden
- an imperative CLI flow with no mirrored runtime override still behaves like
  policy mode

When model apply mirrors an explicit profile or explicit `selection_mode`, the
runtime behavior becomes consistent with the model.

## Eligibility vs Ranking

Conary keeps eligibility and ranking separate:

- Eligibility decides whether a candidate may participate at all.
- Ranking decides which candidate wins among already-eligible candidates.

Eligibility inputs include:

- root request scope (`--repo`, `--from-distro`)
- mixing policy (`strict`, `guarded`, `permissive`)
- explicit allowlist from `allowed_distros`

Ranking input is `SelectionMode`:

- `policy`: preserve existing policy-first ranking behavior
- `latest`: prefer the newest allowed candidate according to the Repology-backed
  latest signal

Explicit version constraints remain strict and scheme-aware. Cross-distro
identity mapping helps find equivalent packages; it does not replace native
version constraint semantics.

## Ranking Modes

### `policy`

`policy` keeps the existing bias toward the current source policy and existing
candidate ranking rules. Update flows prefer staying on the installed source
when a same-source newer version exists.

### `latest`

`latest` still respects eligibility rules first, but once candidates are
allowed it prefers the one with a positive latest signal.

In the current implementation, that signal comes from Repology status data:

- positive “newest” signal among allowed candidates sorts first
- missing or stale signal falls back to normal policy ranking

The source-selection system does not attempt cross-scheme version arithmetic
between RPM, DEB, and Arch versions for this ranking step.

## Flow Behavior

### Install

Install uses the shared effective policy and then layers root-only request
scope such as `--repo` or `--from-distro` on top of it. Exact-name selection
and SAT ordering both respect the effective source-selection settings.

### Update

Update now also loads the shared effective policy.

- `policy` mode remains current-source-biased
- `latest` mode may re-evaluate allowed sources and switch distros when a
  newer allowed candidate has a positive latest signal

Source-switching updates must be previewed and confirmed unless `--yes` is
supplied.

### Model Diff / Apply

Model diff captures source-policy drift as structured actions such as:

- `SetSourcePin`
- `ClearSourcePin`
- `SetSelectionMode`
- `ClearSelectionMode`
- `SetAllowedDistros`
- `ClearAllowedDistros`
- `ReplatformReplace`

Model apply persists source-policy changes first, then executes any
replatform transactions that are actually executable through the shared install
path. Blocked transactions remain visible in the rendered plan and in follow-up
warnings.

### Replatform Planning

`model/replatform.rs` uses the shared source-selection and package-selection
logic to find visible realignment targets and build a
`ReplatformExecutionPlan`.

Each transaction tracks:

- target repository metadata
- exact-version install route availability
- architecture compatibility
- unresolved target dependencies
- whether remove, install, and metadata legs are ready

This makes the plan useful both for preview output and for deciding which
replatform replacements can execute immediately.

## Operator Entry Points

The main CLI entry points are:

```bash
conary distro set fedora-43 --mixing guarded
conary distro mixing permissive
conary distro selection-mode latest
conary distro info
```

`conary distro info` shows the effective selection mode and any known source
affinity data.

## Where To Read Next

- [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md) for the workspace-level module map
- [`docs/llms/subsystem-map.md`](../llms/subsystem-map.md) for assistant-facing entry points
- `crates/conary-core/src/repository/effective_policy.rs` for runtime policy loading
- `crates/conary-core/src/model/parser.rs` for `[system]` parsing and precedence
- `apps/conary/src/commands/update.rs` for source-switching update behavior
- `crates/conary-core/src/model/replatform.rs` for executable replatform planning
