# Project Maintainability And Agent Readiness Umbrella Design And Plan

> **For agentic workers:** This is an umbrella roadmap, not a directly
> executable implementation plan. Do not run it as a single `/goal`. Each phase
> below must first receive its own deeper design and implementation plan, then
> be executed in focused `/goal` slices with `superpowers:subagent-driven-development`
> or `superpowers:executing-plans`.

**Goal:** Reshape Conary so future contributors and LLM-assisted development
sessions can understand, change, test, and review the project in smaller,
clearer, more disciplined slices while preserving the persisted data that would
matter to future users.

**Architecture:** Treat maintainability, contributor UX, and agent readiness as
one system. Large files, unclear ownership, stale docs, hidden verification
knowledge, and scattered fixtures are the same problem viewed from different
angles: the repo is harder to safely change than it needs to be.

**Tech Stack:** Rust, Cargo, SQLite, Conary CCS, Remi, conaryd, `conary-test`,
Markdown docs, existing docs-audit tooling, and focused verification scripts.

---

## Status

Draft umbrella packet for review.

This document is intentionally phase-level. It records the direction,
constraints, acceptance gates, and follow-up design packets needed before
implementation. It does not define every module move or task body. Deeper child
specs and implementation plans will own those details.

## Why This Exists

Conary is still early enough that the project can make bold organizational
changes without burdening real users. That is an advantage. The codebase has
serious functionality already: package installation, adoption, generations,
Remi, conaryd, CCS conversion, scriptlet safety, integration tests, bootstrap
work, and agent-facing operation surfaces. Some of that work now lives in files
that are too large to hold in one working context, and some contributor
knowledge lives in the head of the maintainer rather than in the repo.

This roadmap exists to turn that into a deliberate reset:

1. Put behavior where it belongs.
2. Make large files smaller for real architectural reasons, not mechanical
   churn.
3. Make subsystem ownership and verification obvious.
4. Make test fixtures and local proof loops contributor-facing APIs.
5. Make the repo easier for human contributors and LLM-assisted agents to work
   in without lowering engineering standards.

The target is not "vibe coding" as unchecked edits. The target is small,
reviewable, verifiable context packets that let a contributor or agent act
quickly because the repo itself explains the boundaries.

## Current Anchors

Read these before writing any child spec or implementation plan:

- `AGENTS.md`
- `CONTRIBUTING.md`
- `.github/PULL_REQUEST_TEMPLATE.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/modules/bootstrap.md`
- `docs/modules/ccs.md`
- `docs/modules/conaryd.md`
- `docs/modules/federation.md`
- `docs/modules/remi.md`
- `docs/modules/recipe.md`
- `docs/modules/source-selection.md`
- `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
- Relevant `docs/modules/*.md` files for the touched subsystem

Areas that child specs must inspect before proposing implementation:

Start from `docs/llms/subsystem-map.md`, then inspect the touched directories
with `rg --files` or equivalent. The list below is a roadmap-specific review
checklist, not a replacement for the subsystem map:

- `apps/conary/src/commands/`
- `apps/conary/src/commands/install/`
- `apps/conary/src/commands/ccs/`
- `apps/conary/src/commands/bootstrap/`
- `apps/conary/src/commands/adopt/`
- `apps/conary/src/commands/generation/`
- `apps/conary/src/cli/`
- `apps/conary/src/dispatch.rs`
- `apps/remi/src/server/`
- `apps/conaryd/src/daemon/`
- `apps/conary-test/src/`
- `crates/conary-core/src/ccs/`
- `crates/conary-core/src/repository/`
- `crates/conary-core/src/resolver/`
- `crates/conary-core/src/generation/`
- `crates/conary-core/src/model/`
- `crates/conary-core/src/db/`
- `crates/conary-agent-contract/src/`
- `crates/conary-mcp/src/`
- `data/`
- `recipes/`
- `scripts/`
- `docs/llms/`
- `docs/modules/`
- `docs/operations/`

## Non-Negotiable Constraints

- Persisted state is sacred. Database migrations, on-disk state, package
  archives, manifest compatibility, trust metadata, and generated artifact
  formats require explicit compatibility decisions before changes.
- Service-local persisted state is sacred too. Remi conversion state, cache
  metadata, analytics, federation peers, conaryd job queues, integration-test
  TOML manifests, `data/distros.toml`, and recipe version/checksum inputs such
  as `recipes/versions.toml` require the same compatibility discipline.
- Internal Rust APIs are not sacred. Workspace-only modules, helper types,
  command implementation structure, service internals, and test helpers may be
  broken and rebuilt when the new boundary is better.
- User-facing CLI can change when a child plan explicitly justifies the new
  surface and updates tests/docs. Do not preserve poor names or misleading
  flows only because they already exist.
- Do not weaken trust defaults, adoption safety, selected-generation safety,
  scriptlet replay gates, or host mutation safeguards casually. A child design
  may simplify a host-mutation acknowledgement only if it inventories the
  current protection, proposes an equally explicit replacement UX, and names
  the regression gates that preserve safety.
- Do not add compatibility shims for stale or fake surfaces solely to reduce
  refactor effort.
- Do not split files mechanically without naming the new ownership boundary.
- Do not add new docs that duplicate volatile details already owned elsewhere.
- Do not create broad nested `AGENTS.md` files. Add subtree guidance only when
  that subtree has genuinely different durable rules.
- Do not start implementation from this umbrella doc. Create and review a
  phase-specific design and implementation plan first.

## Current Hotspots

The following line counts were measured before this roadmap was drafted. They
are not failure by themselves, but they show where context boundaries are
already expensive:

| File | Lines | Initial Concern |
|------|------:|-----------------|
| `apps/conary/src/commands/install/mod.rs` | 4267 | CLI command orchestration, planning, rendering, and mutation concerns likely overlap |
| `apps/conary/src/commands/ccs/install.rs` | 3441 | Native package install UX and domain behavior are too hard to inspect together |
| `apps/conary/src/commands/update.rs` | 3334 | Source selection, planning, rendering, and execution need clearer homes |
| `apps/remi/src/server/conversion.rs` | 2999 | Conversion service behavior is difficult to target without server context |
| `crates/conary-core/src/scriptlet/mod.rs` | 2408 | Runtime result types, serde, planning, and tests deserve sharper boundaries |
| `apps/conaryd/src/daemon/routes.rs` | 2333 | Main routes file is still large despite the existing `routes/` subdirectory; complete extraction and move remaining domain behavior out |
| `apps/conary/src/commands/model.rs` | 2260 | Model parsing/editing/rendering responsibilities need clearer ownership |
| `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` | 2178 | CCS conversion metadata logic is large enough for subdomain modules |
| `apps/conary/src/dispatch.rs` | 2167 | Dispatch should stay thin and table-driven where possible |
| `crates/conary-core/src/generation/builder.rs` | 2147 | Generation build phases should expose smaller internal units |
| `apps/conary/src/commands/remove.rs` | 1990 | Remove flow likely mirrors install/update decomposition needs |
| `apps/conary/src/commands/bootstrap/mod.rs` | 1946 | Bootstrap command orchestration and stage logic should stay navigable |
| `crates/conary-core/src/model/replatform.rs` | 1927 | Policy, planning, and rendering boundaries should be reviewed |
| `crates/conary-core/src/resolver/provider/mod.rs` | 1881 | Provider internals need discoverable source ownership |
| `apps/conary-test/src/engine/runner.rs` | 1875 | Integration runner behavior is a contributor-facing API and should be approachable |
| `crates/conary-core/src/model/parser.rs` | 1872 | Model parsing responsibility is large enough for subdomain modules |
| `crates/conary-core/src/container/mod.rs` | 1855 | Namespace/container isolation details deserve sharper internal boundaries |
| `apps/conary/src/commands/install/batch.rs` | 1845 | Batch install logic should be separated from main orchestration where possible |
| `apps/conary/src/commands/system.rs` | 1829 | System command flows should be checked for command/domain separation |
| `apps/conary/src/commands/install/conversion.rs` | 1807 | Legacy conversion command integration should be decoupled from install orchestration |
| `crates/conary-core/src/repository/sync.rs` | 1752 | Repository sync state and network orchestration should be checked for separable units |
| `crates/conary-core/src/ccs/legacy_replay.rs` | 1720 | Scriptlet replay is safety-critical and needs explicit regression gates before decomposition |
| `crates/conary-core/src/ccs/convert/adapters.rs` | 1673 | Adapter-backed conversion evidence is large enough for ownership review |
| `crates/conary-core/src/bootstrap/image.rs` | 1669 | Bootstrap image construction is large and tied to high-cost validation |
| `crates/conary-core/src/ccs/convert/converter.rs` | 1618 | Conversion orchestration should be reviewed with CCS contract work |
| `crates/conary-core/src/generation/artifact.rs` | 1582 | Generation artifact metadata and validation are persisted-output adjacent |

This is an initial snapshot, not a canonical inventory. Child plans should
refresh these numbers before implementation with:

```bash
scripts/line-count-report.sh 30
```

If the script is unavailable in an older checkout, or a one-off shell refresh
is easier, use:

```bash
find apps crates -type f -name '*.rs' -exec wc -l {} + \
    | awk '$NF != "total" { print $1 "\t" $2 }' \
    | sort -rn -k1,1 \
    | awk 'NR <= 30'
```

New line-count thresholds should be used as review signals, not blunt rules:

- Over 1000 lines: ask whether the file still has one clear responsibility.
- Over 1500 lines: require a child plan to explain why new behavior belongs
  there.
- Over 2500 lines: treat decomposition as a phase candidate before adding
  substantial behavior.
- Over 3500 lines: avoid adding new behavior until a reviewed decomposition
  path exists, unless the task is an urgent fix.

## Discipline Principles

### Put Behavior Where It Belongs

Command files should parse arguments, call domain or service APIs, and render
results. They should not become the long-term home for resolver policy, package
contract rules, transaction logic, or server behavior.

Service route handlers should adapt requests and responses. Business logic
should live in service modules that HTTP, MCP, tests, and future transports can
reuse.

Core modules should separate data contracts, planning, execution, persistence,
and rendering when those responsibilities can change independently.

### Make Contributor Paths Explicit

Every major subsystem should answer these questions without requiring a maintainer
brain dump:

- Where do I start reading?
- What owns this behavior?
- What must not be weakened casually?
- What fixture or test proves the narrow behavior?
- What broader verification command proves integration did not drift?
- What docs must change when the behavior changes?

### Optimize For Small Verified Slices

The repo should help contributors and agents work in bounded chunks:

- Smaller files with meaningful module names.
- Focused test filters documented near the subsystem.
- Fixtures with stable names and clear ownership.
- Thin adapters around domain logic.
- Acceptance gates that run in minutes where possible.
- Full workspace gates reserved for changes with real workspace blast radius.

### Delete Misleading Surfaces

Because the project has no external users today, stale command names, dead
compatibility paths, misleading examples, and fake support claims should be
removed instead of preserved. Persisted data still gets careful migration
decisions; accidental internal API stability does not.

## Umbrella Design

The reset should move through seven phases:

1. **Repo Discipline Contract:** define enforceable expectations for module
   ownership, file size, public/private boundaries, persisted-state stability,
   and verification evidence.
2. **Dead Surface Inventory And API Pruning:** inventory stale surfaces and
   remove obvious, trivially verifiable cleanup before it becomes refactor cargo.
3. **Test And Fixture Discipline:** turn test helpers, golden fixtures, and
   integration manifests into predictable contributor-facing APIs.
4. **Hotspot Decomposition:** refactor the largest behavior-heavy files into
   smaller modules with clear ownership and focused tests.
5. **Subsystem Ownership Maps:** make docs and local guidance match the new
   code boundaries.
6. **Contributor Workflow UX:** improve task templates, verification recipes,
   fixture discovery, and newcomer paths.
7. **Enforcement And Drift Control:** add lightweight checks that keep the
   reset from quietly decaying.

The phases can overlap only when a child plan proves the touched files and
verification gates are independent. The default should be serial child plans
with clean commits between slices.

## Phase 1: Repo Discipline Contract

**Purpose:** Define the project-wide engineering discipline this reset will
enforce.

**Problem:** Conary has strong local practices, but too many of them are
implicit. Contributors can find `AGENTS.md` and the subsystem map, but the repo
does not yet define concrete expectations for file-size pressure, module
ownership, acceptable breakage, fixture ownership, or review gates.

**Phase output:** A reviewed discipline contract and implementation plan for
the smallest enforcement/documentation changes needed to make the contract real.

**Scope candidates for the child design:**

- Define file-size review thresholds and how exceptions are justified.
- Define persisted-state stability categories.
- Define what internal APIs may be broken freely.
- Define command/service/core module boundary expectations.
- Define when a nested `AGENTS.md` is appropriate.
- Define required documentation updates for subsystem moves.
- Define minimum verification evidence for refactor-only slices.
- Define how to name child specs/plans and `/goal` slices for maintenance work.
- Decide whether a lightweight line-count report belongs in `scripts/`.
- Define the minimum contributor/agent packet required before the first risky
  decomposition: child-plan template, read-first list, fixture/test ownership,
  fast/medium/full verification recipe, and expected final evidence format.

**Explicit non-goals:**

- Do not refactor hotspot files in Phase 1.
- Do not add heavyweight CI bureaucracy.
- Do not create nested guidance in every directory.
- Do not rewrite the existing CCS native ecosystem roadmap.
- Do not split `conary-core` into new workspace crates as a default response to
  file size. A new crate boundary needs a child design proving ownership,
  dependency, and compilation-coupling benefits beyond smaller files.

**Acceptance gate for Phase 1 child plan:**

- The contract distinguishes persisted state from internal APIs.
- The contract defines review signals for large files without creating a
  brittle hard failure.
- The contract names at least one narrow verification command for docs-only
  discipline changes.
- The contract updates assistant-facing docs without duplicating volatile
  subsystem inventories.
- The first risky decomposition has a minimum contributor/agent packet rather
  than relying on unwritten maintainer knowledge.

## Phase 2: Dead Surface Inventory And API Pruning

**Purpose:** Remove stale, misleading, or unnecessary surfaces that make the
repo harder to understand before they become refactor cargo, starting with an
inventory and obvious stale-surface cleanup.

**Problem:** Early projects accumulate names, docs, examples, helper APIs, and
compatibility assumptions that no longer match the intended direction. Since
Conary has no external users today, this is the moment to delete or reshape
them. Pruning should happen before the largest decompositions when it can
reduce the refactor surface, but behavior-changing deletion still needs tests
and explicit compatibility review. If existing focused tests do not prove the
deletion, defer the deletion until Phase 3 establishes the fixture/test gate.

**Phase output:** A pruning design and implementation plan.

**Scope candidates for the child design:**

- Remove stale CLI aliases or misleading help text.
- Remove unsupported distro wording and examples outside current support.
  Distro-support pruning must include `data/distros.toml` and not only docs or
  CLI help text.
- Delete dead helper APIs after proving no workspace usage remains.
- Narrow overly broad re-exports that make ownership unclear.
- Archive completed docs and remove active-path references to obsolete plans.
- Remove stale test helpers that encode no longer desired behavior.
- Normalize command names and docs examples when the current surface is
  inconsistent.
- Simplify repository-level policy metadata, such as license declarations, when
  the repo has one desired public policy and a child plan inventories every
  tracked and host-service surface that needs to match it.
- Decide the fate of leftover ignored or host-local tool harness files, such as
  `.claude/settings.local.json`, without reintroducing tracked tool-specific
  entrypoints or parallel assistant guidance.

**Explicit non-goals:**

- Do not delete persisted-state migration paths without a compatibility design.
- Do not hide breaking user-facing changes inside unrelated refactors.
- Do not remove safety checks because they are inconvenient to route through
  the new module boundary.
- Do not use pruning as an excuse to delete hard but necessary behavior.

**Acceptance gate for Phase 2 child plan:**

- Each deletion has a repo-grounded reason.
- Usage searches are recorded.
- User-facing changes include docs and tests.
- Persisted-state changes, if any, have explicit migration coverage.
- Distro, data, or recipe changes include a reference sweep covering
  `data/distros.toml`, `recipes/versions.toml`, `recipes/**`, Remi public
  source targets, source-selection persisted mirrors, target compatibility or
  scriptlet replay target IDs, and `conary-test` manifests.
- The child plan distinguishes obvious stale-surface cleanup from behavior
  changes that need a stronger test gate.
- Behavior-changing deletions either name existing focused tests or explicitly
  defer until fixture/test coverage is clarified.

## Phase 3: Test And Fixture Discipline

**Purpose:** Make tests and fixtures stable enough to serve as contributor APIs.

**Problem:** Tests are numerous and useful, but some fixture logic is embedded
inside large test files or helper modules whose ownership is not obvious. As
Conary grows, contributors need to know which fixture shape proves which
behavior and which test filter is the right first gate. The safety net should
be clearer before major hotspot decompositions start.

**Phase output:** A test/fixture discipline design and implementation plan.

**Scope candidates for the child design:**

- Inventory fixture families across `crates/conary-core/tests/`,
  `apps/conary/tests/`, `apps/remi/src/server/`, and `apps/conary-test/src/`.
- Define the fixture discovery map shape: fixture family, owning subsystem,
  generator or regeneration command, consuming tests, fast proof, slow proof,
  and whether the metadata is hand-maintained, generated, or script-checked.
- Identify helper modules that should become stable test-support APIs.
- Define golden fixture naming and metadata conventions.
- Separate large integration tests by behavior when useful.
- Standardize exact test filters in child plans.
- Decide where synthetic package builders belong.
- Decide when a fixture should be source text, generated in test code, or
  checked in as an artifact.
- Add negative fixtures for safety boundaries that must not regress.
- Improve `conary-test` failure diagnostics where runner output currently
  forces maintainers to inspect logs manually.
- Identify long-running integration and QEMU gates, then document the fastest
  valid local proof loop before the full gate.
- Treat integration-test manifests under
  `apps/conary/tests/integration/remi/manifests/` as persisted test
  configuration: schema or semantics changes require migration decisions and
  inventory updates.

**Explicit non-goals:**

- Do not move fixtures merely for aesthetics.
- Do not check in bulky artifacts without a size and regeneration decision.
- Do not collapse fast unit tests into slow integration tests.
- Do not hide real host or network assumptions in fixtures.
- Do not make slow QEMU gates disappear from release criteria just because
  faster local proof loops exist.

**Acceptance gate for Phase 3 child plan:**

- Fixture ownership is documented.
- Test filters are stable enough for `/goal` verification gates.
- Moved helpers have behavior-preserving tests.
- At least one high-value fixture family becomes easier to reuse.
- The fixture map records owner, generator/regeneration command, consuming
  tests, and fast/slow proof commands for each family it covers.
- `conary-test` diagnostic improvements define the minimum failure output:
  suite/test id, step index/name, command, exit code, assertion mismatch,
  stdout/stderr tail or `conary-test logs` command, artifact/result path,
  focused rerun command, and infra-vs-assertion classification.
- Slow gates have a documented quick precursor when a precursor is technically
  valid.
- Manifest schema or semantic changes require `cargo run -p conary-test -- list`,
  focused manifest parser/inventory tests, and an inventory, migration, or
  defaulting decision.

## Phase 4: Hotspot Decomposition

**Purpose:** Reduce the largest behavior-heavy files by moving responsibilities
to the modules that should own them.

**Problem:** Several files are large enough that every change requires scanning
multiple concerns at once. This slows humans down and makes agentic work more
error-prone because a single file may contain CLI parsing, planning, rendering,
domain decisions, persistence, fixtures, and tests.

**Phase output:** A set of child designs/plans for the highest-value hotspot
decompositions.

**Roadmap coordination:** Decompositions of
`apps/conary/src/commands/ccs/install.rs`, `apps/remi/src/server/conversion.rs`,
and core CCS conversion modules must be scheduled with the CCS native ecosystem
roadmap at `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`.
That roadmap is contract-first, so CCS v2 native package contract work can
proceed before CLI hotspot decomposition. CCS hotspot decomposition should be
driven by the active CCS child spec, not file size alone. If a CCS child plan
needs the same file, prefer a decomposition slice that directly reduces that
child plan's risk, then merge before the CCS feature slice starts.

**Scope candidates for child designs:**

- `apps/conary/src/commands/install/`: split command parsing/orchestration from
  planning, renderable summaries, mutation preflight, replay policy integration,
  and transaction execution.
- `apps/conary/src/commands/ccs/install.rs`: separate CCS package inspection,
  signature/trust checks, component selection, install planning, and CLI
  rendering.
- `apps/conary/src/commands/update.rs`: separate source-policy inputs, update
  planning, replatform rendering, execution, and output.
- `apps/remi/src/server/conversion.rs`: move conversion business logic behind
  service APIs that handlers, admin flows, and tests can target directly.
- `apps/conaryd/src/daemon/routes.rs`: complete the already-started extraction
  into `apps/conaryd/src/daemon/routes/`, keep route definitions thin, and move
  daemon operation behavior into route modules or service units.
- `apps/conary/src/dispatch.rs`: move toward a more table-driven or delegated
  dispatch shape if the child design proves it improves clarity.
- `crates/conary-core/src/scriptlet/mod.rs`: split runtime outcome types,
  serde compatibility, replay planning, and test fixtures where those are
  separable.
- `crates/conary-core/src/ccs/convert/`: split large conversion bundle and
  adapter files by evidence source, decision planning, and metadata assembly.
- `crates/conary-core/src/ccs/legacy_replay.rs`: explicitly decide whether
  legacy replay planning belongs in scope for a CCS/scriptlet decomposition;
  preserve replay refusal, sandbox, lifecycle, timeout, and target-compatibility
  gates.
- `crates/conary-core/src/generation/builder.rs`: expose build phases as
  smaller internal units with clear contracts.
- `crates/conary-core/src/model/parser.rs` and
  `crates/conary-core/src/model/replatform.rs`: separate parsing, policy, and
  rendering responsibilities where child analysis supports it.
- `crates/conary-core/src/container/mod.rs`: separate namespace/container
  isolation concepts that can be tested independently.
- `apps/conary/src/commands/install/batch.rs` and
  `apps/conary/src/commands/install/conversion.rs`: make install subflows
  clearer before adding more install behavior.
- `apps/conary/src/commands/bootstrap/mod.rs`: separate command orchestration
  from bootstrap stage logic; child plans must include bootstrap readiness and
  dry-run smoke verification when behavior moves.
- `apps/conary-test/src/engine/runner.rs`: separate manifest execution,
  runtime environment management, reporting, and failure capture.
- `crates/conary-agent-contract/src/` and `crates/conary-mcp/src/`: review
  operation-contract and MCP-adapter boundaries when maintenance work touches
  agent-facing operation surfaces; keep the contract transport-neutral.

**Explicit non-goals:**

- Do not rewrite behavior just to reduce line counts.
- Do not preserve an old module path if it encodes the wrong ownership.
- Do not change persisted formats unless a child plan explicitly scopes that
  compatibility work.
- Do not combine multiple high-risk hotspots in one `/goal` merely because both
  are listed in this phase.

**Acceptance gate for each hotspot child plan:**

- The plan names the current responsibilities in the file.
- The plan names the target module ownership and why it is better.
- The plan identifies behavior-preserving tests before moving code.
- The plan includes at least one focused verification command.
- The plan updates subsystem docs or maps if the "look here first" path changes.
- The child plan names the new owner module, shows the old entrypoint becoming
  thinner or more delegated, and names the focused tests that prove the moved
  behavior.
- For decompositions touching `scriptlet/`, `ccs/convert/`, or
  `ccs/legacy_replay.rs`, the child plan names the specific scriptlet replay
  regression tests that must pass before acceptance.
- For Remi conversion decomposition, the child plan preserves scriptlet
  publication gating, private `review_artifact_path` handling, cache/chunk
  metadata semantics, and federation peer/config state.
- For conaryd route/job decomposition, the child plan preserves the
  `daemon_jobs` schema, idempotency, queued/running restart behavior, SSE
  lifecycle, and live-host mutation acknowledgement.
- For install/update/remove/adopt/conaryd package-job flows, the child plan
  names host-mutation and adoption-authority regression gates, including
  relevant `cargo test -p conary --test cli_daily_ux` filters and
  `cargo run -p conary-test -- run --suite phase3-active-generation-handoff`
  when selected-generation handoff paths are touched.
- For bootstrap decomposition, the child plan includes
  `cargo run -p conary-test -- bootstrap check --json` and
  `cargo run -p conary-test -- bootstrap smoke --dry-run --json`, or explains
  why the touched code does not affect those flows.

## Phase 5: Subsystem Ownership Maps

**Purpose:** Consolidate and audit docs and assistant guidance so they reflect
the real code boundaries after rolling map updates land with decomposition
slices.

**Problem:** Conary already has strong assistant-facing docs, but those maps
can drift as code moves. Map updates should land with the slice that changes a
"look here first" path; this phase audits and consolidates those updates so
contributors do not land in stale or overly broad guidance. If a contributor
reads `docs/llms/subsystem-map.md` and then lands in a 3000-line file, the map
is only half doing its job.

**Phase output:** Updated subsystem maps and local guidance patterns that match
the decomposed code.

**Scope candidates for the child design:**

- Refresh `docs/llms/subsystem-map.md` after major module moves.
- Refresh affected `docs/modules/*.md` files with stable entry points.
- Add narrow "change guide" sections only where they reduce repeated
  maintainer explanation.
- Add feature or capability ownership cards for the actual useful pieces of
  Conary: what the feature does, which files own it, which neighboring systems
  it depends on, which changes require broader verification, and where a
  contributor can work on that feature without being surprised by hidden
  coupling.
- Add nested `AGENTS.md` only for subtrees with distinct durable rules, such as
  generated fixture trees or integration-test manifests, if justified.
- Define how docs should refer to focused test commands without becoming stale.
- Decide where contributor-facing architecture diagrams or flow outlines belong
  when prose alone is not enough.

**Explicit non-goals:**

- Do not turn `AGENTS.md` into a manual.
- Do not duplicate implementation details from Rust files into docs.
- Do not create tool-specific parallel guidance for every assistant.
- Do not add broad doc promises about unsupported future features.

**Acceptance gate for Phase 5 child plan:**

- A fresh contributor can find the owning module for each touched subsystem.
- Assistant-facing docs point to canonical human docs rather than duplicating
  them.
- Stale paths and retired guidance are removed or archived.
- Docs-audit tooling is updated if tracked doc inventory changes.

## Phase 6: Contributor Workflow UX

**Purpose:** Make common contribution flows discoverable, repeatable, and easy
to verify.

**Problem:** The repo has strong verification commands, but the narrow command
for a particular subsystem is often discovered by experience. New contributors
and agents need a clear path from "I want to change X" to "I know which files
to read, which fixtures to use, and which command proves it." Developer UX also
includes feedback-loop speed: build cache guidance, quick compile/test gates,
clear runner diagnostics, and honest labels for slow QEMU/KVM validation.

**Phase output:** A contributor workflow UX design and implementation plan.

**Scope candidates for the child design:**

- Task templates for design, plan, implementation, review, and cleanup slices.
- Subsystem verification recipes with narrow, medium, and full gates.
- "Good first slice" guidance for safe maintenance tasks.
- A fixture discovery map for core, CLI, Remi, conaryd, and `conary-test`.
- A convention for naming integration manifests and golden fixtures.
- A convention for reporting verification in final answers, PR bodies, and
  roadmap task completions.
- Human workflow updates in `CONTRIBUTING.md` and
  `.github/PULL_REQUEST_TEMPLATE.md`, with assistant-specific routing kept in
  `AGENTS.md` and `docs/llms/README.md`.
- Optional scripts that print suggested verification gates for a changed path.
- Feature-focused workflow recipes that let a contributor pick a Conary
  capability, read the right local packet, and see both the narrow proof and
  the cross-system proof before editing.
- Review prompts for external assistants that ask for repo-grounded findings,
  not generic advice.
- Compile-cache guidance for local development, including how to use sccache
  when available without assuming a tracked shared cache directory, and how the
  existing `Cargo.toml` dev incremental and `fast-release` profile fit the
  feedback loop.
- Test iteration guidance that separates fast local checks, medium package
  checks, and slow QEMU/KVM release evidence.
- `conary-test` diagnostics improvements when a failing run needs clearer
  next-step output.
- A live-system mutation UX review that decides whether the current explicit
  acknowledgement flag should remain, be renamed, or be replaced by a lower
  friction flow with equivalent safety evidence.

**Explicit non-goals:**

- Do not create a separate contributor manual that duplicates all module docs.
- Do not replace code review with generated checklists.
- Do not require contributors to run full workspace gates for every small
  docs-only or module-local change.
- Do not make local speedups depend on a maintainer's host-local paths.

**Acceptance gate for Phase 6 child plan:**

- A new contributor can select a subsystem and find a read-first list.
- A contributor can find the focused verification command for a small change.
- Fixture ownership and naming are documented where they matter.
- `CONTRIBUTING.md` and `.github/PULL_REQUEST_TEMPLATE.md` carry human
  contribution workflow updates, while `AGENTS.md` and `docs/llms/README.md`
  remain the top-level assistant entrypoints.
- The workflow guidance names at least one fast path for compile/test feedback
  and one explicit slow path for release-grade validation.

## Phase 7: Enforcement And Drift Control

**Purpose:** Keep the maintainability reset from decaying after the first pass.

**Problem:** Refactors help once. Lightweight drift control keeps the repo from
returning to the same shape under delivery pressure.

**Phase output:** A lightweight enforcement design and implementation plan.

**Scope candidates for the child design:**

- Line-count report script for Rust files and maybe docs.
- Changed-path to suggested-verification script.
- Docs path drift checks for assistant-facing maps.
- Fixture inventory checks.
- Optional CI/reporting jobs that warn rather than fail at first.
- A release or maintenance checklist section for large module additions.
- Periodic "hotspot report" command that can be pasted into planning sessions.

**Explicit non-goals:**

- Do not add noisy CI that developers learn to ignore.
- Do not fail builds purely because a file crosses a line threshold.
- Do not require network services for local maintainability checks.
- Do not turn every style preference into a script.

**Acceptance gate for Phase 7 child plan:**

- Checks are cheap enough to run locally.
- Warnings explain what to do next.
- The checks reinforce existing docs rather than creating hidden policy.
- The first run reports actionable paths, owner hints where available, and
  suggested next commands instead of only counts.

## Suggested Child Design Order

1. Repo Discipline Contract.
2. Dead Surface Inventory And API Pruning, starting with obvious stale surfaces
   and an inventory of deletions that need stronger tests.
3. Test And Fixture Discipline, so the refactor safety net is clearer before
   large files move.
4. One CLI hotspot decomposition chosen by current product risk. If the active
   CCS child spec is still contract-first, let CCS v2 contract work proceed
   before `conary ccs install` decomposition.
5. One service hotspot decomposition, preferably Remi conversion or conaryd
   routes, sequenced around CCS native ecosystem work.
6. Subsystem ownership map refresh after the first decompositions land.
7. Contributor workflow UX, starting with the test/fixture discovery map and
   verification recipes that were proven during the first decompositions.
8. Enforcement and drift control.

This order is intentionally pragmatic. If the CCS native ecosystem roadmap needs
a specific module first, choose the maintenance child design that reduces that
roadmap's immediate risk, merge it cleanly, then start the CCS feature slice.

Some outputs can land incrementally before their phase formally completes:

- A line-count script from Phase 7 can be produced during Phase 1 as a concrete
  discipline-contract artifact.
- Subsystem map updates from Phase 5 should happen in the same slice that
  changes a "look here first" path, not wait for a separate docs batch.
- Dead code found during Phase 4 decomposition may be deleted in the same slice
  when the deletion is trivially verifiable. Complex or risky deletions still
  defer to Phase 2 pruning.
- Child plans should declare which incremental deliveries they include.

First shippable artifacts by phase:

- Phase 1: discipline contract updates plus a line-count report command or script.
- Phase 2: pruning inventory plus at least one obvious stale-surface cleanup.
- Phase 3: fixture discovery map for one high-value subsystem.
- Phase 4: one reviewed hotspot child plan and one behavior-preserving slice.
- Phase 5: rolling map updates audited against the changed subsystem paths.
- Phase 6: one human/assistant workflow recipe slice, ideally tied to a real
  feature ownership packet.
- Phase 7: warn-only drift report with actionable paths and next commands.

## Seeded Follow-Up Child Designs

These ideas are intentionally recorded here so they do not live only in chat
history. They are not implementation permission on their own; each one needs a
reviewed child design and implementation plan before code or repo-policy changes
land.

### Feature Ownership And Interaction Gates

**Fits:** Phase 5 plus Phase 6.

Create a feature-focused map for the actual capabilities people may want to
work on independently: native package installation, adoption and unadoption,
generation build/switch/recovery, CCS authoring/install, Remi publication and
serving, conaryd package jobs, bootstrap/self-hosting, integration-test
execution, and agent/MCP operation surfaces. Each entry should name:

- The short capability description.
- The owning files and docs.
- The nearby systems it interacts with.
- The focused test or command for a small edit.
- The broader verification gate required when the edit changes behavior across
  a boundary.
- The docs that must change when the feature moves.

The child design should make it easy to work on one cool Conary capability in
isolation while still making cross-system coupling visible. For example, a Remi
publication change that affects CCS package availability should point at both
Remi tests and the relevant CCS/client verification gate.

### MIT-Only License Simplification

**Fits:** Phase 2 plus Phase 6.

Review and simplify the repository license story to MIT-only if the desired
project policy is a single MIT license. The child design should inventory
`LICENSE`, `README.md`, `CONTRIBUTING.md`, Cargo package metadata, generated
release metadata, GitHub repository metadata, and any docs or templates that
state contribution licensing. The implementation should update tracked files
and leave any host-service setting changes as explicit operator steps when they
cannot be represented in git.

### Live-System Mutation Acknowledgement UX

**Fits:** Phase 6, with Phase 2 pruning only after the safety replacement is
specified.

Review whether the current live-system mutation acknowledgement flow is still
earning its friction. The child design should inventory commands and tests that
depend on the flag, define which operations are genuinely dangerous, and
compare options such as a shorter flag, contextual confirmation, dry-run-first
guidance, or `--yes` plus operation-specific risk prompts. Any replacement must
preserve the core safety property: a command that mutates the active host should
not do so accidentally or silently.

Minimum gates for that child design should include focused CLI diagnostics,
live-host safety tests, generation/adoption handoff tests when touched, and
conaryd package-job tests when daemon execution inherits the same policy.

## Verification Strategy

Each child plan must define exact commands. The umbrella default is:

- Docs-only changes:
  - `git diff --check`
  - relevant `rg` sweeps for stale paths or retired names; when `rg` is not
    available, use a `find ... -type f ... | xargs grep -n ...` fallback named
    by the child plan
  - docs-audit scripts when tracked doc inventory changes
- Rust refactor changes:
  - `cargo build -p <package>`
  - `cargo test -p <package>`
  - focused package test for the touched subsystem
  - `cargo fmt --check`
  - `cargo clippy -p <package> --all-targets -- -D warnings` when practical
  - `git diff --check`
- Cross-crate behavior changes:
  - focused tests for each owning package
  - broader workspace checks only when the touched boundary justifies them
- Integration-test harness changes:
  - `cargo run -p conary-test -- list`
  - focused `conary-test` suite or dry-run command named by the child plan
- Generation, filesystem, or boot-artifact changes:
  - focused package tests named by the child plan
  - `scripts/local-qemu-validation.sh` or a narrower local image-path proof
    only when the touched behavior affects release-grade QEMU evidence
- Persisted-state or format changes:
  - classify the touched state or format before editing
  - name backward-read, migration, or round-trip tests
  - cover relevant SQLite migrations, `.ccs` archives,
    `MANIFEST.cbor`/`MANIFEST.toml`, Remi `converted_packages`, cache, and
    federation state, conaryd `daemon_jobs`, bootstrap JSON/DB artifacts,
    `conary-test` manifests, `data/distros.toml`, and recipe TOML inputs

No child plan should claim completion on line-count reduction alone. The proof
is clearer ownership plus preserved or intentionally changed behavior.

## Verification Gates For This Umbrella Packet

Before locking in this roadmap:

- `test -z "$(git diff --no-index --check /dev/null docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md)"`
  while the file is untracked, or `git diff --check` after it is staged/tracked.
- Placeholder and stale-surface sweep:
  `for term in T''BD TO''DO FI''XME Cent''OS RH''EL "Debian sta""ble" open''SUSE Al''pine CLAU''DE Cla''ude "Open Review"" Questions"; do rg -n "$term" docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md; done`
- Path existence checks for every backticked path added by review.
- `git status --short --branch`
- Docs-audit scripts if this packet changes tracked docs inventory or ledger
  expectations.

## Review Checklist For This Umbrella

- Does it preserve persisted data while allowing bold internal cleanup?
- Does it treat contributor UX as first-class work?
- Does it avoid adding unsupported distro or compatibility promises?
- Does it avoid mechanical refactor churn?
- Does it define enough phase structure for child specs to begin?
- Does it keep `AGENTS.md` and `docs/llms/README.md` as the top-level assistant
  entrypoints?
- Does it make future CCS-native work easier rather than competing with it?
- Does each child plan touching persisted state, trust, host mutation, or
  scriptlet replay name the exact compatibility or safety regression tests?

## Resolved Review Decisions

- **Phase 1 output location:** Default to updating `AGENTS.md`,
  `docs/llms/README.md`, and focused module docs directly. Add a standalone
  discipline contract only if the Phase 1 child design proves the policy would
  otherwise bloat those map-style files.
- **Line-count reporting:** Add a lightweight POSIX-friendly reporting script
  under `scripts/`, and keep the ad hoc planning command in the roadmap for
  quick one-off refreshes.
- **First hotspot leverage:** Let the active CCS child spec drive CCS-adjacent
  hotspot work. If CCS v2 contract work is active, core contract design can
  proceed before CLI decomposition. Treat `apps/conary/src/commands/ccs/install.rs`
  as the default first CLI hotspot only when install UX or package install
  behavior is the active risk.
- **First contributor UX artifact:** Build the test/fixture discovery map
  first, then use it to drive verification recipes and task templates.
