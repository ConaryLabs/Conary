---
last_updated: 2026-05-12
revision: 6
summary: Vendor-neutral map for coding assistants working in the Conary repository, current validation docs, and tool-specific entrypoints
---

# Conary For Coding Assistants

## Purpose

This directory is the vendor-neutral map for coding assistants working on
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
- Tool-specific entrypoints such as `CLAUDE.md`, `GEMINI.md`, and
  `.github/copilot-instructions.md` should stay intentionally thin and point
  back to this layered doc system instead of becoming parallel manuals.
- If a subtree later needs materially different durable instructions, prefer a
  nested `AGENTS.md` scoped to that subtree over bloating the root guidance.

## OpenAI GPT-5.5 Notes

This repo should keep durable guidance model-neutral, but OpenAI/Codex prompt
and harness changes should be checked against current OpenAI docs:

- [Prompt guidance](https://developers.openai.com/api/docs/guides/prompt-guidance)
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

Do not bake a "current date" into durable assistant docs. Add explicit dates or
time zones only when a workflow needs user-local, release, policy-effective, or
other non-UTC context.

## Core Docs

- [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md): workspace-level architecture and data flow
- [`docs/INTEGRATION-TESTING.md`](../INTEGRATION-TESTING.md): `conary-test` suites, phases, and runtime expectations
- [`docs/modules/bootstrap.md`](../modules/bootstrap.md): bootstrap pipeline background
- [`docs/modules/ccs.md`](../modules/ccs.md): CCS format and tooling notes
- [`docs/modules/federation.md`](../modules/federation.md): Remi federation model and trust constraints
- [`docs/modules/query.md`](../modules/query.md): CLI query surface and related data paths
- [`docs/modules/recipe.md`](../modules/recipe.md): recipe/build-system background
- [`docs/modules/source-selection.md`](../modules/source-selection.md): source-policy inputs, runtime mirrors, and replatform/update behavior
- [`docs/operations/infrastructure.md`](../operations/infrastructure.md): MCP-first operations, deploy, and host notes
- [`docs/llms/subsystem-map.md`](subsystem-map.md): stable "look here first" pointers distilled from legacy assistant docs

## Focused Docs

- [`docs/operations/bootstrap-selfhosting-vm.md`](../operations/bootstrap-selfhosting-vm.md): truthful operator flow for the current bootstrap self-hosting VM path
- [`docs/operations/post-generation-export-follow-up-roadmap.md`](../operations/post-generation-export-follow-up-roadmap.md): remaining image projection, ISO, bundle, and boot-artifact provenance work
- [`docs/operations/bootstrap-follow-up-investigations.md`](../operations/bootstrap-follow-up-investigations.md): deferred architecture and cleanup ideas to revisit after bootstrap is stable

## Working Rules

- Treat `AGENTS.md` as a map, not a manual.
- Prefer `AGENTS.md` as the shared cross-tool filename where the tool supports
  it.
- Keep tool-specific files such as `CLAUDE.md`, `GEMINI.md`, or
  `.github/copilot-instructions.md` short and pointed back at `AGENTS.md`.
- Avoid duplicating or conflicting repo-wide guidance across tool-specific
  entrypoints or path rules.
- Prefer MCP tools over ad hoc SSH or curl when the MCP surface already covers
  the workflow.
- When version-specific library behavior matters, check current external
  documentation instead of guessing APIs.

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
