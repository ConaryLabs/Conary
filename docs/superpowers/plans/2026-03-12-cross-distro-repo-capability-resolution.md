# Cross-Distro Repo Capability Resolution Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make repository-backed dependency solving work correctly for RPM, DEB, and Arch metadata by promoting parsed `requires`/`provides` into first-class normalized data that the SAT layer consumes directly.

**Architecture:** Stop treating repository dependency semantics as ad hoc strings hidden in `repository_packages.dependencies` and `metadata` blobs. Introduce a normalized cross-distro repository dependency model plus persisted requirement/provide tables, have each repo parser populate that model with native semantics, and update the SAT/provider layer to resolve transitive dependencies against normalized provides instead of package-name guessing and metadata `LIKE` scans.

**Tech Stack:** Rust, rusqlite/SQLite migrations, current `conary-core` repository parsers, current `conary-core` SAT provider (`resolvo`-backed), existing install/conversion flows.

---

## Native Semantics References

Use these as behavioral references only. Do not copy architecture or implementation directly into Conary.

- RPM:
  - `dnf5` repository/solver behavior: `https://github.com/rpm-software-management/dnf5`
  - `libsolv` capability/solver model: `https://raw.githubusercontent.com/openSUSE/libsolv/master/README`
- Debian:
  - `apt` repository semantics and solver-facing behavior: `https://salsa.debian.org/apt-team/apt`
  - Debian Policy package relationships: `https://www.debian.org/doc/debian-policy/ch-relationships.html`
- Arch:
  - `pacman` repository and dependency semantics: `https://gitlab.archlinux.org/pacman/pacman`
  - `libalpm`/Arch dependency behavior should inform versioned `provides`, dependency matching, and soname-style package metadata handling where applicable

Planning rule for every parser and resolver task below:
- preserve native ecosystem semantics first
- normalize them into one Conary model second
- keep cross-distro heuristics and Repology-style name mapping above the native semantics layer
- do not “fix” one distro by weakening another distro’s rules
- add regression tests from real ecosystem behaviors, not just synthetic happy paths

## File Structure

### New files

- `conary-core/src/repository/dependency_model.rs`
  - Cross-distro normalized repository requirement/provide types.
  - Native-format parsing helpers shared by Fedora, Debian, and Arch parser outputs.
- `conary-core/src/db/models/repository_capability.rs`
  - CRUD helpers for persisted repository `provides`.
- `conary-core/src/db/models/repository_requirement.rs`
  - CRUD helpers for persisted repository `requires`.

### Modified files

- `conary-core/src/repository/mod.rs`
  - Export the new normalized dependency model.
- `conary-core/src/db/models/mod.rs`
  - Export new repository requirement/provide model types.
- `conary-core/src/db/migrations.rs`
  - Add schema for persisted repo requirements/provides and indexes.
- `conary-core/src/db/schema.rs`
  - Bump schema version and verify migrations.
- `conary-core/src/db/models/repository.rs`
  - Stop serving as the primary parser for repo dependency semantics.
  - Keep compatibility accessors, but read from normalized tables where appropriate.
- `conary-core/src/repository/sync.rs`
  - Persist normalized requirements/provides during repo sync.
- `conary-core/src/repository/parsers/fedora.rs`
  - Emit structured RPM requirements/provides, including capability versions and rich-dep flags.
- `conary-core/src/repository/parsers/debian.rs`
  - Emit structured Debian requirements/provides, including alternatives and version operators.
- `conary-core/src/repository/parsers/arch.rs`
  - Emit structured Arch requirements/provides, including versioned provides.
- `conary-core/src/resolver/provider.rs`
  - Load normalized repo requirements/provides instead of reparsing strings/JSON.
- `conary-core/src/repository/dependencies.rs`
  - Replace metadata string scans with normalized capability queries.
- `conary-core/src/resolver/sat.rs`
  - Keep solver API stable, but add tests proving cross-distro provider closure works.
- `src/commands/install/conversion.rs`
  - Remove RPM-specific compensation logic once normalized repo solving handles transitive providers correctly.

### Existing tests to extend

- `conary-core/src/repository/parsers/fedora.rs`
- `conary-core/src/repository/parsers/debian.rs`
- `conary-core/src/repository/parsers/arch.rs`
- `conary-core/src/repository/dependencies.rs`
- `conary-core/src/resolver/provider.rs`
- `conary-core/src/resolver/sat.rs`

## Chunk 1: Normalize Repository Dependency Semantics

### Task 1: Add cross-distro repository dependency types

**Files:**
- Create: `conary-core/src/repository/dependency_model.rs`
- Modify: `conary-core/src/repository/mod.rs`

- [ ] **Step 1: Define normalized requirement/provide types**

Add:
- `RepositoryRequirement`
- `RepositoryRequirementClause`
- `RepositoryProvide`
- `RepositoryCapabilityKind`
- `RepositoryRequirementKind`
- `RepositoryDependencyFlavor` (`Rpm`, `Deb`, `Arch`)
- `ConditionalRequirementBehavior` (`Hard`, `Conditional`, `UnsupportedRich`)

The normalized shape must support:
- exact capability name
- version constraint
- package-name requirement vs provided-capability requirement
- alternatives (`A | B`)
- rich/conditional marker without pretending it is unconditional

- [ ] **Step 2: Export the module**

Run: `cargo test -p conary-core repository::parsers::fedora -- --nocapture`
Expected: compile failure from unimplemented parser integration, not module export errors.

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/repository/dependency_model.rs conary-core/src/repository/mod.rs
git commit -m "feat(core): add normalized repository dependency model"
```

### Task 2: Add persisted tables for repo requirements/provides

**Files:**
- Create: `conary-core/src/db/models/repository_capability.rs`
- Create: `conary-core/src/db/models/repository_requirement.rs`
- Modify: `conary-core/src/db/models/mod.rs`
- Modify: `conary-core/src/db/migrations.rs`
- Modify: `conary-core/src/db/schema.rs`

- [ ] **Step 1: Add migration for normalized repo requirements/provides**

Create tables keyed by `repository_package_id`:
- `repository_provides`
- `repository_requirements`
- `repository_requirement_alternatives` if needed for OR groups

Add indexes for:
- `(capability, kind)`
- `(repository_package_id)`
- `(name, version_constraint)` if useful for direct lookup

- [ ] **Step 2: Add model helpers**

Implement insert/list/delete helpers that mirror existing `ProvideEntry`/`DependencyEntry` ergonomics.

- [ ] **Step 3: Add migration tests**

Run: `cargo test -p conary-core db::schema -- --nocapture`
Expected: schema version increases cleanly and new tables are present.

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/db/models/repository_capability.rs conary-core/src/db/models/repository_requirement.rs conary-core/src/db/models/mod.rs conary-core/src/db/migrations.rs conary-core/src/db/schema.rs
git commit -m "feat(core): persist normalized repository requirements and provides"
```

## Chunk 2: Teach All Repo Parsers to Populate the Model

### Task 3: Convert Fedora parser output to normalized RPM requirements/provides

**Files:**
- Modify: `conary-core/src/repository/parsers/fedora.rs`
- Test: `conary-core/src/repository/parsers/fedora.rs`

- [ ] **Step 1: Emit structured RPM provides**

Capture:
- capability name
- capability version
- capability kind where distinguishable (`package`, `soname`, generic capability)

- [ ] **Step 2: Emit structured RPM requirements**

Preserve:
- version constraints
- package vs capability requirements
- rich/conditional dependencies as structured conditional records, not plain strings

- [ ] **Step 3: Keep compatibility metadata during transition**

Continue filling current metadata JSON fields short-term so existing code paths still work while later tasks migrate off them.

- [ ] **Step 4: Add focused tests**

Cover:
- `kernel-*-uname-r`
- `coreutils-common = 9.7`
- rich/conditional dep like `((linux-firmware >= X) if linux-firmware)`

- [ ] **Step 5: Commit**

```bash
git add conary-core/src/repository/parsers/fedora.rs
git commit -m "feat(core): normalize fedora repo requirements and provides"
```

### Task 4: Convert Debian parser output to normalized DEB requirements/provides

**Files:**
- Modify: `conary-core/src/repository/parsers/debian.rs`
- Test: `conary-core/src/repository/parsers/debian.rs`

- [ ] **Step 1: Extend Debian parser output**

Preserve:
- package requirements with version operators
- alternative dependencies (`pkg-a | pkg-b`)
- virtual provides from package metadata when available

- [ ] **Step 2: Map Debian semantics into normalized clauses**

Do not collapse OR dependencies into a single string.

- [ ] **Step 3: Add focused tests**

Cover:
- versioned `Depends`
- OR dependencies
- virtual provide resolution shape

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/repository/parsers/debian.rs
git commit -m "feat(core): normalize debian repo requirements and provides"
```

### Task 5: Convert Arch parser output to normalized Arch requirements/provides

**Files:**
- Modify: `conary-core/src/repository/parsers/arch.rs`
- Test: `conary-core/src/repository/parsers/arch.rs`

- [ ] **Step 1: Extend Arch parser output**

Preserve:
- versioned `depends`
- versioned `provides`
- package-level provides distinct from file/system assumptions

- [ ] **Step 2: Add focused tests**

Cover:
- `depends=foo>=1.2`
- `provides=foo=1.2`
- provider package with different package name than provided capability

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/repository/parsers/arch.rs
git commit -m "feat(core): normalize arch repo requirements and provides"
```

## Chunk 3: Persist and Query Normalized Repo Dependency Data

### Task 6: Persist normalized parser output during repo sync

**Files:**
- Modify: `conary-core/src/repository/sync.rs`
- Modify: `conary-core/src/db/models/repository.rs`
- Modify: `conary-core/src/db/models/repository_capability.rs`
- Modify: `conary-core/src/db/models/repository_requirement.rs`

- [ ] **Step 1: Delete and repopulate normalized rows per sync**

When a repository refreshes:
- replace `repository_packages`
- replace normalized provides
- replace normalized requirements

- [ ] **Step 2: Keep compatibility accessors while migrating callers**

`RepositoryPackage::parse_dependencies()` should become a compatibility wrapper over normalized requirement rows, not the source of truth.

- [ ] **Step 3: Add sync-level tests**

Run: `cargo test -p conary-core repository::sync -- --nocapture`
Expected: normalized rows exist after sync for Fedora fixtures.

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/repository/sync.rs conary-core/src/db/models/repository.rs conary-core/src/db/models/repository_capability.rs conary-core/src/db/models/repository_requirement.rs
git commit -m "feat(core): persist normalized repo dependency data during sync"
```

### Task 7: Replace metadata blob scans with normalized capability queries

**Files:**
- Modify: `conary-core/src/repository/dependencies.rs`
- Test: `conary-core/src/repository/dependencies.rs`

- [ ] **Step 1: Replace `metadata LIKE` capability lookups**

Use normalized `repository_provides` queries for:
- direct capability → package mapping
- version-aware capability matching
- provider selection among multiple repos/candidates

- [ ] **Step 2: Keep cross-distro helper logic above normalized queries**

`repology`/name-variation helpers should operate after normalized native lookup fails, not before.

- [ ] **Step 3: Add tests**

Cover:
- RPM provided capability
- Debian virtual package
- Arch versioned provide

- [ ] **Step 4: Commit**

```bash
git add conary-core/src/repository/dependencies.rs
git commit -m "feat(core): query normalized repo provides for dependency resolution"
```

## Chunk 4: Rewire the SAT Provider to Use Normalized Semantics

### Task 8: Load normalized provides/requirements into the SAT provider

**Files:**
- Modify: `conary-core/src/resolver/provider.rs`
- Test: `conary-core/src/resolver/provider.rs`

- [ ] **Step 1: Stop reparsing metadata JSON in the provider**

Load:
- package solvables
- normalized requirement clauses
- normalized provided capabilities with separate capability versions

- [ ] **Step 2: Resolve candidates by normalized provides**

When a dependency name has no direct package match:
- query normalized repo provides
- attach matching provider solvables
- compare against provide version when the requirement is capability-based

- [ ] **Step 3: Respect alternatives**

Represent OR groups so the SAT provider can see multiple viable candidates instead of flattening them.

- [ ] **Step 4: Ignore or explicitly mark unsupported conditional clauses**

Do not silently treat conditional/rich requirements as unconditional package names.
If unsupported in SAT, omit them from hard requirements and carry their existence into diagnostics/tests.

- [ ] **Step 5: Add provider tests**

Cover:
- `kernel-modules-core-uname-r`
- `coreutils-common`
- Debian virtual package
- Arch versioned provide

- [ ] **Step 6: Commit**

```bash
git add conary-core/src/resolver/provider.rs
git commit -m "feat(core): load normalized repo semantics into SAT provider"
```

### Task 9: Add SAT-level regression coverage for cross-distro provider closure

**Files:**
- Modify: `conary-core/src/resolver/sat.rs`
- Test: `conary-core/src/resolver/sat.rs`

- [ ] **Step 1: Add end-to-end SAT tests**

Cover:
- RPM transitive capability provider chain
- Debian OR/virtual dependency
- Arch versioned provider chain

- [ ] **Step 2: Add “wrong package version vs provide version” regression**

Specifically assert that satisfying a capability uses the capability version when present, not the provider package version.

- [ ] **Step 3: Commit**

```bash
git add conary-core/src/resolver/sat.rs
git commit -m "test(core): add cross-distro SAT provider regression coverage"
```

## Chunk 5: Remove Stopgap Logic and Verify Real Product Paths

### Task 10: Simplify conversion/install-side stopgaps

**Files:**
- Modify: `src/commands/install/conversion.rs`
- Modify: `src/commands/install/dependencies.rs`

- [ ] **Step 1: Remove repo-specific compensation that duplicates provider logic**

Delete or minimize logic that only exists because repo semantics were not normalized:
- metadata blob probing
- RPM-only string heuristics
- repeated package-name guessing

- [ ] **Step 2: Keep user-facing diagnostics strong**

If resolution still fails, error output must name:
- unresolved requirement
- which package required it
- whether it was package/capability/conditional

- [ ] **Step 3: Commit**

```bash
git add src/commands/install/conversion.rs src/commands/install/dependencies.rs
git commit -m "refactor(conary): remove repo dependency stopgaps after normalization"
```

### Task 11: Verify Group N plus Debian/Arch repo resolution regressions

**Files:**
- Test: `tests/integration/remi/manifests/phase3-group-n-container.toml`
- Test: existing parser/provider/sat unit tests

- [ ] **Step 1: Run focused local verification**

Run:
- `cargo test -p conary-core repository::parsers::fedora -- --nocapture`
- `cargo test -p conary-core repository::parsers::debian -- --nocapture`
- `cargo test -p conary-core repository::parsers::arch -- --nocapture`
- `cargo test -p conary-core repository::dependencies -- --nocapture`
- `cargo test -p conary-core resolver::provider -- --nocapture`
- `cargo test -p conary-core resolver::sat -- --nocapture`
- `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 2: Run product-path verification**

Run:
- Forge Group N container suite on `fedora43`
- one Debian metadata/provider regression path
- one Arch metadata/provider regression path

Expected:
- Group N no longer fails on `kernel-*-uname-r`
- Group N no longer fails on `coreutils-common`
- Debian virtual/OR dependencies resolve through normalized provider data
- Arch versioned provides resolve without string heuristics

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test(phase3): verify normalized cross-distro repo capability resolution"
```

## Notes for Execution

- Do not keep extending the current RPM-specific heuristics unless a failing test proves the normalized model still needs a narrow compatibility shim.
- `repology` and cross-distro name-variation helpers remain useful, but only as fallback after native-format resolution fails.
- Prefer first-class tables over more JSON fields; the current metadata blob scanning is the core design debt.
- If Debian and Arch reveal materially different needs after parser normalization, split the later execution into distro-specific follow-up plans rather than growing this one indefinitely.
