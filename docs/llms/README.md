---
last_updated: 2026-07-08
revision: 13
summary: Vendor-neutral fresh-agent landing map with feature ownership and routing ergonomics
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
2. Use this file to decide which canonical docs to open next.
3. Run one command to route the work:
   - `bash scripts/agent-context.sh --list`
   - `bash scripts/agent-context.sh --feature <slug>`
   - `bash scripts/agent-context.sh --path <file>`
4. Read the start-here files printed by that command before editing.
5. Use the focused proof for narrow edits and the interaction gate when the
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
2. This file is the vendor-neutral routing layer into canonical docs.
3. `scripts/agent-context.sh` and `docs/modules/feature-ownership.md` own exact
   feature slugs, path routing, start-here files, focused proof, and interaction
   gates.
4. Linked canonical docs own subsystem, testing, and operations detail.

## Instruction Layers

- `AGENTS.md` is the shared repo-wide contract for coding agents.
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
- Historical tool notes belong under `docs/llms/archive/`.

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
- [`docs/llms/subsystem-map.md`](subsystem-map.md): stable "look here first" pointers distilled from legacy assistant docs

## Focused Docs

- [`docs/operations/bootstrap-selfhosting-vm.md`](../operations/bootstrap-selfhosting-vm.md): truthful operator flow for the current bootstrap self-hosting VM path
- [`docs/operations/daily-driver-ux-matrix.md`](../operations/daily-driver-ux-matrix.md): daily-driver CLI diagnostics, unsupported-case routes, shell completion checks, and focused Goal 7 tests
- [`docs/operations/post-generation-export-follow-up-roadmap.md`](../operations/post-generation-export-follow-up-roadmap.md): remaining bundle, boot-artifact verification, pristine-validation, sandbox, and image-projection work after x86_64 ISO export landed
- [`docs/operations/bootstrap-follow-up-investigations.md`](../operations/bootstrap-follow-up-investigations.md): deferred architecture and cleanup ideas to revisit after bootstrap is stable
- [`docs/llms/openai-codex.md`](openai-codex.md): OpenAI/Codex-specific prompt and harness notes kept out of the vendor-neutral map

## Doc Families

- **Contract:** `AGENTS.md` and thin tool shims. These are durable instructions
  loaded by agents or coding tools.
- **Routing:** this file, `docs/llms/subsystem-map.md`,
  `docs/modules/feature-ownership.md`, and `scripts/agent-context.sh`.
- **Canonical docs:** architecture, module, integration-testing, operations,
  guide, and `docs/specs/` contract docs that describe current repo behavior or
  intended active product contracts.
- **Planning docs:** active files under `docs/superpowers/plans/` and
  `docs/superpowers/specs/`. They define scoped future work and must not be
  mistaken for already-shipped behavior.
- **Historical docs:** archive directories and rows marked historical in the
  documentation accuracy audit inventory. Use them for provenance, not current
  instructions.

## Working Rules

- Treat `AGENTS.md` as a contract and this file as a map.
- Prefer `AGENTS.md` as the shared cross-tool filename where the tool supports
  it.
- Use `scripts/agent-context.sh` before feature-scoped edits instead of
  manually skimming long route lists.
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
- When version-specific library, SDK, MCP, model, or coding-agent behavior
  matters, check current external documentation before editing durable
  guidance.
- For maintainability, pruning, or refactor work, require the task packet to
  name the owning subsystem, the current large-file or stale-surface pressure,
  the intended new boundary, persisted-state impact, focused verification, and
  docs or subsystem-map updates.
- Use `scripts/line-count-report.sh` when a planning or review pass needs a
  fresh Rust hotspot snapshot. Treat the report as a prioritization aid, not a
  CI failure condition.
- Use `scripts/maintainability-drift-report.sh` before broad feature,
  refactor, or docs-routing changes to get warn-only changed-path owner hints,
  docs-audit status, and current hotspot context.

## Agent-Doc Proof Floor

For docs-only edits to assistant guidance, run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Add a focused stale-reference sweep for any retired path, tool shim, or routing
phrase touched by the edit.

## Freshness Rules

- Prefer linked canonical docs over copied volatile facts.
- Use frontmatter (`last_updated`, `revision`, `summary`) for canonical docs
  that are meant to stay discoverable and maintained over time.
- Treat `AGENTS.md` as a contract and this file as a map.
- Use `scripts/agent-context.sh` before feature-scoped edits instead of
  manually skimming long route lists.
- Do not duplicate detailed owner-card routing outside
  `docs/modules/feature-ownership.md`.
- Do not duplicate schema counts, workflow counts, or host-specific trivia here.
- If a detail cannot be kept fresh realistically, omit it instead of preserving
  stale lore.
- For broad documentation work, use `scripts/docs-audit-inventory.sh` and
  `scripts/check-doc-audit-ledger.sh` so the tracked doc set, audit ledger, and
  current repo shape stay aligned.
- For implementation-to-claim work, also check
  `docs/superpowers/feature-coherency-ledger.tsv` with
  `scripts/check-coherency-ledger.sh` and
  `scripts/check-coherency-wave-scopes.sh`; grep the ledger for
  `doc:<path>` or `path:<path>` before editing a doc or source file that may be
  pinned by a coherency row.
- When version-specific library, SDK, MCP, model, or coding-agent behavior
  matters, check current external documentation before editing durable
  guidance.
