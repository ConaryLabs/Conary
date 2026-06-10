# Project Maintainability Phase 3 Test And Fixture Discipline Design And Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` or `superpowers:executing-plans`
> to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for
> tracking. This is a Phase 3 child packet under
> `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Make Remi and CCS conversion/publication fixtures discoverable as a
contributor-facing API, then use that first map as the pattern for later
fixture discipline slices.

**Architecture:** Start with a documentation-first fixture ownership map rather
than moving fixtures. The first executable slice creates a canonical
`docs/modules/test-fixtures.md` map for Remi plus CCS conversion/publication
fixtures, routes assistant and module docs to it, and records fast/medium/slow
verification gates that future feature and decomposition plans can reuse.

**Tech Stack:** Rust, Cargo tests, Conary CCS conversion fixtures, Remi
publication tests, `conary-test` TOML manifests, Markdown docs, existing
docs-audit tooling.

---

## Status

Draft packet for review.

This packet is intentionally scoped to Phase 3's first fixture-discipline
slice. It does not move fixture helpers, rewrite test modules, change
`conary-test` output, or alter Remi/CCS behavior. Those are later Phase 3 or
Phase 4 implementation slices once the map shape is proven.

## Read First

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/modules/ccs.md`
- `docs/modules/remi.md`
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
- `docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md`

## Design

Phase 3 should turn important fixtures into named contributor APIs. The first
map does that for Remi and CCS conversion/publication because this area is both
high-value and easy to regress:

- CCS conversion has golden fixture IDs and support-matrix rows that define
  public-ready, review-required, blocked, legacy-replay, and rejected outcomes.
- Conary CLI tests have synthetic legacy scriptlet bundle builders used by
  install, remove, upgrade, query, and foreign replay tests.
- Remi has publication gate tests that decide which converted rows can be
  advertised, indexed, searched, and served.
- `conary-test` manifests and results are the slow evidence layer for repository
  and native package-manager flows.

The map should describe each fixture family with the same fields:

- **Family ID:** stable lowercase id for child plans.
- **Owner:** owning subsystem and first file to read.
- **Purpose:** behavior the fixture proves.
- **Fixture sources:** checked-in files or in-test builders.
- **Consumes:** tests or commands that use the fixtures.
- **Fast proof:** narrow local command for small edits.
- **Medium proof:** package-level or cross-package command.
- **Slow proof:** integration or QEMU command when applicable.
- **Regeneration:** command or explicit statement that the fixture is
  hand-maintained.
- **Safety notes:** unsupported public targets, private paths, host mutation,
  scriptlet replay, or publication gates that must not be weakened.

The first implementation should create the schema and initial rows only. Later
Phase 3 packets can add:

- `conary-test` runner diagnostics improvements.
- Native package corpus fixture ownership.
- Bootstrap and QEMU source-image fixture maps.
- Generation artifact/export fixture maps.
- Feature ownership cards that attach fixture gates to user-visible Conary
  capabilities.

## Non-Goals

- Do not move fixture files in this slice.
- Do not rewrite Remi conversion or publication behavior.
- Do not change `conary-test` manifest semantics.
- Do not add new distro support or imply public support beyond Fedora 44,
  Ubuntu 26.04, and Arch.
- Do not add slow QEMU gates as required proof for docs-only fixture-map edits.
- Do not duplicate volatile test counts in assistant-facing map files.
- Do not turn archived historical docs into active fixture ownership.

## Current Repo-Grounded Fixture Families

| Family | Current sources | Current proof |
|--------|-----------------|---------------|
| CCS conversion golden cases | `crates/conary-core/src/ccs/convert/golden_fixtures.rs`, `crates/conary-core/src/ccs/convert/support_matrix.rs`, `crates/conary-core/src/ccs/convert/adapters.rs`, `crates/conary-core/src/ccs/convert/blocked_classes.rs` | `cargo test -p conary-core golden_fixtures`, `cargo test -p conary-core support_matrix` |
| Conary synthetic legacy bundle fixtures | `apps/conary/tests/common/legacy_scriptlet_fixtures.rs`, `bundle_replay.rs`, `foreign_replay.rs`, `query_scripts.rs` | `cargo test -p conary --test bundle_replay`, `cargo test -p conary --test foreign_replay`, `cargo test -p conary --test query_scripts` |
| Remi publication gate fixtures | `apps/remi/src/server/publication.rs`, `conversion.rs`, `index_gen.rs`, `prewarm.rs`, handler tests under `apps/remi/src/server/handlers/` | `cargo test -p remi publication`, `cargo test -p remi persisted_goal8a_golden_outcomes_respect_publication_gate` |
| Remi test artifact fixtures | `apps/remi/src/server/handlers/admin/artifacts.rs`, `apps/remi/src/server/handlers/artifacts.rs`, `apps/remi/src/server/artifact_paths.rs` | `cargo test -p remi test_upload_fixture`, `cargo test -p remi test_public_fixture_get_and_head` |
| `conary-test` manifest suites | `apps/conary/tests/integration/remi/manifests/`, `apps/conary-test/src/config/`, `apps/conary-test/src/suite_inventory.rs` | `cargo run -p conary-test -- list`, `cargo test -p conary-test suite_inventory` |

## Implementation Plan

### Task 0: Lock Reviewed Phase 3 Plan And Docs-Audit Metadata

**Files:**
- Add: `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Stage the reviewed plan before regenerating docs inventory**

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md
```

- [ ] **Step 2: Regenerate docs-audit inventory**

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected: the inventory includes:

```text
docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md	planning	maintainer
```

- [ ] **Step 3: Add the plan ledger row**

Add this literal-tab row to
`docs/superpowers/documentation-accuracy-audit-ledger.tsv` near the other active
maintainability plan rows:

```text
docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md	docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md	planning	maintainer	maintainability; phase3; test-fixtures; fixture-discipline; remi; ccs	AGENTS.md; docs/llms/README.md; docs/llms/subsystem-map.md; docs/ARCHITECTURE.md; docs/INTEGRATION-TESTING.md; docs/modules/ccs.md; docs/modules/remi.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md; crates/conary-core/src/ccs/convert/golden_fixtures.rs; crates/conary-core/src/ccs/convert/support_matrix.rs; apps/conary/tests/common/legacy_scriptlet_fixtures.rs; apps/remi/src/server/publication.rs; apps/remi/src/server/conversion.rs; apps/conary/tests/integration/remi/manifests	verified	corrected	Added the reviewed Phase 3 plan for the first test and fixture discipline slice: a Remi plus CCS conversion/publication fixture ownership map, exact proof recipes, and docs routing without moving fixtures.
```

- [ ] **Step 4: Update the audit summary for the active Phase 3 plan**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, rename
`### 2026-06-06 Maintainability Phase 2 Planning` to
`### 2026-06-06 Maintainability Planning` if that header still has the Phase 2
suffix. Then append this paragraph to that maintainability planning section:

```markdown
The Phase 3 test and fixture discipline plan now opens the fixture-map lane for
Remi and CCS conversion/publication evidence. It keeps the first slice
documentation-first so fixture ownership and verification recipes are explicit
before helper moves or hotspot decompositions begin.
```

Then update the final counts to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 144
- `verified-no-change`: 14
- `corrected`: 43
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its evidence and
notes mention the Phase 3 fixture-discipline planning update.

- [ ] **Step 5: Verify docs-audit and diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --cached --check
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit the reviewed plan lock-in**

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan fixture discipline slice"
```

### Task 1: Add The Initial Fixture Map

**Files:**
- Add: `docs/modules/test-fixtures.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Create the fixture map document**

Create `docs/modules/test-fixtures.md` with this content:

````markdown
---
last_updated: 2026-06-06
revision: 1
summary: Map Remi and CCS conversion/publication fixture ownership and proof gates
---

# Test Fixtures And Proof Maps

This module records fixture families that future contributors and agents can
treat as stable proof surfaces. It does not replace the tests themselves. It
answers where a fixture lives, what behavior it proves, which tests consume it,
and which verification command is the right first gate.

CCS means Conary Content Store in this map.

## Fixture Map Schema

Each fixture family should record:

- **Family ID:** stable lowercase id used by child plans.
- **Owner:** subsystem and first source file to inspect.
- **Purpose:** behavior the fixture proves.
- **Fixture sources:** checked-in files or in-test builders.
- **Consumes:** tests or commands that use the fixtures.
- **Fast proof:** narrow local command for small edits.
- **Medium proof:** package-level or cross-package command.
- **Slow proof:** integration or QEMU command when applicable.
- **Regeneration:** command or hand-maintained status.
- **Safety notes:** public-target, scriptlet, trust, host mutation, private-path,
  or publication boundaries.

## Remi And CCS Conversion/Publication Fixture Families

| Family ID | Owner | Fast proof |
|-----------|-------|------------|
| `ccs-convert-golden-cases` | CCS convert | `cargo test -p conary-core golden_fixtures`; `cargo test -p conary-core support_matrix` |
| `legacy-scriptlet-bundle-fixtures` | Conary CLI tests | `cargo test -p conary --test bundle_replay synthetic_legacy_bundle_fixtures_cover_task5_matrix` |
| `remi-scriptlet-publication-gate` | Remi server publication | `cargo test -p remi publication` |
| `remi-test-artifact-fixtures` | Remi artifact handlers | `cargo test -p remi test_upload_fixture`; `cargo test -p remi test_public_fixture_get_and_head` |
| `conary-test-remi-manifests` | Integration harness | `cargo run -p conary-test -- list`; `cargo test -p conary-test suite_inventory` |

### ccs-convert-golden-cases

- **Owner:** CCS convert:
  `crates/conary-core/src/ccs/convert/golden_fixtures.rs`.
- **Purpose:** Stable expected outcomes for native-free, fully replaced,
  legacy replay, review-required, blocked, and rejected conversion cases.
- **Fixture sources:**
  `crates/conary-core/src/ccs/convert/golden_fixtures.rs`;
  `crates/conary-core/src/ccs/convert/support_matrix.rs`; adapter and
  blocked-class registries.
- **Consumes:** Core conversion tests and CLI conversion integration tests.
- **Fast proof:** `cargo test -p conary-core golden_fixtures`;
  `cargo test -p conary-core support_matrix`.
- **Medium proof:** `cargo test -p conary --test conversion_integration golden_conversion`.
- **Slow proof:** No slow gate for map-only changes.
- **Regeneration:** Hand-maintained Rust tables guarded by uniqueness,
  supported-target, and support-matrix alignment tests.
- **Safety notes:** Public-ready fixtures must use exact supported target IDs:
  `fedora-44`, `ubuntu-26.04`, or `arch`.

### legacy-scriptlet-bundle-fixtures

- **Owner:** Conary CLI tests:
  `apps/conary/tests/common/legacy_scriptlet_fixtures.rs`.
- **Purpose:** Synthetic legacy scriptlet bundles for install, remove, upgrade,
  foreign replay, and query safety behavior.
- **Fixture sources:** `apps/conary/tests/common/legacy_scriptlet_fixtures.rs`;
  local builders in `apps/conary/tests/bundle_replay.rs` and
  `apps/conary/tests/query_scripts.rs`.
- **Consumes:** `apps/conary/tests/bundle_replay.rs`;
  `apps/conary/tests/foreign_replay.rs`; `apps/conary/tests/query_scripts.rs`.
- **Fast proof:**
  `cargo test -p conary --test bundle_replay synthetic_legacy_bundle_fixtures_cover_task5_matrix`.
- **Medium proof:** `cargo test -p conary --test bundle_replay`;
  `cargo test -p conary --test foreign_replay`;
  `cargo test -p conary --test query_scripts`.
- **Slow proof:** No slow gate for map-only changes; use focused
  `conary-test` suites only when install/remove behavior changes active host
  flows.
- **Regeneration:** Hand-maintained Rust builders.
- **Safety notes:** Do not weaken review, blocked, raw replay, target
  compatibility, or private-path redaction gates. CLI replay fixtures are not
  Remi publication fixtures; see `remi-scriptlet-publication-gate` for
  server-side gates.

### remi-scriptlet-publication-gate

- **Owner:** Remi server: `apps/remi/src/server/publication.rs`.
- **Purpose:** Public-ready filtering for converted packages and chunks based
  on scriptlet metadata.
- **Fixture sources:** `apps/remi/src/server/publication.rs`;
  `apps/remi/src/server/conversion.rs`; `apps/remi/src/server/index_gen.rs`;
  `apps/remi/src/server/prewarm.rs`; handler tests under
  `apps/remi/src/server/handlers/`.
- **Consumes:** Remi publication, conversion, generated-index,
  sparse/detail/search/chunk serving, and prewarm tests.
- **Fast proof:** `cargo test -p remi publication`.
- **Medium proof:**
  `cargo test -p remi persisted_goal8a_golden_outcomes_respect_publication_gate`;
  `cargo test -p remi generated_index_includes_public_scriptlets_without_private_path`.
- **Slow proof:** No slow gate for map-only changes; run
  `cargo test -p remi` for behavior changes that affect serving.
- **Regeneration:** Hand-maintained test rows and helper builders.
- **Safety notes:** Public listing and chunk serving must not expose non-public
  scriptlet rows or private `review_artifact_path` values. Server-side
  publication fixtures are not CLI replay fixtures; see
  `legacy-scriptlet-bundle-fixtures` for local replay behavior.

### remi-test-artifact-fixtures

- **Owner:** Remi artifact handlers:
  `apps/remi/src/server/handlers/admin/artifacts.rs`.
- **Purpose:** Upload and serve static test fixture artifacts through admin and
  public routes.
- **Fixture sources:** `apps/remi/src/server/handlers/admin/artifacts.rs`;
  `apps/remi/src/server/handlers/artifacts.rs`;
  `apps/remi/src/server/artifact_paths.rs`.
- **Consumes:** Admin upload tests, public fixture GET/HEAD tests, audit action
  tests.
- **Fast proof:** `cargo test -p remi test_upload_fixture`;
  `cargo test -p remi test_public_fixture_get_and_head`.
- **Medium proof:** `cargo test -p remi artifacts`.
- **Slow proof:** No slow gate for map-only changes.
- **Regeneration:** Generated in temporary directories during tests.
- **Safety notes:** Keep path traversal rejection and fixture-size limits
  intact.

### conary-test-remi-manifests

- **Owner:** Integration harness: `apps/conary-test/src/config/` and
  `apps/conary-test/src/suite_inventory.rs`.
- **Purpose:** Declarative Remi and package-manager integration suites.
- **Fixture sources:** `apps/conary/tests/integration/remi/manifests/`;
  `apps/conary/tests/integration/remi/containers/`.
- **Consumes:** `cargo run -p conary-test -- list`, manifest parser tests,
  suite runner, local QEMU validation scripts.
- **Fast proof:** `cargo run -p conary-test -- list`;
  `cargo test -p conary-test suite_inventory`.
- **Medium proof:**
  `cargo test -p conary-test config::tests::test_load_phase1_core_manifest`;
  `cargo test -p conary-test config::tests::test_load_phase3_group_m_manifest_installs_local_fixture_ccs`.
- **Slow proof:** Suite-specific commands such as
  `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4`
  when behavior changes require live integration proof. `fedora44` is the
  existing `conary-test` runner distro key; public CCS target IDs remain
  `fedora-44`, `ubuntu-26.04`, and `arch`.
- **Regeneration:** Manifests are hand-maintained TOML. Fixture packages are
  built or published through `conary-test fixtures` commands and scripts
  documented in `docs/INTEGRATION-TESTING.md`. Suite result JSON is generated
  locally under the ignored `apps/conary/tests/integration/remi/results/`
  directory.
- **Safety notes:** Treat manifest schema and semantics as persisted test
  configuration; changes need parser/list proof and an explicit migration or
  defaulting decision.

## How To Use This Map

- For docs-only edits to this map, run docs-audit and diff hygiene.
- For CCS conversion fixture edits, start with the core fast proof and add the
  Conary conversion integration filter when conversion output changes.
- For local replay or query fixture edits, start with the focused Conary test
  that consumes the fixture family and then run the full owning integration test
  file.
- For Remi publication or serving edits, run the focused Remi filter that names
  the gate being changed, then run `cargo test -p remi` when public listing,
  chunk serving, or conversion state changes.
- For `conary-test` manifest edits, run `cargo run -p conary-test -- list`
  before any suite execution. If a manifest schema or semantic changes, run the
  parser tests named above before a live suite.
- For broader integration-test expectations, see `docs/INTEGRATION-TESTING.md`.

## Deferred Fixture Families

The following families are known but not mapped in detail in this first slice.
They are candidate future ownership rows; later slices must validate source
roots and proof commands before treating them as committed gates:

- Native package corpus fixtures under
  `apps/conary/tests/fixtures/phase4-daily-driver-corpus/` and
  `apps/conary/tests/fixtures/phase4-runtime-fixture/`.
- Native package-manager daily-driver and CLI daily UX fixture patterns under
  `apps/conary/tests/native_pm_daily_driver.rs` and
  `apps/conary/tests/cli_daily_ux.rs`.
- `conary-test` bootstrap check and smoke fixtures documented in
  `docs/INTEGRATION-TESTING.md`.
- Bootstrap and QEMU source-image fixtures.
- Generation export and ISO carrier fixtures.
- Recipe and source-selection fixtures.
- conaryd daemon job fixtures.
- Agent/MCP operation fixtures.
- TUF trust and signature verification fixtures under `apps/conary/tests/fixtures/trust/`.

Add these in later Phase 3 slices using the same schema.
````

- [ ] **Step 2: Stage the map before regenerating docs inventory**

```bash
git add docs/modules/test-fixtures.md
```

- [ ] **Step 3: Regenerate docs-audit inventory**

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected: the inventory includes:

```text
docs/modules/test-fixtures.md	canonical	contributor
```

- [ ] **Step 4: Add the fixture map ledger row**

Add this literal-tab row to
`docs/superpowers/documentation-accuracy-audit-ledger.tsv` near the other
`docs/modules/` rows:

```text
docs/modules/test-fixtures.md	docs/modules/test-fixtures.md	canonical	contributor	test-fixtures; fixture-map; remi; ccs; conary-test	docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md; crates/conary-core/src/ccs/convert/golden_fixtures.rs; crates/conary-core/src/ccs/convert/support_matrix.rs; apps/conary/tests/common/legacy_scriptlet_fixtures.rs; apps/remi/src/server/publication.rs; apps/remi/src/server/conversion.rs; apps/conary-test/src/suite_inventory.rs; apps/conary/tests/integration/remi/manifests	verified	corrected	Added the first canonical fixture ownership map for Remi and CCS conversion/publication fixtures, including owners, consumers, fast and medium proof commands, slow proof boundaries, regeneration notes, and safety constraints.
```

- [ ] **Step 5: Update the audit summary for the fixture map**

In the maintainability planning section of
`docs/superpowers/documentation-accuracy-audit-summary.md`, append this
paragraph after the Phase 3 plan paragraph:

```markdown
The first fixture-map artifact records Remi and CCS conversion/publication
fixture families as contributor-facing proof surfaces, including owner files,
consuming tests, fast and medium proof commands, slow proof boundaries, and
safety notes.
```

Then update the final counts to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 145
- `verified-no-change`: 14
- `corrected`: 44
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its evidence and
notes mention the new fixture map and current counts.

- [ ] **Step 6: Verify the map and docs-audit**

```bash
grep -n "ccs-convert-golden-cases" docs/modules/test-fixtures.md
grep -n "remi-scriptlet-publication-gate" docs/modules/test-fixtures.md
grep -n "conary-test-remi-manifests" docs/modules/test-fixtures.md
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --cached --check
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 7: Commit the fixture map**

```bash
git add docs/modules/test-fixtures.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: map remi ccs fixture ownership"
```

### Task 2: Route Assistant And Module Docs To The Fixture Map

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Update `docs/llms/subsystem-map.md` look-here-first pointers**

In the "Look Here First" section, replace the CCS bullet:

```markdown
- CCS package building, chunking, verification, and conversion:
  `crates/conary-core/src/ccs/builder.rs`,
  `crates/conary-core/src/ccs/binary_manifest.rs`,
  `crates/conary-core/src/ccs/chunking.rs`, and
  `crates/conary-core/src/ccs/convert/`
```

with:

```markdown
- CCS package building, chunking, verification, conversion, and fixture proof:
  `crates/conary-core/src/ccs/builder.rs`,
  `crates/conary-core/src/ccs/binary_manifest.rs`,
  `crates/conary-core/src/ccs/chunking.rs`,
  `crates/conary-core/src/ccs/convert/`, and
  `docs/modules/test-fixtures.md`
```

Replace the Remi bullet:

```markdown
- Remi admin and MCP flows:
  `apps/remi/src/server/admin_service.rs`,
  `apps/remi/src/server/mcp.rs`, and
  `apps/remi/src/server/handlers/admin/`
```

with:

```markdown
- Remi admin, publication, artifact fixture, and MCP flows:
  `apps/remi/src/server/admin_service.rs`,
  `apps/remi/src/server/publication.rs`,
  `apps/remi/src/server/mcp.rs`,
  `apps/remi/src/server/handlers/artifacts.rs`,
  `apps/remi/src/server/handlers/admin/`, and
  `docs/modules/test-fixtures.md`
```

Add this bullet under "Prefer Existing Deep Dives" after the CCS module link:

```markdown
- [`docs/modules/test-fixtures.md`](../modules/test-fixtures.md) for Remi and CCS fixture ownership and proof commands
```

- [ ] **Step 2: Update `docs/modules/ccs.md` fixture reference**

Add this section after `## Architecture Context` and before
`## Legacy Scriptlet Bundles And Replay`:

````markdown
## Fixture Ownership

The first fixture ownership map for CCS conversion lives in
`docs/modules/test-fixtures.md`. Start there before changing golden conversion
cases, support-matrix fixture names, adapter-backed public-ready evidence, or
legacy scriptlet bundle fixtures. The fast proof for map-only or table-only
changes is:

```bash
cargo test -p conary-core golden_fixtures
cargo test -p conary-core support_matrix
```

If conversion output changes, also run:

```bash
cargo test -p conary --test conversion_integration golden_conversion
```
````

- [ ] **Step 3: Update `docs/modules/remi.md` fixture reference**

Add this subsection after `### Legacy Scriptlet Publication Gate` and before
`## Conversion Benchmark Evidence`:

````markdown
### Fixture Ownership

The first Remi fixture ownership map lives in `docs/modules/test-fixtures.md`.
Start there before changing scriptlet publication gates, converted package
public-ready filtering, public index metadata, review artifacts, static test
fixture uploads, or `conary-test` manifest behavior.

Fast proof for publication-gate edits:

```bash
cargo test -p remi publication
```

Medium proof when public serving, conversion state, or generated metadata
changes:

```bash
cargo test -p remi
```
````

- [ ] **Step 4: Update docs-audit rows for routed docs**

Refresh the existing ledger rows for:

- `docs/llms/subsystem-map.md`
- `docs/modules/ccs.md`
- `docs/modules/remi.md`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

The `docs/modules/ccs.md` row changes from `verified-no-change` to `corrected`
because this task edits it. Then update the final counts to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 145
- `verified-no-change`: 13
- `corrected`: 45
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

- [ ] **Step 5: Verify routing docs and docs-audit**

```bash
grep -n "test-fixtures.md" docs/llms/subsystem-map.md docs/modules/ccs.md docs/modules/remi.md
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit routing docs**

```bash
git add docs/llms/subsystem-map.md docs/modules/ccs.md docs/modules/remi.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: route fixture proof guidance"
```

### Task 3: Verify Focused Fixture Commands

**Files:**
- Verify docs and test commands from Tasks 1 and 2.

- [ ] **Step 1: Run CCS fixture proof commands**

```bash
cargo test -p conary-core golden_fixtures
cargo test -p conary-core support_matrix
cargo test -p conary --test conversion_integration golden_conversion
```

Expected: all commands exit 0.

- [ ] **Step 2: Run Remi fixture proof commands**

```bash
cargo test -p remi publication
cargo test -p remi persisted_goal8a_golden_outcomes_respect_publication_gate
```

Expected: both commands exit 0.

- [ ] **Step 3: Run conary-test manifest proof commands**

```bash
cargo run -p conary-test -- list
cargo test -p conary-test suite_inventory
```

Expected: both commands exit 0.

- [ ] **Step 4: Run docs-audit and formatting gates**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
cargo fmt --check
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 5: Run a stale-surface sweep for the new packet and docs**

```bash
found=0
for term in T''BD TO''DO FI''XME Cent''OS RH''EL "Debian sta""ble" open''SUSE Al''pine CLAU''DE Cla''ude "Open Review"" Questions"; do
    if grep -n "$term" docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md docs/modules/test-fixtures.md docs/llms/subsystem-map.md docs/modules/ccs.md docs/modules/remi.md; then
        found=1
    fi
done
test "$found" -eq 0
```

Expected: the loop exits 0 with no output.

- [ ] **Step 6: Commit any verification-only doc fixes**

If Task 3 found stale wording or command drift in the docs, commit only those
doc fixes:

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md docs/modules/test-fixtures.md docs/llms/subsystem-map.md docs/modules/ccs.md docs/modules/remi.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs: verify fixture discipline map"
```

If Task 3 finds no doc fixes, do not create an empty commit.

### Task 4: Final Verification And Push

**Files:**
- Verify all files changed by Tasks 0 through 3.

- [ ] **Step 1: Run final verification**

```bash
cargo test -p conary-core golden_fixtures
cargo test -p conary-core support_matrix
cargo test -p conary --test conversion_integration golden_conversion
cargo test -p remi publication
cargo test -p remi persisted_goal8a_golden_outcomes_respect_publication_gate
cargo run -p conary-test -- list
cargo test -p conary-test suite_inventory
cargo fmt --check
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 2: Verify final repo state before push**

```bash
git status --short --branch
git log --oneline origin/main..HEAD
git rev-list --left-right --count HEAD...origin/main
```

Expected: clean worktree, local commits ahead, and behind count 0.

- [ ] **Step 3: Push**

```bash
git push
```

- [ ] **Step 4: Verify clean remote parity**

```bash
git status --short --branch
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
git worktree list --porcelain
```

Expected: `HEAD` equals `origin/main`, ahead/behind is `0 0`, and no unexpected
worktree changes remain.

## Review Checklist

- Does the fixture map schema include enough information for a future `/goal`
  to choose the right test gate?
- Does the first slice avoid moving fixtures before ownership is clear?
- Are Remi and CCS fixture families named with real repo paths and tests?
- Does the plan preserve supported public target scope: Fedora 44, Ubuntu 26.04,
  and Arch?
- Does it avoid making slow integration/QEMU proof mandatory for docs-only
  fixture-map edits?
- Does it avoid duplicating volatile test counts in assistant-facing docs?
- Are docs-audit staging steps ordered so `git ls-files` can see new docs?
