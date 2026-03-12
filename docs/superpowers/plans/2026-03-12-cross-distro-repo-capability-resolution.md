# Cross-Distro Native Repo Semantics and Resolution Policy Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make RPM, Debian, and Arch repository packages resolve correctly as first-class Conary inputs inside one unified package universe, with native dependency semantics, scheme-aware version comparison, policy-driven source selection, cross-distro mixing when allowed, and optional hard pin / replatform behavior when requested. The product goal is not only “install from more distros,” but “converge those packages into Conary-managed state” so users gain CAS-backed storage, deduplication, stronger provenance, immutable generations, safer rollback, and richer declarative system control.

**Architecture:** Replace the current RPM-centric string parsing path with a native repository semantics pipeline: each parser emits structured requirements/provides plus a version scheme and source distro, repo sync persists that data into normalized tables, and selection/SAT resolution consume it directly. Put canonical cross-distro identity above that layer: unscoped root package requests expand through curated plus Repology-backed canonical mappings, rank allowed distro implementations, then hand the chosen implementation to the native repo solver. Make declarative source policy the product-facing source of truth for that ranking: the system model and product commands should capture user intent such as `balanced/latest-anywhere`, hard pins, and package-family overrides, while explicit request scoping (`--repo`, `--from-distro`) remains separate from dependency mixing (`strict`, `guarded`, `permissive`) so policy stays distinct from capability matching.

**Tech Stack:** Rust, rusqlite/SQLite migrations, current `conary-core` repository parsers, `resolvo`, existing canonical/distro pin models, existing install/conversion/adopt flows.

---

## Behavioral Targets As Of 2026-03-12

Use these as behavioral references only. Do not copy architecture or implementation directly into Conary.

- RPM / DNF / libsolv
  - DNF5 `install` and `download` document that `--from-repo` limits only explicitly requested packages and their provides lookup, while dependencies are still resolved from any enabled repository. Conary should mirror this for explicit request scoping instead of accidentally pinning the whole dependency graph.
  - DNF5 `repoclosure` checks dependency closure across enabled repositories, which is the right mental model for Conary's repo verification once native provides/requires are first-class.
  - libsolv explicitly supports `rpm`, `deb`, and `arch linux`, so Conary should store solver-facing repository data as native capabilities/relations instead of flattened JSON strings.
- Debian
  - Debian Policy 4.7.3.0 states that relationship fields support alternatives with `|`, version operators `<<`, `<=`, `=`, `>=`, `>>`, and versioned `Provides`.
  - Debian Policy also states that unversioned `Provides` do not satisfy versioned dependencies, while versioned `Provides` do.
- Arch
  - Arch `PKGBUILD` docs say versioned `provides` are valid and should include version information when consumers require one.
  - Arch `PKGBUILD` docs also say `pkgname` is implicitly provided and should not be redundantly added to `provides`.
  - `libalpm_depends(3)` says satisfier lookup first checks literal names and then providers, and that depstrings may include version operators.
  - `vercmp(8)` / `alpm-pkgver(7)` define Arch version ordering; `pkgrel` is only compared when it is present on both sides.
- Repology / canonical mapping
  - Repology is appropriate for cross-distro identity and freshness hints for root package selection.
  - Repology is not authoritative for native dependency edges, virtual provides, or solver semantics inside a distro repository.

Reference URLs:
- `https://dnf5.readthedocs.io/en/latest/commands/install.8.html`
- `https://dnf5.readthedocs.io/en/latest/commands/download.8.html`
- `https://dnf5.readthedocs.io/en/latest/dnf5_plugins/repoclosure.8.html`
- `https://github.com/openSUSE/libsolv`
- `https://www.debian.org/doc/debian-policy/ch-relationships.html`
- `https://wiki.archlinux.org/title/PKGBUILD`
- `https://man.archlinux.org/man/libalpm_depends.3.en`
- `https://man.archlinux.org/man/core/pacman/vercmp.8.en`
- `https://man.archlinux.org/man/alpm-pkgver.7.en`

Planning rules for every parser, selector, and resolver task below:
- preserve native ecosystem semantics first
- normalize them into one Conary model second
- keep policy decisions above native semantics
- make declarative source policy the primary user-facing contract; keep runtime pin tables as compatibility mirrors during migration
- treat cross-distro access as the front door, but Conary-managed ownership as the long-term value unlock
- treat explicit request scope and dependency mixing as separate controls
- use curated and Repology-backed mappings for root package identity and cross-distro implementation ranking, never for native dependency edges
- keep name-variation fallback above native lookup, never inside it
- support policy rules at more than one granularity: exact package, canonical package family, and future package/source classes
- do not reuse `crate::version::RpmVersion` or `VersionConstraint` for Debian/Arch repository semantics
- do not invent a single synthetic version ordering across RPM, Debian, and ALPM version schemes
- add regression tests from real ecosystem behaviors, not only synthetic happy paths

## File Structure

### New files

- `conary-core/src/repository/dependency_model.rs`
  - Cross-distro normalized repository requirement/provide types, including OR groups and conditional markers.
- `conary-core/src/repository/versioning.rs`
  - Repository-native version schemes and comparison/constraint helpers for RPM, Debian, and ALPM.
- `conary-core/src/repository/resolution_policy.rs`
  - Policy types for explicit request scope, distro pinning, package overrides, and dependency mixing.
- `conary-core/src/db/models/repository_capability.rs`
  - CRUD helpers for persisted repository `provides`.
- `conary-core/src/db/models/repository_requirement.rs`
  - CRUD helpers for persisted requirement groups and requirement clauses.

### Modified files

- `conary-core/src/repository/mod.rs`
  - Export dependency model, versioning, and policy modules.
- `conary-core/src/model/parser.rs`
  - Promote source policy from narrow `distro + mixing` config into a richer declarative model with compatibility aliases.
- `conary-core/src/model/state.rs`
  - Snapshot effective source policy, not only package install/pin state.
- `conary-core/src/repository/parsers/mod.rs`
  - Extend parser output to carry normalized requirements/provides plus `source_distro` and `version_scheme`.
- `conary-core/src/db/models/mod.rs`
  - Export new repository requirement/provide models.
- `conary-core/src/db/models/distro_pin.rs`
  - Keep legacy pin state in sync with the richer effective source policy during migration.
- `conary-core/src/db/models/canonical.rs`
  - Extend canonical implementation records if needed for cross-distro ranking metadata and freshness hints.
- `conary-core/src/db/migrations.rs`
  - Add normalized repo dependency tables plus package/trove distro and version scheme columns.
- `conary-core/src/db/schema.rs`
  - Bump schema version and verify migrations.
- `conary-core/src/db/models/repository.rs`
  - Add `distro` / `version_scheme` fields to `RepositoryPackage`.
  - Move compatibility accessors onto normalized tables instead of JSON as callers migrate.
- `conary-core/src/db/models/trove.rs`
  - Add installed-state `source_distro` / `version_scheme` metadata for solver correctness.
- `conary-core/src/repository/sync.rs`
  - Persist normalized requirements/provides and package origin fields transactionally during sync.
- `conary-core/src/repository/parsers/fedora.rs`
  - Emit structured RPM semantics, including capability versions and rich dependency markers.
- `conary-core/src/repository/parsers/debian.rs`
  - Emit structured Debian semantics, including alternatives, `Pre-Depends`, and versioned `Provides`.
- `conary-core/src/repository/parsers/arch.rs`
  - Emit structured ALPM semantics, including versioned provides and native depstrings.
- `conary-core/src/repository/selector.rs`
  - Use scheme-aware version comparison and policy-aware candidate filtering.
- `conary-core/src/repository/resolution.rs`
  - Thread request scope and mixing policy into repository selection and package resolution.
- `conary-core/src/repository/dependencies.rs`
  - Replace metadata blob scans with normalized capability queries and policy-aware resolution.
- `conary-core/src/canonical/repology.rs`
  - Keep Repology import/ranking logic focused on root package identity and implementation freshness classes, not dependency solving.
- `conary-core/src/canonical/sync.rs`
  - Ensure repository sync feeds canonical mappings with enough metadata for seamless cross-distro root selection.
- `conary-core/src/resolver/canonical.rs`
  - Keep canonical mixing behavior aligned with repository resolution policy semantics.
- `conary-core/src/resolver/provider.rs`
  - Load normalized repo requirements/provides and scheme-aware versions instead of reparsing strings/JSON.
- `conary-core/src/resolver/sat.rs`
  - Keep solver API stable where possible, but add tests for cross-distro policy and provider closure.
- `src/commands/install/mod.rs`
  - Convert `--from-distro` into request scope instead of string-rewrite-only behavior.
- `src/commands/install/dep_mode.rs`
  - Keep dependency takeover/adopt behavior aligned with the richer source policy and convergence model.
- `src/commands/install/dep_resolution.rs`
  - Reuse adopt/takeover-aware dependency planning as one of the convergence primitives.
- `src/commands/model.rs`
  - Make `model diff/apply/snapshot` understand and surface source policy, replatform intent, and effective profile defaults.
- `src/cli/model.rs`
  - Expose source-policy-aware model workflow affordances.
- `src/cli/distro.rs`
  - Keep legacy distro pin commands as compatibility shorthands over the richer policy model.
- `src/commands/distro.rs`
  - Route distro pin/mixing commands through the effective source policy layer.
- `src/main.rs`
  - Thread any new product-facing source policy surfaces into CLI dispatch.
- `src/commands/adopt/system.rs`
  - Reuse system adoption as a gradual convergence primitive, not only a one-shot migration command, and persist distro/version scheme when bulk adoption tracks system packages.
- `src/commands/adopt/takeover.rs`
  - Reuse package takeover as the safe ownership-transfer path from system PM to Conary.
- `src/commands/generation/takeover.rs`
  - Reuse system takeover orchestration as the full-system convergence path where appropriate.
- `src/commands/generation/builder.rs`
  - Keep generation eligibility aligned with CAS-backed content and ownership state.
- `src/commands/update.rs`
  - Let update/apply paths progressively take over adopted packages when policy prefers Conary-owned implementations.
- `src/commands/install/dependencies.rs`
  - Preserve version constraints and policy context when delegating to repo resolution.
- `src/commands/install/conversion.rs`
  - Remove RPM-specific stopgaps once normalized repo solving is authoritative.
- `src/commands/system.rs`
  - Persist distro/version scheme when system packages are adopted or tracked.
- `src/commands/adopt/packages.rs`
  - Persist distro/version scheme when adopted package metadata is available.
- `src/commands/adopt/convert.rs`
  - Persist distro/version scheme during conversion-assisted adoption.
- `conary-core/src/packages/common.rs`
  - Thread version scheme/source distro into trove creation helpers where metadata is already known.

### Existing tests to extend

- `conary-core/src/repository/parsers/fedora.rs`
- `conary-core/src/repository/parsers/debian.rs`
- `conary-core/src/repository/parsers/arch.rs`
- `conary-core/src/repository/selector.rs`
- `conary-core/src/repository/dependencies.rs`
- `conary-core/src/repository/resolution.rs`
- `conary-core/src/canonical/sync.rs`
- `conary-core/src/resolver/canonical.rs`
- `conary-core/src/resolver/provider.rs`
- `conary-core/src/resolver/sat.rs`
- `conary-core/src/model/parser.rs`
- `conary-core/src/model/state.rs`
- `conary-core/tests/canonical.rs`
- `src/commands/adopt/takeover.rs`
- `src/commands/generation/takeover.rs`
- `src/commands/update.rs`
- `conary-core/src/db/schema.rs`

## Chunk 0: Product Policy, Model Ownership, And Replatform Semantics

### Task 0: Promote source policy to a first-class Conary contract

**Files:**
- Modify: `conary-core/src/model/parser.rs`
- Modify: `conary-core/src/model/state.rs`
- Modify: `conary-core/src/db/models/distro_pin.rs`
- Modify: `conary-core/src/db/models/trove.rs`
- Modify: `src/commands/model.rs`
- Modify: `src/cli/model.rs`
- Modify: `src/cli/distro.rs`
- Modify: `src/commands/distro.rs`
- Modify: `src/commands/install/dep_mode.rs`
- Modify: `src/commands/install/dep_resolution.rs`
- Modify: `src/commands/adopt/system.rs`
- Modify: `src/commands/adopt/takeover.rs`
- Modify: `src/commands/generation/takeover.rs`
- Modify: `src/commands/generation/builder.rs`
- Modify: `src/commands/update.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Replace narrow distro pin config with richer source policy modeling**

Evolve the system model beyond only `system.distro` and `system.mixing`.

Add declarative concepts for:
- source selection profile such as `balanced/latest-anywhere`
- allowed distros / repositories
- optional hard pin target plus pin strength
- rule-based overrides at exact package, canonical package family, or package-class scope
- convergence intent for full-system apply / upgrade when the preferred source set changes
- explicit convergence stages that can reuse existing install-source states such as tracked-only, CAS-backed, and fully taken-over

Compatibility requirements:
- keep legacy `[system] distro = ...` and `mixing = ...` parseable for at least one migration cycle
- map legacy inputs into the richer policy model deterministically

- [ ] **Step 2: Make first-run/default source policy explicit**

Define product behavior for how users express intent early:
- interactive onboarding or first-run workflow should ask what to optimize for
- default policy for non-interactive flows is `balanced/latest-anywhere`
- the saved model/state must make that choice visible instead of leaving it implicit

- [ ] **Step 3: Define replatform and convergence semantics**

Treat source-policy changes as desired-state changes:
- changing from Fedora affinity to an Arch hard pin is a replatform request
- `model diff`, `model apply`, and full-system upgrade should preview and then converge toward the new preferred source set
- large replacement plans should surface clearly as replatform operations, not as surprising ordinary upgrades

Reuse existing Conary migration primitives instead of inventing a second takeover stack:
- `AdoptedTrack` = tracked in Conary, still system-PM-owned, suitable for visibility and dependency accounting
- `AdoptedFull` = tracked plus CAS-backed content, suitable for generation-building and later takeover
- `Taken` / `Repository` = Conary-owned implementation after ownership transfer or native Conary install

Convergence rules:
- track-only is acceptable for early discovery and dependency bookkeeping
- generation-ready state requires CAS-backed content, so source-policy-driven replatform should prefer `AdoptedFull`, `Taken`, or `Repository` for packages that must land in generations
- Remi-backed installs and takeover paths should be reused as the normal way to replace system-PM-owned packages over time
- blocked/critical packages may remain adopted or system-owned even when the broader profile prefers takeover
- the intended steady state is increased Conary ownership over time, because CAS-backed content and Conary-managed installs unlock generations, rollback, verification, provenance, and storage dedup benefits that plain system-PM tracking cannot provide

Expected behavior:
- if a Fedora 43 system is hard-pinned to Arch and the user performs full upgrade/apply, Conary should converge toward Arch-managed implementations where native constraints are satisfiable
- policy-only changes that do not require package replacement should remain low-noise
- user-facing messaging should make it clear that cross-distro package access is only part of the value; the bigger payoff is moving packages into Conary-managed state where platform features like CAS-backed reuse and generations become available

- [ ] **Step 4: Keep runtime CLI surfaces compatible during migration**

During transition:
- `conary distro set` and `conary distro mixing` should become compatibility shorthands over the richer source policy layer
- `model snapshot` should capture effective source policy, not only explicit installs and package version pins
- runtime DB pin tables should mirror effective source policy for compatibility, not remain the long-term source of truth
- `conary adopt-system`, package takeover, and system takeover should be treated as convergence engines that `model apply` / upgrade can call into or mirror, not as isolated side features
- update/install dep modes (`satisfy`, `adopt`, `takeover`) should evolve toward profile-driven defaults rather than staying purely ad hoc flags

- [ ] **Step 5: Add focused tests**

Cover:
- parsing richer source policy with profile, hard pin, and allowed-distro list
- legacy `[system] distro` / `mixing` compatibility mapping
- snapshot round-tripping effective source policy into model form
- policy rules for a family-level override such as “Fedora kernels, Debian rest”
- convergence planning over existing ownership states such as `AdoptedTrack -> AdoptedFull -> Taken`

- [ ] **Step 6: Commit**

```bash
git add conary-core/src/model/parser.rs conary-core/src/model/state.rs conary-core/src/db/models/distro_pin.rs src/commands/model.rs src/cli/model.rs src/cli/distro.rs src/commands/distro.rs src/main.rs
git commit -m "feat(model): promote source policy to a first-class contract"
```

## Chunk 1: Native Model, Versioning, and Policy Surfaces

### Task 1: Add native repository dependency, version, and policy types

**Files:**
- Create: `conary-core/src/repository/dependency_model.rs`
- Create: `conary-core/src/repository/versioning.rs`
- Create: `conary-core/src/repository/resolution_policy.rs`
- Modify: `conary-core/src/repository/mod.rs`
- Modify: `conary-core/src/repository/parsers/mod.rs`

- [ ] **Step 1: Define native dependency model types**

Add:
- `RepositoryDependencyFlavor` (`Rpm`, `Deb`, `Arch`)
- `RepositoryCapabilityKind` (`PackageName`, `Virtual`, `Soname`, `File`, `Generic`)
- `RepositoryRequirementKind` (`Depends`, `PreDepends`, `Optional`, `Build`, `Conflict`, `Breaks`)
- `ConditionalRequirementBehavior` (`Hard`, `Conditional`, `UnsupportedRich`)
- `RepositoryRequirementGroup`
- `RepositoryRequirementClause`
- `RepositoryProvide`

The normalized shape must support:
- exact capability or package name
- package-name requirement vs capability requirement
- alternatives (`A | B`) as first-class groups, not flattened strings
- separate package version vs provide version
- source/native text for diagnostics
- rich/conditional marker without silently downgrading it to an unconditional package requirement

- [ ] **Step 2: Define scheme-aware repository versioning types**

Add:
- `RepositoryVersionScheme` (`Rpm`, `Debian`, `Alpm`)
- `RepositoryVersion`
- `RepositoryVersionConstraint`

Requirements:
- RPM comparison uses current RPM semantics
- Debian comparison does not reuse RPM ordering
- ALPM comparison follows `vercmp`/`alpm-pkgver`
- version constraints stay tied to the scheme that produced them

- [ ] **Step 3: Define repository resolution policy types**

Add:
- `RequestScope`
- `DependencyMixingPolicy`
- `ResolutionPolicy`
- `CandidateOrigin`
- `PolicyRuleScope`
- `SourceSelectionProfile`

Policy rules to encode:
- explicit request scope (`--repo`, `--from-distro`) applies only to root requests
- dependency mixing uses `strict`, `guarded`, or `permissive`
- policy rules can authorize an out-of-pin distro for one package, one canonical family, or a higher-level package class
- policy filtering happens after native semantic matching, not before

- [ ] **Step 4: Extend parser output contract**

Update `PackageMetadata` (or introduce a replacement output struct) so parsers return:
- `source_distro`
- `version_scheme`
- normalized `requirements`
- normalized `provides`
- existing legacy dependency/metadata fields only as transition compatibility output

- [ ] **Step 5: Run focused compile check**

Run: `cargo test -p conary-core repository::parsers::fedora -- --nocapture`
Expected: compile failure only from unimplemented downstream integrations, not missing exports or parser-output fields.

- [ ] **Step 6: Commit**

```bash
git add conary-core/src/repository/dependency_model.rs conary-core/src/repository/versioning.rs conary-core/src/repository/resolution_policy.rs conary-core/src/repository/mod.rs conary-core/src/repository/parsers/mod.rs
git commit -m "feat(core): add native repo semantics and resolution policy types"
```

### Task 2: Add persisted schema for native repo semantics and installed-state version schemes

**Files:**
- Create: `conary-core/src/db/models/repository_capability.rs`
- Create: `conary-core/src/db/models/repository_requirement.rs`
- Modify: `conary-core/src/db/models/mod.rs`
- Modify: `conary-core/src/db/migrations.rs`
- Modify: `conary-core/src/db/schema.rs`
- Modify: `conary-core/src/db/models/repository.rs`
- Modify: `conary-core/src/db/models/trove.rs`

- [ ] **Step 1: Add schema for normalized repo requirements/provides**

Create:
- `repository_requirement_groups`
- `repository_requirements`
- `repository_provides`

Schema requirements:
- every package requirement group is first-class
- every OR clause is a separate row
- every provide stores capability name, capability kind, native version text, and version scheme
- indexes exist for `(capability, kind)`, `(repository_package_id)`, and group lookups

- [ ] **Step 2: Expose package origin and version-scheme columns**

Ensure `repository_packages` has usable model-backed columns for:
- `distro`
- `version_scheme`

Add installed-state columns to `troves`:
- `source_distro`
- `version_scheme`

Backfill policy:
- preserve existing rows
- default legacy unknown rows to current RPM behavior only as compatibility fallback
- make new rows populate accurate metadata going forward

- [ ] **Step 3: Add model helpers**

Implement insert/list/delete helpers that mirror current ergonomics:
- list requirement groups by package id
- list clauses by group id
- list provides by package id
- delete normalized rows by repository or package id

- [ ] **Step 4: Add migration tests**

Run: `cargo test -p conary-core db::schema -- --nocapture`
Expected:
- schema version increases cleanly
- new tables are present
- `repository_packages` and `troves` expose new columns through model code

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/db/models/repository_capability.rs conary-core/src/db/models/repository_requirement.rs conary-core/src/db/models/mod.rs conary-core/src/db/migrations.rs conary-core/src/db/schema.rs conary-core/src/db/models/repository.rs conary-core/src/db/models/trove.rs
git commit -m "feat(core): persist native repo semantics and version schemes"
```

## Chunk 2: Teach All Repository Parsers To Emit Native Semantics

### Task 3: Convert Fedora parser output to structured RPM semantics

**Files:**
- Modify: `conary-core/src/repository/parsers/fedora.rs`
- Test: `conary-core/src/repository/parsers/fedora.rs`

- [ ] **Step 1: Emit structured RPM provides**

Capture:
- capability name
- capability version
- capability kind where distinguishable (`package`, `soname`, generic capability)
- package self-provide behavior where RPM metadata implies it

- [ ] **Step 2: Emit structured RPM requirements**

Preserve:
- version constraints
- package vs capability requirements
- rich/conditional dependencies as structured records, not plain strings

- [ ] **Step 3: Keep compatibility metadata during transition**

Continue filling current metadata JSON fields only until all downstream callers stop consuming them.

- [ ] **Step 4: Add focused tests**

Cover:
- `kernel-*-uname-r`
- `coreutils-common = 9.7`
- rich/conditional dep like `((linux-firmware >= X) if linux-firmware)`

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/repository/parsers/fedora.rs
git commit -m "feat(core): normalize fedora repo semantics"
```

### Task 4: Convert Debian parser output to structured DEB semantics

**Files:**
- Modify: `conary-core/src/repository/parsers/debian.rs`
- Test: `conary-core/src/repository/parsers/debian.rs`

- [ ] **Step 1: Parse hard relationship fields that affect solvability**

At minimum, capture:
- `Depends`
- `Pre-Depends`
- `Provides`

Preserve:
- package requirements with Debian operators
- alternative dependencies (`pkg-a | pkg-b`)
- versioned virtual provides

- [ ] **Step 2: Map Debian semantics into normalized groups and clauses**

Rules:
- never collapse OR dependencies into one string
- versioned `Provides` must stay versioned
- unversioned `Provides` must not masquerade as satisfying versioned requirements

- [ ] **Step 3: Add focused tests**

Cover:
- versioned `Depends`
- OR dependencies
- versioned vs unversioned virtual `Provides`
- `Pre-Depends` surviving as a distinct requirement kind

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/repository/parsers/debian.rs
git commit -m "feat(core): normalize debian repo semantics"
```

### Task 5: Convert Arch parser output to structured ALPM semantics

**Files:**
- Modify: `conary-core/src/repository/parsers/arch.rs`
- Test: `conary-core/src/repository/parsers/arch.rs`

- [ ] **Step 1: Emit structured Arch requirements and provides**

Preserve:
- versioned `depends`
- versioned `provides`
- package-level implicit self-provide
- package-level provides distinct from file/system assumptions

- [ ] **Step 2: Preserve ALPM-native depstrings**

Model depstrings so later resolution can follow libalpm behavior:
- literal package match first
- provider fallback second
- version operators applied to both package and provide matches

- [ ] **Step 3: Add focused tests**

Cover:
- `depends=foo>=1.2`
- `provides=foo=1.2`
- provider package with different package name than provided capability
- ensure `pkgname` is not redundantly required in the `provides` array to behave as provided

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/repository/parsers/arch.rs
git commit -m "feat(core): normalize arch repo semantics"
```

## Chunk 3: Persist, Query, And Select With Policy Awareness

### Task 6: Persist normalized parser output during repository sync

**Files:**
- Modify: `conary-core/src/repository/sync.rs`
- Modify: `conary-core/src/db/models/repository.rs`
- Modify: `conary-core/src/db/models/repository_capability.rs`
- Modify: `conary-core/src/db/models/repository_requirement.rs`

- [ ] **Step 1: Replace sync persistence with normalized writes**

When a repository refreshes:
- replace `repository_packages`
- replace normalized requirement groups
- replace normalized requirement clauses
- replace normalized provides

- [ ] **Step 2: Persist package origin metadata**

Every synced `RepositoryPackage` must persist:
- `distro`
- `version_scheme`
- legacy metadata only where still needed for compatibility

- [ ] **Step 3: Keep compatibility accessors while migrating callers**

`RepositoryPackage::parse_dependencies()` and `parse_dependency_requests()` should become wrappers over normalized rows where possible, not the source of truth.

- [ ] **Step 4: Add sync-level tests**

Run: `cargo test -p conary-core repository::sync -- --nocapture`
Expected: synced Fedora, Debian, and Arch fixtures populate normalized rows and package origin metadata.

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/repository/sync.rs conary-core/src/db/models/repository.rs conary-core/src/db/models/repository_capability.rs conary-core/src/db/models/repository_requirement.rs
git commit -m "feat(core): persist native repo semantics during sync"
```

### Task 7: Replace metadata blob scans with normalized capability queries

**Files:**
- Modify: `conary-core/src/repository/dependencies.rs`
- Test: `conary-core/src/repository/dependencies.rs`

- [ ] **Step 1: Replace `metadata LIKE` and JSON parsing lookups**

Use normalized capability queries for:
- direct capability to package mapping
- version-aware capability matching
- provider selection among multiple repos/candidates

- [ ] **Step 2: Keep cross-distro heuristics above native lookup**

`repology` / name-variation helpers should only run after exact native-format lookup fails.

- [ ] **Step 3: Add tests**

Cover:
- RPM provided capability
- Debian virtual package
- Arch versioned provide
- no silent fallback to package-name guessing when native provide data exists

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/repository/dependencies.rs
git commit -m "feat(core): query normalized repo capabilities for dependency resolution"
```

### Task 8: Make package selection and resolution policy-aware

**Files:**
- Modify: `conary-core/src/repository/selector.rs`
- Modify: `conary-core/src/repository/resolution.rs`
- Modify: `conary-core/src/canonical/repology.rs`
- Modify: `conary-core/src/canonical/sync.rs`
- Modify: `conary-core/src/db/models/canonical.rs`
- Modify: `conary-core/src/resolver/canonical.rs`
- Modify: `src/commands/install/mod.rs`
- Test: `conary-core/src/repository/selector.rs`
- Test: `conary-core/src/repository/resolution.rs`
- Test: `conary-core/src/canonical/sync.rs`
- Test: `conary-core/src/resolver/canonical.rs`

- [ ] **Step 1: Expand unscoped root package requests through canonical mappings**

For `conary install <name>` without an explicit repo or distro scope:
- resolve `<name>` as either canonical name or distro-specific implementation name
- expand to all allowed supported-distro implementations using curated plus Repology-backed mappings
- keep this expansion strictly at the root request layer
- once a root implementation is chosen, solve all dependency edges natively from repo metadata

- [ ] **Step 2: Rank root implementations across distros safely**

When multiple supported distro implementations are allowed:
- never directly compare Debian, RPM, and ALPM version strings as if they shared one ordering
- prefer explicit request scope, package overrides, distro pin, and affinity first
- then use curated or Repology-backed freshness/version metadata for cross-distro root-candidate ranking when available
- if no safe cross-distro freshness signal exists, prefer deterministic policy ordering over an unsafe synthetic version comparison

Define “freshest allowed supported implementation” operationally:
- first choose the best candidate within each distro using that distro's native version scheme
- then compare distros using external freshness evidence such as curated ranking or Repology project status/version metadata
- treat that external signal as a root-candidate ranking hint, not as a dependency constraint language
- if two distros are both current or freshness cannot be compared confidently, fall back to deterministic policy ordering instead of guessing
- never let Repology cause a dependency edge like `libssl.so.3`, `libc6`, or `glibc` to be renamed across distros

- [ ] **Step 3: Use scheme-aware version ordering in selector**

Replace RPM-only ordering with repository-package version scheme ordering.

Also fix architecture compatibility rules for:
- RPM `noarch`
- Debian `all`
- Arch `any`

- [ ] **Step 4: Apply explicit request scope only to root requests**

Conary behavior should mirror DNF5-style request scoping:
- `--repo` and `--from-distro` constrain the explicitly requested package or provide lookup
- transitive dependencies are not silently forced into the same scope unless policy says so

- [ ] **Step 5: Enforce mixing policy consistently**

Use one shared interpretation across canonical and repository resolution:
- `strict`: dependencies and explicit requests stay within the pinned distro unless a package, family, or explicit per-request override says otherwise
- `guarded`: prefer pinned/request distro first, allow cross-distro fallback with an explicit warning/diagnostic
- `permissive`: allow any enabled distro, but keep stable preference ordering (request scope, pin, affinity, priority)

- [ ] **Step 6: Add tests**

Cover:
- canonical request like `apache-httpd` or equivalent distro names expanding to multiple supported implementations
- permissive mode choosing an out-of-pin implementation when it is the freshest allowed supported candidate
- incomparable cross-scheme versions falling back to deterministic policy ordering instead of synthetic comparison
- dependency edges for that chosen implementation still solved with native distro semantics
- `--from-distro` constrains only the root request
- family-level override such as “Fedora kernels, Debian rest” applies before the global profile/pin fallback
- `strict` rejects out-of-pin dependency candidates
- `guarded` warns on fallback
- `permissive` allows mixed-distro closure

- [ ] **Step 7: Commit**

```bash
git add conary-core/src/repository/selector.rs conary-core/src/repository/resolution.rs conary-core/src/canonical/repology.rs conary-core/src/canonical/sync.rs conary-core/src/db/models/canonical.rs conary-core/src/resolver/canonical.rs src/commands/install/mod.rs
git commit -m "feat(core): add seamless cross-distro root package selection"
```

## Chunk 4: Rewire The SAT Layer To Use Native Semantics End-To-End

### Task 9: Load normalized provides and requirements into the SAT provider

**Files:**
- Modify: `conary-core/src/resolver/provider.rs`
- Test: `conary-core/src/resolver/provider.rs`

- [ ] **Step 1: Stop reparsing repo dependency strings and JSON**

Load:
- repo solvables
- normalized requirement groups and clauses
- normalized provides with scheme-aware provide versions

- [ ] **Step 2: Use native version schemes inside provider filtering**

Rules:
- package version comparisons use package version scheme
- provide version comparisons use provide version scheme
- capability satisfaction must not fall back to provider package version when a provide version exists

- [ ] **Step 3: Respect alternatives and conditional records**

Implement:
- OR groups as actual multi-candidate SAT inputs
- conditional/rich requirements as explicit unsupported/ignored-with-diagnostics records unless SAT support is added in the same task

- [ ] **Step 4: Add provider tests**

Cover:
- `kernel-modules-core-uname-r`
- `coreutils-common`
- Debian virtual package with versioned provide
- Arch versioned provide
- mixed repositories under `strict` / `guarded` / `permissive`

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/resolver/provider.rs
git commit -m "feat(core): load native repo semantics into SAT provider"
```

### Task 10: Make installed-state solver inputs version-scheme aware

**Files:**
- Modify: `conary-core/src/db/models/trove.rs`
- Modify: `conary-core/src/packages/common.rs`
- Modify: `src/commands/system.rs`
- Modify: `src/commands/adopt/packages.rs`
- Modify: `src/commands/adopt/system.rs`
- Modify: `src/commands/adopt/convert.rs`
- Modify: `src/commands/install/mod.rs`
- Modify: `conary-core/src/resolver/provider.rs`
- Test: `conary-core/src/resolver/provider.rs`

- [ ] **Step 1: Persist accurate installed-state origin data where available**

Populate `troves.source_distro` and `troves.version_scheme` for:
- repository installs
- conversion-assisted installs
- adopt flows that know source package format/distro

- [ ] **Step 2: Add compatibility fallback for legacy rows**

Legacy rows without explicit scheme metadata may continue using current RPM behavior, but:
- the fallback must be explicit
- it must be visible in code/comments
- tests must cover the fallback boundary

- [ ] **Step 3: Make installed solvables use their stored version scheme**

Provider behavior must compare installed troves with the correct scheme when they participate in dependency satisfaction or upgrade/downgrade decisions.

- [ ] **Step 4: Add tests**

Cover:
- installed Debian package satisfying Debian versioned dependency
- installed Arch package participating in provider selection
- legacy installed RPM row still behaving like current Conary

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/db/models/trove.rs conary-core/src/packages/common.rs src/commands/system.rs src/commands/adopt/packages.rs src/commands/adopt/system.rs src/commands/adopt/convert.rs src/commands/install/mod.rs conary-core/src/resolver/provider.rs
git commit -m "feat(core): make installed solver inputs version-scheme aware"
```

### Task 11: Add SAT-level regression coverage for cross-distro closure and policy

**Files:**
- Modify: `conary-core/src/resolver/sat.rs`
- Test: `conary-core/src/resolver/sat.rs`

- [ ] **Step 1: Add end-to-end SAT tests**

Cover:
- RPM transitive capability provider chain
- Debian OR plus versioned virtual dependency
- Arch versioned provider chain
- explicit request scope with cross-distro dependency fallback

- [ ] **Step 2: Add policy regressions**

Specifically assert:
- `strict` pin rejects out-of-pin provider closure
- `guarded` allows fallback with surfaced warning path
- `permissive` can solve a mixed-distro transaction when native semantics line up

- [ ] **Step 3: Add provide-version regression**

Assert that satisfying a capability uses the capability version when present, not the provider package version.

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/resolver/sat.rs
git commit -m "test(core): add cross-distro SAT and policy regression coverage"
```

## Chunk 5: Integrate Product Paths And Remove Stopgaps

### Task 12: Simplify install/conversion stopgaps and preserve policy-rich diagnostics

**Files:**
- Modify: `src/commands/install/dependencies.rs`
- Modify: `src/commands/install/conversion.rs`
- Modify: `src/commands/install/mod.rs`
- Modify: `src/commands/install/dep_mode.rs`
- Modify: `src/commands/install/dep_resolution.rs`
- Modify: `src/commands/update.rs`
- Modify: `src/commands/adopt/system.rs`
- Modify: `src/commands/adopt/takeover.rs`
- Modify: `src/commands/generation/takeover.rs`
- Modify: `src/commands/generation/builder.rs`

- [ ] **Step 1: Preserve version constraints and policy context into repo resolution**

Fix the current name-only path so install resolution passes full request tuples:
- dependency name
- version constraint
- request scope
- applicable mixing policy

- [ ] **Step 2: Remove stopgap logic made obsolete by normalized semantics**

Delete or minimize:
- metadata blob probing
- RPM-only string heuristics
- repeated package-name guessing
- special-case compensation that duplicates provider logic

- [ ] **Step 2.5: Reuse adopt/takeover/generation primitives for gradual convergence**

Instead of adding a separate one-off “replatform installer” path:
- use adopt-system style tracking when policy wants visibility before ownership transfer
- use full adoption / CAS ingestion when policy wants a package to participate in generations before takeover
- use takeover / Remi-backed install paths when policy wants Conary to replace the system-PM-owned implementation
- keep blocked critical packages on the safe side of that transition unless explicitly supported

Make the ownership ladder explicit in code and diagnostics:
- `AdoptedTrack` means tracked but not generation-ready
- `AdoptedFull` means CAS-backed and generation-eligible, but still not fully Conary-owned
- `Taken` / `Repository` means fully converged for that package

Design implication:
- do not optimize only for “can install package X from distro Y”
- also optimize for “does that package now participate in Conary’s higher-level guarantees and platform features”

- [ ] **Step 3: Keep user-facing diagnostics strong**

If resolution still fails, error output must name:
- unresolved requirement
- which package required it
- whether it was package, capability, OR group, or conditional
- which policy (`strict`, `guarded`, `permissive`) affected candidate selection
- which distros/repos were considered or excluded
- whether the package stayed tracked, was CAS-ingested, or was fully taken over as part of convergence

- [ ] **Step 4: Commit**

```bash
git add src/commands/install/dependencies.rs src/commands/install/conversion.rs src/commands/install/mod.rs src/commands/install/dep_mode.rs src/commands/install/dep_resolution.rs src/commands/update.rs src/commands/adopt/system.rs src/commands/adopt/takeover.rs src/commands/generation/takeover.rs src/commands/generation/builder.rs
git commit -m "refactor(conary): remove repo solving stopgaps after native normalization"
```

### Task 13: Verify Fedora, Debian, Arch, and mixed-distro product paths

**Files:**
- Test: `tests/integration/remi/manifests/phase3-group-n-container.toml`
- Test: existing parser/selector/provider/sat tests

- [ ] **Step 1: Run focused local verification**

Run:
- `cargo test -p conary-core repository::parsers::fedora -- --nocapture`
- `cargo test -p conary-core repository::parsers::debian -- --nocapture`
- `cargo test -p conary-core repository::parsers::arch -- --nocapture`
- `cargo test -p conary-core repository::selector -- --nocapture`
- `cargo test -p conary-core repository::dependencies -- --nocapture`
- `cargo test -p conary-core repository::resolution -- --nocapture`
- `cargo test -p conary-core resolver::canonical -- --nocapture`
- `cargo test -p conary-core resolver::provider -- --nocapture`
- `cargo test -p conary-core resolver::sat -- --nocapture`
- `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 2: Run product-path verification**

Run:
- Forge Group N container suite on `fedora43`
- one Debian virtual/alternative dependency regression path
- one Arch versioned provide regression path
- one mixed-distro install path with `permissive`
- one unscoped canonical request that chooses the freshest allowed supported implementation across distros
- one strict pin rejection path
- one `--from-distro` explicit-request path where dependencies still resolve from other enabled distros when policy allows
- one full-system replatform path where source policy changes from one distro preference to another and the upgrade/apply plan surfaces that clearly
- one family-level override path such as Fedora kernels with Debian-or-Arch userland
- one incremental convergence path where packages move from tracked-only to CAS-backed to taken-over as updates arrive from Remi
- one generation build path that proves track-only packages are not treated as fully converged generation content

Expected:
- Group N no longer fails on `kernel-*-uname-r`
- Group N no longer fails on `coreutils-common`
- Debian virtual and OR dependencies resolve through normalized provider data
- Arch versioned provides resolve with ALPM semantics, not RPM heuristics
- permissive mixed-distro transactions succeed when native requirements are satisfiable
- strict pin blocks out-of-policy candidates deterministically
- replatform operations are surfaced distinctly from ordinary upgrades and converge toward the new preferred source set
- gradual convergence reuses existing adopt/takeover machinery instead of bypassing it

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test(phase3): verify native cross-distro repo semantics and policy"
```

## Notes For Execution

- Do not keep extending the current RPM-specific heuristics unless a failing test proves the normalized model still needs a narrow compatibility shim.
- `repology` and cross-distro name-variation helpers remain useful, but only after native-format resolution fails.
- Prefer first-class tables over more JSON fields; metadata blob scanning is still the core design debt.
- Keep request scoping, mixing policy, and native semantics distinct in code structure. If those get tangled together, back up and split the abstraction.
- Do not claim “cross-distro correctness” until Task 10 lands; repo-native correctness alone is not enough for mixed installed-state systems.
