# CCS Native Ecosystem Umbrella Design And Plan

> **For agentic workers:** This is an umbrella roadmap, not a directly executable implementation plan. Do not run it as a single `/goal`. Each phase below must first receive its own deeper design and implementation plan, then be executed in focused `/goal` slices with `superpowers:subagent-driven-development` or `superpowers:executing-plans`.

**Goal:** Make CCS-native packaging and Remi-native serving complete enough that Conary can become a real native package ecosystem, with package creation easy enough for a maintainer to reach a correct first package quickly while keeping distro support intentionally limited to Fedora 44, Ubuntu 26.04, and Arch.

**Architecture:** Finish the package contract first, then build authoring, publication, adapter profiles, and proof packages on top of it. Adoption and legacy conversion remain the bridge for real hosts while CCS-native package authoring and Remi-native publication mature into the strategic center.

**Tech Stack:** Rust, serde, CBOR, TOML, SQLite, Conary CCS, Remi, `conary-test`, Cargo, existing Conary docs/audit tooling.

---

## Status

Draft umbrella packet for review.

This document is intentionally phase-level. It records the sequence, non-goals,
acceptance gates, and follow-up design packets needed before implementation.
It does not define every schema field or task body. The deeper phase specs and
plans will own those details.

## Why This Exists

The current Conary codebase already has serious building blocks: CCS packages,
content-addressed storage, chunking, generations, source selection, adoption,
legacy package conversion, scriptlet safety, recipes, and Remi package serving.
The gap is that CCS-native packaging is not yet a complete native ecosystem.

The main mismatch is between Conary's richer internal package semantics and
what CCS-native authoring plus binary package metadata can currently carry as
the authoritative install-time package contract. Some information exists in
human-authored TOML but is missing from the binary path, some fields are
conversion-oriented, and some native authoring and publication workflows are
thin compared with what real distro package maintainers need.

The next large effort should not be "support more distros." It should be:

1. Complete the CCS-native package contract.
2. Make native packages easy to author, build, test, and publish.
3. Make Remi serve CCS-native packages as first-class artifacts.
4. Only then make supported distro adaptation more data-driven.

## Current Anchors

Read these before writing any child spec or implementation plan:

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/ARCHITECTURE.md`
- `docs/modules/ccs.md`
- `docs/modules/remi.md`
- `docs/modules/recipe.md`
- `docs/modules/source-selection.md`
- `docs/specs/ccs-format-v1.md`
- `docs/INTEGRATION-TESTING.md`

Code areas that phase specs must inspect before proposing implementation:

- `crates/conary-core/src/ccs/manifest.rs`
- `crates/conary-core/src/ccs/binary_manifest.rs`
- `crates/conary-core/src/ccs/package.rs`
- `crates/conary-core/src/ccs/archive_reader.rs`
- `crates/conary-core/src/ccs/builder.rs`
- `crates/conary-core/src/ccs/builder/`
- `crates/conary-core/src/repository/`
- `crates/conary-core/src/recipe/`
- `apps/conary/src/commands/ccs/`
- `apps/conary/src/commands/repo.rs`
- `apps/remi/src/server/`
- `apps/remi/src/bin/remi.rs`
- `apps/conary-test/src/`
- `data/distros.toml`

## Non-Negotiable Constraints

- Keep public distro support limited to Fedora 44, Ubuntu 26.04, and Arch.
- Do not add any other distro targets as part of this roadmap.
- `ROADMAP.md` contains a future distro-expansion note outside this roadmap's
  scope. This umbrella packet does not implement or approve that future work.
- Do not weaken adoption safety or native package-manager authority boundaries.
- Do not turn regex, heuristic, or conversion-only evidence into authoritative
  native package truth.
- Do not make the core package contract depend on host I/O.
- Do not make Remi publication trust depend on unverified local operator state.
- Do not carry compatibility shims for stale or fake surfaces solely to avoid
  breaking internal code that should be replaced.
- Do not preserve TOML-only install-time authority unless a phase explicitly
  proves it is the right representation boundary.
- Do not design a schema that is only pleasant for Conary internals. A
  maintainer-facing CCS package should be easy to start, easy to lint, and hard
  to accidentally publish with missing authority.
- Do not start implementation from this umbrella doc. Create and review the
  phase-specific design and implementation plan first.

## Maintainer Ergonomics Principle

Ease of package creation is a product requirement, not polish. The native CCS
path should feel like a guided packaging workflow:

- Start from a directory, source archive, binary artifact, or recipe and get a
  sensible draft package with a small number of choices.
- Use templates for common package shapes instead of forcing every maintainer
  to learn the full contract first.
- Prefer safe defaults for boring cases, then fail loudly when install-time
  authority, host integration, or publication metadata is missing.
- Make lint diagnostics actionable: explain the missing field, why it matters,
  and the smallest acceptable fix.
- Keep expert controls available without making the first package require
  expert knowledge.
- Treat local build, local test, and local Remi publication as one loop, not
  separate subsystems that the maintainer has to stitch together manually.

Every phase must preserve this principle. Phase 1 should design an
authoritative package contract that can still be generated and explained by
tools. Phase 2 should turn that into the primary maintainer experience.

## Umbrella Design

Conary needs a layered native package ecosystem:

1. **Package contract layer:** Defines what a CCS-native package is, what it
   provides and requires, what files and lifecycle effects it owns, and what
   metadata is authoritative at install time.
2. **Authoring and build layer:** Lets a maintainer create, lint, build, test,
   and locally install native CCS packages without needing to understand every
   internal subsystem or hand-author every advanced field.
3. **Publication layer:** Lets Remi accept, verify, index, stage, promote, and
   serve native CCS packages.
4. **Distro adapter layer:** Captures supported host-family details as data and
   fixtures, not hidden branches scattered through core logic.
5. **Proof corpus layer:** Uses a small representative package set to prove the
   package contract, workflows, publication path, and adapter profiles together.

Each layer should be testable on its own. Later layers must not hide missing
authority in earlier layers.

## Deferred Or Scoped Decisions

The following existing CCS features are adjacent to this roadmap but should not
quietly expand a phase unless that phase-specific design chooses to include
them:

- **OCI export:** Existing `ccs/export/` and `conary ccs export` behavior
  remains available. Native publication through OCI registries is deferred.
- **Enhancement engine:** Existing `ccs/enhancement/` behavior remains
  conversion-oriented unless a child spec explicitly designs native package
  enhancement.
- **Lockfile format:** Existing `ccs.lock` documentation in the v1 format spec
  should be evaluated by Phase 2 if native authoring needs reproducible
  dependency pinning in the maintainer loop.
- **`[legacy]` manifest section:** Existing legacy-format generation metadata
  remains in place. Phase 1 and Phase 4 should decide whether v2 keeps it,
  narrows it, or replaces pieces with adapter profile data.
- **Unsupported distro wording drift:** Existing code and docs should be swept
  during relevant child plans so help text and examples do not imply public
  support outside Fedora 44, Ubuntu 26.04, and Arch. Child plans should include
  concrete CLI help, API parameter, fixture, and doc sweeps rather than relying
  only on this roadmap's supported-target grep.

## Phase 1: CCS v2 Native Package Contract

**Purpose:** Define a complete authoritative install-time package contract for
native CCS packages.

**Problem:** CCS v1 has useful pieces, but the human-authored manifest and
binary package manifest do not carry the same truth. Some install-relevant
fields such as config, `[scriptlets]` capability declarations, redirects,
policy, and provenance are TOML-only or defaulted when reading a CBOR-only
package. Typed provides such as sonames, binaries, and pkg-config entries also
degrade on CBOR-only reads, and component default selection is split between
binary component references and TOML component defaults. The current
`toml_integrity_hash` protects the bundled TOML from post-build drift, but it
does not make TOML-only fields first-class binary package authority. Dependency
and package identity semantics are thinner than Conary's repository and
resolver model. Native lifecycle, file metadata, service metadata, policy, and
provenance need clear install-time authority.

**Phase output:** A reviewed CCS v2 contract design and implementation plan.

**Scope candidates for the child design:**

- Package identity: name, epoch when needed, version, release, architecture,
  platform, ABI, flavor, source identity, and package kind. The CCS v1 format
  spec documents a `type` field for package/group/redirect packages, but the
  current `Package` struct does not implement that field; the v2 design must
  close this spec-to-code gap. The child spec must also decide the full group
  and redirect payload contract instead of only adding a kind enum.
- Dependency model: runtime, pre-install or pre-activation dependencies, build
  dependencies, optional suggestions, alternatives, virtual provides, conflicts,
  breaks, replaces, obsoletes, and file or capability dependencies.
- File model: regular files, directories, symlinks, config files, ownership,
  modes, capabilities, checksums, special-file policy, component membership,
  and conflict behavior.
- Lifecycle model: declarative hooks, service integration, tmpfiles, sysusers,
  alternatives, config merge behavior, removal behavior, and sandbox or host
  integration requirements.
- Package policy: allowed host mutations, required capabilities, public-serving
  policy, provenance expectations, and trust metadata.
- Binary/TOML boundary: decide which data is authoring-only, which is
  install-time authoritative, and how archive verification prevents drift. The
  v2 design should evaluate whether to build on `BinaryManifest`'s existing
  `toml_integrity_hash` or replace it with a more comprehensive contract.
- Component and lifecycle binding: define how declarative hooks, service
  actions, config behavior, and removal behavior map to components, including
  selective installs.
- Supported-target interface checkpoint: consider which host/profile inputs
  Phase 1 needs for lifecycle validation without implementing Phase 4 profiles.
- Compatibility/migration: define how v1 packages continue to parse while v2
  packages become the native target.

**Explicit non-goals:**

- Do not implement new CLI authoring UX in Phase 1 except where needed to test
  the contract.
- Do not add Remi native publish behavior in Phase 1.
- Do not expand supported distro targets.
- Do not solve every recipe/build workflow detail.

**Acceptance gate for Phase 1 child plan:**

- The child spec names the authoritative install-time representation.
- The child spec explains what remains authoring-only and why.
- The child spec describes how common metadata can be generated, inferred, or
  templated without weakening install-time authority.
- The child plan includes round-trip tests for the package contract.
- The child plan includes negative tests for missing/defaulted install-time
  fields that must not silently degrade.
- The child plan includes docs-truth updates for `docs/modules/ccs.md`,
  `docs/specs/ccs-format-v1.md` or a successor v2 spec, and docs-audit
  inventory/ledger implications.
- Verification is scoped to `conary-core` first, with broader workspace tests
  only when the implementation crosses crate boundaries.

## Phase 2: Native Authoring And Build Workflow

**Purpose:** Make CCS-native packages easy to create, lint, build, test, and
install locally.

**Problem:** Native CCS authoring exists, but the developer path is not yet a
full package-maintainer workflow. Recipes, manifests, policy, provenance, and
local install testing need to feel like one coherent loop.

**Phase output:** A reviewed native authoring/build workflow design and
implementation plan.

Existing CCS CLI commands such as `init`, `build`, `inspect`, `verify`, `sign`,
`keygen`, `install`, `shell`, `run`, `export`, and `enhance` provide starting
points. New maintainer commands or flows such as `lint` and package-local
`test` need to be designed from scratch, and existing commands need v2-aware
improvements. Current UX paper cuts, including stale hyphenated command names
in help text, should be treated as concrete Phase 2 polish targets.

**Scope candidates for the child design:**

- `conary ccs init` templates for common package shapes.
- Guided package creation from an existing directory, source archive, binary
  artifact, or recipe.
- `conary ccs lint` for contract validation, file policy, dependency shape,
  host-integration declarations, and publication readiness diagnostics.
- `conary ccs build` improvements for CCS v2 metadata, provenance, source
  references, reproducibility inputs, and deterministic output.
- `conary ccs test` or equivalent package-local validation against an isolated
  root or test fixture.
- Recipe/source package integration that distinguishes source authoring,
  build-time metadata, and install-time package metadata.
- Maintainer diagnostics that explain how to fix package contract problems.
- Developer loop support for local Remi or local repository testing without
  requiring cloud infrastructure.
- Example packages and template fixtures that make the supported package shapes
  discoverable from the CLI and docs.
- Existing `conary ccs shell` and `conary ccs run` runtime helpers as pieces
  of the package-local developer loop, while acknowledging that they currently
  resolve installed packages by name rather than local package files or
  just-built artifacts.
- Developer key generation, local trust-policy setup, and handoff metadata for
  later Remi publication.

**Explicit non-goals:**

- Do not make Remi the only local developer path.
- Do not promise hermetic source builds beyond what the child spec can prove.
- Do not add unsupported host targets or distro-specific templates outside
  Fedora 44, Ubuntu 26.04, and Arch.
- Do not turn recipe shell phases into trusted package metadata unless the
  contract records and validates the resulting truth.

**Acceptance gate for Phase 2 child plan:**

- A maintainer can create a minimal native CCS package from a template.
- A maintainer can draft a package from existing package contents or source
  inputs without hand-writing the full manifest first.
- The lint/build/test loop catches missing authoritative metadata before
  publication.
- The workflow emits or verifies CCS v2 package metadata from Phase 1.
- The plan includes CLI tests and focused package fixture tests.
- The plan keeps source/build provenance separate from install-time authority.
- The plan defines at least one "first package" smoke flow with exact commands
  and expected diagnostics.
- The child plan includes docs-truth updates for affected CLI, CCS, recipe, and
  maintainer-facing docs plus docs-audit inventory/ledger implications.

## Phase 3: Remi Native Publication

**Purpose:** Make Remi a first-class CCS-native repository and publication
service, not only an on-demand conversion/cache service.

**Problem:** Remi can convert and serve CCS artifacts, but the native CCS
publication workflow needs a clearer path. Current native uploads are stored in
conversion-shaped metadata with `original_format = "ccs"` and upload-prefixed
source checksums, so native packages still inherit legacy conversion storage
semantics. The ecosystem needs dedicated native package intake, verification,
signing or attestation, index generation, staging, promotion, and public-ready
serving.

**Phase output:** A reviewed Remi-native publication design and implementation
plan.

**Scope candidates for the child design:**

- Native CCS package intake route or CLI command.
- Package contract verification before storage.
- Supported-target validation and normalization for native intake, including
  the distinction between public support IDs and internal family slugs.
- Trust and signing model for native package publication.
- Developer public key registration, verification, rotation, and revocation
  flows.
- Staging, promotion, rejection, and rollback states.
- Index schema for native package metadata, dependencies, capabilities,
  components, files where appropriate, chunks, signatures, and provenance.
- Search/sparse-index behavior that treats native CCS packages as first-class
  rows.
- Local developer Remi path for package publication tests.
- Maintainer-facing publish diagnostics that explain why a package is accepted,
  staged, rejected, or not public-ready.
- Publication gates shared with conversion where appropriate, but not confused
  with conversion-only scriptlet review gates.

**Explicit non-goals:**

- Do not require cloud R2 for local native publication proof.
- Do not make legacy conversion publication the source of truth for native
  package acceptance.
- Do not weaken existing Remi authentication, federation, or pinned-trust
  expectations.
- Do not expand distro targets.

**Acceptance gate for Phase 3 child plan:**

- A native CCS package can be published to local Remi, verified, indexed, and
  fetched by Conary.
- Public-ready status is derived from native package verification and trust
  policy, not legacy conversion heuristics.
- The plan includes Remi-owned tests for publication state transitions.
- The plan includes client-side tests for resolving and installing from a Remi
  native package source.
- Index and search behavior are covered enough that native packages cannot be
  silently omitted or misrepresented as conversion-only artifacts, including
  native-only upload rows that are not backed by `repository_packages`.
- The child plan includes docs-truth updates for affected Remi, repository,
  trust, and client docs plus docs-audit inventory/ledger implications.

## Phase 4: Supported Distro Adapter Profiles

**Purpose:** Make Fedora 44, Ubuntu 26.04, and Arch adaptation more
data-driven and easier to validate.

**Problem:** Distro-aware behavior exists across repository parsing, source
selection, host integration, adoption, replay target checks, and test fixtures.
Conary needs a clearer supported-target profile model so adapting within the
current support set is mostly data and fixtures instead of scattered code.

**Phase output:** A reviewed supported-distro adapter profile design and
implementation plan.

**Scope candidates for the child design:**

- Declarative supported-target profiles for Fedora 44, Ubuntu 26.04, and Arch.
- Service manager, path layout, config, tmpfiles, sysusers, alternatives, shell,
  package-manager authority, and security-policy profile facts.
- Repository parser/backend capability declarations.
- Host-integration validators for native CCS lifecycle metadata.
- A normalized boundary between public support IDs, internal repository family
  slugs, and legacy replay target identifiers.
- Fixture-driven profile tests for each supported target.
- Clear separation between package contract, host profile, and conversion
  source-target metadata.

**Explicit non-goals:**

- Do not add new supported distro families.
- Do not make every distro adaptation purely declarative if a parser/backend is
  genuinely family-specific.
- Do not allow profiles to bypass package contract validation.
- Do not move host I/O into core planners.

**Acceptance gate for Phase 4 child plan:**

- Supported target facts are discoverable from one profile surface.
- The profile layer covers only Fedora 44, Ubuntu 26.04, and Arch.
- Profile validation is tested with fixtures and does not require live host I/O
  in core.
- Adding or editing a supported-target fact updates tests in the same slice.
- Distro-specific code paths become easier to find, not harder.
- The child plan includes docs-truth updates for affected supported-target,
  source-selection, Remi, and integration-testing docs plus docs-audit
  inventory/ledger implications.

## Phase 5: Native CCS Proof Corpus

**Purpose:** Prove the ecosystem with a small set of representative native CCS
packages and end-to-end workflows.

**Problem:** A package ecosystem is only real when the package contract,
authoring workflow, Remi publication, and supported-target profiles work
together on realistic package shapes.

**Phase output:** A reviewed proof-corpus design and implementation plan.

**Scope candidates for the child design:**

- A simple CLI package.
- A daemon/service package.
- A library plus development component split.
- A package with config/noreplace behavior.
- A source-built package with provenance.
- A Remi-published native package fetched and installed by Conary.
- Negative fixtures for conflicts, missing metadata, unsafe host integration,
  and unsupported target claims.

**Explicit non-goals:**

- Do not try to package a full distribution in this phase.
- Do not use converted legacy packages as proof that native authoring works.
- Do not use one-off hand-built artifacts that cannot be rebuilt by the test
  harness.
- Do not add unsupported distro fixtures.

**Acceptance gate for Phase 5 child plan:**

- Each proof package has a clear package-contract reason to exist.
- The corpus exercises CCS v2 metadata, authoring/build workflow, Remi native
  publication, and supported-target profiles.
- The corpus is reproducible enough for local validation.
- The plan includes positive and negative integration tests.
- The resulting docs can honestly describe the CCS-native package workflow.
- The child plan includes docs-truth updates for affected proof-corpus,
  integration-testing, CCS, Remi, and maintainer docs plus docs-audit
  inventory/ledger implications.

## Sequencing Rules

The phases should normally run in order:

1. Phase 1 contract.
2. Phase 2 authoring/build workflow.
3. Phase 3 Remi native publication.
4. Phase 4 supported distro adapter profiles.
5. Phase 5 proof corpus.

Phase 4 can start design review before Phase 3 implementation finishes, but it
must not implement profile behavior that depends on package fields Phase 1 has
not defined. Phase 5 must wait until Phases 1 through 3 have enough working
surface to build and publish representative native packages.

## Review Process

Before locking this umbrella packet in:

- Review it against the current codebase for stale types, stale paths, and
  unsupported distro drift.
- Review it against external research findings when that local artifact or
  pasted context is available; otherwise treat the research packet as optional
  background and rely on repo-grounded review.
- Review it against `docs/modules/ccs.md`, `docs/modules/remi.md`, and
  `docs/modules/recipe.md` for contradictions.
- Review it against `AGENTS.md` and `docs/llms/README.md` for assistant
  workflow fit.
- Confirm it does not read as an implementation plan that an agent should run
  directly.

Before starting any phase:

- Write a phase-specific design spec or combined design/plan if the phase is
  small enough.
- Run an agentic review of that phase packet before commit.
- Lock in the phase packet with a commit.
- Execute implementation in `/goal` slices.
- Merge, push, and clean up before the next slice unless the user explicitly
  chooses otherwise.

## Verification Gates For This Umbrella Packet

The umbrella packet itself is docs-only. Use lightweight documentation gates:

```bash
git diff --check
git diff --no-index --check /dev/null docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md
rg -n "TB[D]|TO[D]O" docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md
rg -n "(?i)\\b([c]entos|[r]hel|[d]ebian|[o]pensuse|[a]lpine|[t]umbleweed)\\b" docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md
rg -n "Fedora 4[4]|Ubuntu 26\\.[0]4|\\bArc[h]\\b" docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md
git status --short --branch
```

Expected results:

- `git diff --check` passes.
- The direct untracked-file whitespace check has no output; its nonzero exit
  code is expected before the file is tracked because the file differs from
  `/dev/null`.
- The placeholder sweep has no output.
- The unsupported-target sweep has no output.
- The supported-target sweep shows only intentional mentions of Fedora 44,
  Ubuntu 26.04, and Arch.
- `git status --short --branch` shows only intentional docs changes.

When the packet is locked in, explicitly refresh docs-audit state and verify
completion:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
```

If the ledger check reports this new roadmap as missing, add a reviewed ledger
row for the roadmap before committing.

## First Follow-Up Packet

The first deeper packet should be:

```text
CCS v2 Native Package Contract
```

It should focus only on Phase 1. It should not implement authoring workflow,
Remi publication, distro profile expansion, or proof-corpus work except where
Phase 1 tests require minimal fixtures.
