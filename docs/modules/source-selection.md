---
last_updated: 2026-07-25
revision: 10
summary: Document source policy, native repository authority, exact package identity, and lifecycle handoff
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
| `SystemConfig` | `model/parser/source_policy.rs` | Model-layer source-policy config from `[system]` |
| `SourcePinConfig` | `model/parser/source_policy.rs` | Explicit source pin plus source strength |
| `ConvergenceIntent` | `model/parser/source_policy.rs` | How aggressively Conary should move packages toward Conary-managed state |
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
- `pin`: the source-pin contract, with distro plus mixing strength.
- `convergence`: how aggressively package ownership should move during source
  transitions.

The removed flat `[system].distro` and `[system].mixing` aliases are rejected;
write `[system.pin]` directly. `SystemConfig::effective_selection_mode()` prefers the explicit
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

The repository feed catalog makes
`crates/conary-core/src/repository/supported_profiles/` the source of truth for
configured feed IDs, dependency flavor, version scheme, and Remi route-family
mapping. Fedora 44, Ubuntu 26.04, and Arch are the currently configured public
feeds, not the only destination systems Conary supports. Internal route slugs
such as `fedora` and `ubuntu` are not feed IDs. The
`repo add --remi-distro` surface accepts only those exact public IDs. Remi
sync and package fetches translate the stored public ID to the profile-owned
route slug. Persisted package identity requires the exact public ID; route slugs
are never accepted as source-identity aliases. The superseded
`data/distros.toml` catalog was deleted in M4d.

## Native Repository Authenticity

Native source selection begins only after one typed trust contract authenticates
the repository grammar. `RepositoryTrustPolicy` in
`crates/conary-core/src/repository/trust.rs` is the persisted authority;
`trust/openpgp.rs` prepares exact pinned certificates and the authenticated
Arch keyring. Parser construction, sync, package download, CLI repository
creation, Remi's admin API, and the hosted repository manifest all consume that
same tagged contract. There is no signature-check boolean, permissive mode,
runtime disable command, key lookup by short ID, or guessed key URL.

The supported chains are:

- Debian: a pinned Release certificate verifies the clearsigned `InRelease`,
  or the exact `Release` plus `Release.gpg` fallback. The signed Release
  SHA-256 and size authenticate the selected compressed `Packages` index; each
  package stanza's exact SHA-256 and size authenticate its `.deb`.
- RPM: repository metadata and packages have distinct authorities. Either a
  detached OpenPGP signature or an exact HTTPS metalink identity authenticates
  `repomd.xml`; its SHA-256 and size authenticate `primary.xml`; primary
  metadata authenticates the RPM bytes; and an independently pinned package
  certificate verifies the RPM's embedded OpenPGP signature.
- Arch: one exact keyring source supplies the pinned master certificates,
  their certifications, and packager certificates. An explicit threshold says
  how many distinct pinned masters must certify a packager key; the hosted Arch
  feeds require three of the current five masters. `SigLevel` is represented
  directly: package signatures are required, database signatures are required
  or optional, and any signature that is present must be valid under
  `TrustedOnly`. `Never`, `TrustAll`, and optional package signatures are not
  representable.

`crates/conary-core/src/repository/parsers/{debian,fedora,arch}.rs` owns the
three metadata chains. Fedora metalink identity parsing is isolated in
`parsers/fedora/metalink.rs`. `repository/download.rs` owns the terminal
package checks. Missing keys, a missing required signature, an unsupported
hash, duplicate authority records, invalid certification thresholds, or any
identity mismatch fail closed and remove a partially downloaded package.

The contract was derived against the package managers' and repository
generators' own documentation:

- [APT archive authentication](https://manpages.debian.org/bookworm/apt/apt-secure.8.en.html)
  and the [Debian repository format](https://wiki.debian.org/DebianRepository/Format)
- [DNF `gpgcheck` and `repo_gpgcheck`](https://dnf.readthedocs.io/en/stable/conf_ref.html),
  [RPM signatures and digests](https://rpm.org/docs/6.0.x/manual/signatures_digests.html),
  and the [createrepo_c repomd API](https://rpm-software-management.github.io/createrepo_c/c/group__repomd.html)
- [`pacman.conf` SigLevel](https://man.archlinux.org/man/pacman.conf.5.en),
  [ALPM repository database signatures](https://man.archlinux.org/man/extra/alpm-repo-db/alpm-repo-db.7.en),
  [Arch's master-key trust model](https://archlinux.org/master-keys/), and the
  [`archlinux-keyring` file list](https://archlinux.org/packages/core/any/archlinux-keyring/files/)

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

## Native Lifecycle Source Identity

Source policy decides which package artifact may satisfy a request. Once an RPM,
Debian, or Arch artifact is selected, its typed package-manager lifecycle ABI is
part of that artifact's identity and is not reinterpreted by `DistroPin`
mixing policy.

The lifecycle bundle records the exact source format, distro, release,
architecture, and native version scheme. Packages with source lifecycle
programs carry those programs and their exact source ABI into the Conary-owned
runtime; packages without them carry no invented hooks. Source selection
cannot change either fact.

Install, update, replatform, and model apply all pass selected lifecycle bundles
to the same typed transaction planner. The planner derives slots, argv, trigger
matches, order, and payload visibility from package metadata and transaction
state. There are no operator replay flags, mixing-policy overrides, shallow
target matrices, or command-text exceptions. If the selected root cannot
satisfy a typed interpreter or source-manager semantic, preflight reports that
semantic before mutation so the missing model can be engineered.

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
between RPM, DEB, and Arch versions for this ranking step. A positive latest
signal or distinct repository priority may select one candidate. If eligible
candidates remain tied across version schemes, selection returns a typed
ambiguity and requires an explicit repository scope or priority; repository
names never break the tie.

## Flow Behavior

### Native Package Adoption

`conary system adopt <pkg> --dry-run` builds the same package-specific plan
that apply consumes. The preview opens an existing current-schema database
read-only and never migrates it. Native discovery resolves the exact installed
package identity, version, and architecture, then inventories files,
dependencies, provides, existing Conary tracking, and tracked-file ownership.

Each requested package is classified as ready, already tracked, missing,
ambiguous, unsupported, or blocked by a tracked-file conflict. Shared
directories keep their existing owner instead of being reassigned. `--full`
also validates that every planned regular file or symlink is readable for CAS
capture, but preview never creates CAS state.

Adoption preserves migration continuity for a machine that already has native
package-manager state. It is not Conary's foreign-package acquisition path and
does not prove source-independent cross-distro conversion or lifecycle
execution.

Apply consumes that plan, rechecks file ownership inside its database
transaction, and only then writes checkpoints, package metadata, CAS objects,
changesets, and the state snapshot. Track and full adoption both preserve
native package-manager authority until an explicit takeover or
selected-generation handoff.

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

Update also enforces the limited-preview ownership boundary:

- Conary-owned updates always delegate to the same selected-root transaction
  used by `conary install`. When no current generation exists, that transaction
  materializes authoritative DB/CAS state and publishes the first generation;
  it never mutates the host root in place.
- Adopted packages keep native package-manager authority under the default
  `satisfy`/`adopt` behavior. Update reports the skip with native-PM guidance
  instead of silently replacing the package.
- `--ownership takeover` is the explicit package-level ownership crossing.
  Package names do not create special takeover exceptions; compatibility and
  dependency proof come from the selected package and typed provider graph.
- `--security` only proceeds when each requested Conary-owned update source is
  marked as publishing supported advisory metadata. Sources marked `unknown` or
  `unsupported` cause a refusal before mutation, so security-only output never
  implies completeness for a source that cannot answer advisory questions.

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
conary distro set fedora-44 --mixing guarded
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
- `crates/conary-core/src/repository/trust.rs` and
  `repository/trust/openpgp.rs` for native repository authority
- `crates/conary-core/src/repository/parsers/` and
  `repository/download.rs` for authenticated metadata and package intake
- `crates/conary-core/src/repository/supported_profiles/` for configured feed
  IDs, route slugs, parser flavor, and source version scheme
- `crates/conary-core/src/model/parser/source_policy.rs` for `[system]` parsing
  and precedence; `model/parser.rs` owns the aggregate model and validation
- `apps/conary/src/commands/install/source_policy.rs` for install request-scope
  policy construction and canonical package name resolution
- `apps/conary/src/commands/adopt/packages.rs` for shared single-package
  adoption preview/apply planning and native-authority preservation
- `apps/conary/src/commands/update/mod.rs` for update module routing
- `apps/conary/src/commands/update/package.rs` for single-package update
  execution, delta/full update handling, and lifecycle execution preflight
- `apps/conary/src/commands/update/source_policy.rs` for source-policy update
  preview and replatform update context
- `apps/conary/src/commands/update/selection.rs` for source-switching update
  candidate behavior
- `apps/conary/src/commands/update/adopted_authority.rs` for adopted-update
  native-authority policy
- `apps/conary/src/commands/update/collection.rs` for `update @collection`
  orchestration, member filtering, and per-member update dispatch
- `apps/conary/src/commands/model.rs` for the model command hub
- `apps/conary/src/commands/model/context.rs` for model loading and diff
  enrichment
- `apps/conary/src/commands/model/presentation.rs` for source-policy and
  replatform summaries
- `apps/conary/src/commands/model/apply.rs` for model apply execution and
  replatform install dispatch
- `apps/conary/src/commands/model/apply/derived.rs` for persisted
  derived-package definition and build ownership
- `apps/conary/src/commands/model/remote_diff.rs` and
  `apps/conary/src/commands/model/lock.rs` for remote include drift and
  lockfile behavior
- `crates/conary-core/src/model/replatform.rs` for executable replatform planning
