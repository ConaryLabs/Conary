---
last_updated: 2026-06-06
revision: 11
summary: GPT-5.5/Codex-first map with feature ownership, bootstrap smoke, and drift-control routing
---

# Conary For Coding Assistants

## Purpose

This directory is the GPT-5.5/Codex-first map for coding assistants working on
Conary. Use it to find the right canonical docs quickly without turning the
root guidance into a manual.

## Guidance Order

1. Start with `AGENTS.md` for the repo contract, verification commands, and
   safety rules.
2. Use this file to find the right deeper docs.
3. Follow the linked canonical docs for subsystem, testing, and operations
   detail.

## Instruction Layers

- `AGENTS.md` is the shared repo-wide contract for coding agents.
- This file is the vendor-neutral map into the canonical docs under `docs/`.
- Canonical subsystem and operations detail belongs in human-readable docs such
  as `docs/ARCHITECTURE.md`, `docs/modules/*.md`, and
  `docs/operations/*.md`.
- Tool-specific entrypoints such as `GEMINI.md` and
  `.github/copilot-instructions.md` should stay intentionally thin and point
  back to this layered doc system instead of becoming parallel manuals.
- Claude-specific tracked entrypoints and `.claude/` harness files are retired.
  Keep Claude-era context in `docs/llms/archive/`, not in active guidance.
- If a subtree later needs materially different durable instructions, prefer a
  nested `AGENTS.md` scoped to that subtree over bloating the root guidance.

## OpenAI GPT-5.5 Notes

This repo should keep durable guidance model-neutral, but OpenAI/Codex prompt
and harness changes should be checked against current OpenAI docs:

- [Codex AGENTS.md guidance](https://developers.openai.com/codex/guides/agents-md)
- [Codex best practices](https://developers.openai.com/codex/learn/best-practices)
- [Prompt guidance](https://developers.openai.com/api/docs/guides/prompt-guidance)
- [Prompt engineering](https://developers.openai.com/api/docs/guides/prompt-engineering)
- [Reasoning best practices](https://developers.openai.com/api/docs/guides/reasoning-best-practices)
- [Using GPT-5.5](https://developers.openai.com/api/docs/guides/latest-model#using-reasoning-models)

For Codex or other OpenAI agents, keep stable repo policy near the top of the
prompt by pointing to `AGENTS.md` and linked canonical docs. Put dynamic context
such as branch state, failing commands, run IDs, and one-off user notes near the
end so repeated prompts stay cache-friendly and less prone to stale copied
lore.

State the desired mode plainly: plan, design, implement, review, debug, or
verify. Include acceptance criteria and exact verification commands when they
are known, while leaving room for the agent to inspect the codebase and adjust
the path. Prefer outcome-focused constraints over long, brittle scripts.
For long-running Codex work, ask for an explicit plan/TODO list, short notable
tool preambles, persistence until the request is fully handled, and final
evidence before success claims.

Keep GPT-5.5 prompts simple and direct. Use Markdown headings, XML tags, or
other clear delimiters when mixing logs, diffs, requirements, and expected
output. Do not ask for hidden chain-of-thought; ask for findings, decisions,
verification evidence, and concise rationale.

Treat output length and reasoning depth as separate concerns. Use harness
controls such as `text.verbosity` and `reasoning.effort` when available; in repo
prompts, ask for concrete budgets, section counts, or machine-readable output
only when the workflow needs them.

For tool-heavy sessions, short tool preambles are useful. If a future harness
manages Responses API state directly, preserve returned assistant output item
metadata such as `phase`, use `previous_response_id` where appropriate, and make
compaction summaries preserve completed actions, active assumptions, IDs, tool
outcomes, unresolved blockers, and the next concrete goal.

Keep tool-specific behavior in tool descriptions, MCP schemas, or harness
configuration when possible. `AGENTS.md` and this directory should carry
cross-tool policy, source-of-truth pointers, and durable repo workflow
expectations. Use structured outputs or schema validation in a harness instead
of prose-only JSON schema instructions.

There is no active OpenAI/LLM prompt harness in this repository today.
`crates/conary-core/src/automation/prompt.rs` is product automation UI, not a
model prompt layer. If a future agentic harness is added, prefer the Responses
API plus current Agents SDK patterns over custom orchestration, and document the
runtime contract outside the repo-wide assistant map.

Do not bake a "current date" into durable assistant docs. Add explicit dates or
time zones only when a workflow needs user-local, release, policy-effective, or
other non-UTC context.

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

## Working Rules

- Treat `AGENTS.md` as a map, not a manual.
- Prefer `AGENTS.md` as the shared cross-tool filename where the tool supports
  it.
- Keep tool-specific files such as `GEMINI.md` or
  `.github/copilot-instructions.md` short and pointed back at `AGENTS.md`.
- Do not reintroduce tracked `CLAUDE.md`, `.claude/`, or Claude hook helpers
  unless the active toolchain changes.
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
- When version-specific library behavior matters, check current external
  documentation instead of guessing APIs.
- For maintainability, pruning, or refactor work, require the task packet to
  name the owning subsystem, the current large-file or stale-surface pressure,
  the intended new boundary, persisted-state impact, focused verification, and
  docs or subsystem-map updates.
- For feature-scoped work, use `docs/modules/feature-ownership.md` to find the
  start-here files, neighboring systems, focused proof, and broader interaction
  gate before editing.
- Use `scripts/line-count-report.sh` when a planning or review pass needs a
  fresh Rust hotspot snapshot. Treat the report as a prioritization aid, not a
  CI failure condition.
- Use `scripts/maintainability-drift-report.sh` before broad feature,
  refactor, or docs-routing changes to get warn-only changed-path owner hints,
  docs-audit status, and current hotspot context.

## Freshness Rules

- Prefer linked canonical docs over copied volatile facts.
- Use frontmatter (`last_updated`, `revision`, `summary`) for canonical docs
  that are meant to stay discoverable and maintained over time.
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
