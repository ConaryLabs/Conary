---
last_updated: 2026-07-25
revision: 17
summary: Vendor-neutral assistant map with canonical design ownership, issue-backed execution, feature ownership, and documentation-truth routing
---

# Conary For Coding Assistants

## Purpose

This directory is the vendor-neutral landing map for coding assistants working
on Conary. It should get a fresh agent from "I know nothing about this repo" to
"I know the owner files and proof commands for this task" without becoming a
second subsystem manual.

## First Five Minutes

1. Read [`AGENTS.md`](../../AGENTS.md) for the repo contract, safety rules, and
   verification expectations.
2. Read [`CONTRIBUTING.md`](../../CONTRIBUTING.md#development-workflow) for the
   issue, branch, pull-request, review, and closeout lifecycle.
3. Use this file to decide which canonical docs to open next.
4. If you need a capability name, run
   `bash scripts/agent-context.sh --list` to discover available feature slugs.
5. Route the actual work with:
   - `bash scripts/agent-context.sh --feature <slug>` for a feature slug.
   - `bash scripts/agent-context.sh --path <file>` for a specific path.
6. Read the start-here files, focused proof, and interaction gates printed by
   the `--feature` or `--path` command before editing.
7. Use the focused proof for narrow edits and the interaction gate when the
   change crosses a neighbor system.

## Choose By Task

- **Feature or source edit:** run `bash scripts/agent-context.sh --path <file>`
  for each touched path, or `--feature <slug>` when starting from a capability.
- **Docs-only assistant guidance edit:** start with `AGENTS.md`, this file,
  `docs/llms/subsystem-map.md`, and the affected tool shim; use the docs proof
  floor below.
- **Architecture or subsystem question:** use `docs/ARCHITECTURE.md` and the
  relevant `docs/modules/*.md` file after owner-card routing.
- **Integration-test or fixture work:** read `docs/INTEGRATION-TESTING.md` and
  `docs/modules/test-fixtures.md`.
- **Deploy, MCP, or host workflow:** read `docs/operations/infrastructure.md`.
- **Version-specific tool, SDK, MCP, or model behavior:** check current vendor
  documentation before editing durable guidance.

## Guidance Order

1. `AGENTS.md` is the canonical repo-wide contract.
2. `CONTRIBUTING.md` owns the issue-backed branch and pull-request lifecycle.
3. This file is the vendor-neutral routing layer into canonical docs.
4. `scripts/agent-context.sh` and `docs/modules/feature-ownership.md` own exact
   feature slugs, path routing, start-here files, focused proof, and interaction
   gates.
5. Linked canonical docs own subsystem, testing, and operations detail.

## Instruction Layers

- `AGENTS.md` is the shared repo-wide contract for coding agents.
- `CONTRIBUTING.md` is the shared issue, branch, pull-request, review, and
  closeout workflow for humans and agents.
- `docs/llms/README.md` is the vendor-neutral landing map into canonical docs.
- `docs/modules/feature-ownership.md` is the detailed owner-card source, reached
  through `scripts/agent-context.sh` whenever possible.
- `docs/llms/subsystem-map.md` is a compact workspace orientation index, not a
  second ownership database.
- Canonical subsystem and operations detail belongs in human-readable docs such
  as `docs/ARCHITECTURE.md`, `docs/modules/*.md`, and
  `docs/operations/*.md`.
- Tool-specific entrypoints such as `CLAUDE.md`, `GEMINI.md`, `REASONIX.md`, and
  `.github/copilot-instructions.md` stay intentionally thin and point back to
  this layered doc system instead of becoming parallel manuals.
- Detailed roadmap state lives in `docs/roadmaps/`. Durable design decisions
  live in the architecture, module, or `docs/specs/` document that owns the
  affected surface. Bounded execution state lives in the primary issue and
  draft pull request. Stable public or persisted contracts remain in
  `docs/specs/`.

## Core Docs

- [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md): workspace-level architecture and data flow
- [`docs/INTEGRATION-TESTING.md`](../INTEGRATION-TESTING.md): `conary-test` suites, phases, and runtime expectations
- [`docs/modules/bootstrap.md`](../modules/bootstrap.md): bootstrap pipeline background
- [`docs/modules/ccs.md`](../modules/ccs.md): CCS format and tooling notes
- [`docs/modules/feature-ownership.md`](../modules/feature-ownership.md): feature ownership cards and interaction verification gates
- [`docs/modules/federation.md`](../modules/federation.md): Remi federation model and trust constraints
- [`docs/modules/query.md`](../modules/query.md): CLI query surface and related data paths
- [`docs/modules/recipe.md`](../modules/recipe.md): recipe/build-system background
- [`docs/modules/source-selection.md`](../modules/source-selection.md): source-policy inputs, runtime mirrors, and replatform/update behavior
- [`docs/operations/infrastructure.md`](../operations/infrastructure.md): structured operations transport, deploy, and host notes
- [`docs/roadmaps/development-roadmap.md`](../roadmaps/development-roadmap.md): current maturity, ordered workstreams, blockers, proof, and longer horizons
- [`docs/llms/subsystem-map.md`](subsystem-map.md): stable "look here first" pointers distilled from retired assistant docs

## Focused Docs

- [`docs/operations/bootstrap-selfhosting-vm.md`](../operations/bootstrap-selfhosting-vm.md): truthful operator flow for the current bootstrap self-hosting VM path
- [`docs/operations/daily-driver-ux-matrix.md`](../operations/daily-driver-ux-matrix.md): daily-driver CLI diagnostics, unsupported-case routes, shell completion checks, and focused Goal 7 tests
- [`docs/llms/openai-codex.md`](openai-codex.md): OpenAI/Codex-specific prompt and harness notes kept out of the vendor-neutral map

## Doc Families

- **Contract:** `AGENTS.md`, `CONTRIBUTING.md`, and thin tool shims. These are
  durable instructions loaded by contributors, agents, or coding tools.
- **Routing:** this file, `docs/llms/subsystem-map.md`,
  `docs/modules/feature-ownership.md`, and `scripts/agent-context.sh`.
- **Canonical docs:** architecture, module, integration-testing, operations,
  guide, and `docs/specs/` contract docs that describe current repo behavior or
  intended active product contracts.
- **Roadmap and execution state:** `docs/roadmaps/` owns detailed current
  ordering and milestone status. The primary issue and draft pull request own
  bounded execution state. Durable decisions move directly into the canonical
  architecture, module, or specification document they affect. None of these
  surfaces should be mistaken for already-shipped behavior.
- **Planning history:** after durable truth and resume facts move to canonical
  owners, completed, superseded, or abandoned planning is deleted from the
  current tree and recovered through Git history. Do not create a replacement
  archive.

## Working Rules

- Treat `AGENTS.md` as a contract and this file as a map.
- Use one primary GitHub issue for each non-trivial work slice, an issue-linked
  branch, and a pull request for integration; follow `CONTRIBUTING.md` for
  linkage and closeout semantics.
- Prefer `AGENTS.md` as the shared cross-tool filename where the tool supports
  it.
- Use `scripts/agent-context.sh --feature <slug>` before feature-scoped edits,
  or `--path <file>` to route a path, instead of manually skimming long route
  lists. It prints the owning card's start-here files, safety invariants,
  focused proof, and interaction gate before editing.
- Do not duplicate detailed owner-card routing outside
  `docs/modules/feature-ownership.md`.
- Keep tool-specific files such as `CLAUDE.md`, `GEMINI.md`, `REASONIX.md`, or
  `.github/copilot-instructions.md` short and pointed back at the shared
  contract.
- Do not reintroduce tracked `.claude/` harness files or Claude hook helpers
  unless the active toolchain needs shared versioned Claude configuration.
- Avoid duplicating or conflicting repo-wide guidance across tool-specific
  entrypoints or path rules.
- Prefer structured Conary operation surfaces over ad hoc SSH or curl when the
  available MCP/HTTP/CLI surface already covers the workflow.
- Treat `crates/conary-agent-contract` as the LLM-facing operation vocabulary;
  MCP code should adapt that contract, not become the product contract itself.
- For local developer environment validation, start with
  `cargo run -p conary-test -- bootstrap check --json`, then preview
  `cargo run -p conary-test -- bootstrap smoke --dry-run --json` before running
  `cargo run -p conary-test -- bootstrap smoke --json`.
- Treat `bootstrap smoke` as a local test-runner proof loop. It may build
  images, start containers, and write result files, but it is not fixture
  publishing and does not add live MCP resources or live MCP prompts.
- For maintainability, pruning, or refactor work, require the task packet to
  name the owning subsystem, the current large-file or stale-surface pressure,
  the intended new boundary, persisted-state impact, focused verification, and
  docs or subsystem-map updates.
- Use `scripts/line-count-report.sh` when a planning or review pass needs a
  fresh Rust hotspot snapshot. Treat the report as a prioritization aid, not a
  CI failure condition.
- Use `scripts/maintainability-drift-report.sh` before broad feature,
  refactor, or docs-routing changes to get warn-only changed-path owner hints,
  documentation-truth status, and current hotspot context.

## Agent-Doc Proof Floor

For docs-only edits to assistant guidance, run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/test-doc-truth.sh
bash scripts/test-agent-context.sh
bash scripts/agent-context.sh --validate
git diff --check
```

Add a focused stale-reference sweep for any retired path, tool shim, or routing
phrase touched by the edit.

## Freshness Rules

- Prefer linked canonical docs over copied volatile facts.
- Use frontmatter (`last_updated`, `revision`, `summary`) for canonical docs
  that are meant to stay discoverable and maintained over time.
- Do not duplicate schema counts, workflow counts, or host-specific trivia here.
- If a detail cannot be kept fresh realistically, omit it instead of preserving
  stale lore.
- When version-specific library, SDK, MCP, model, or coding-agent behavior
  matters, check current external documentation before editing durable guidance.
- For broad documentation work, run the doc-truth checker and its fixtures,
  then validate feature-card routing and local-link/diff hygiene.
- For implementation-to-claim work, use the owning feature card to select the
  focused behavior proof and interaction gate in addition to doc truth.
