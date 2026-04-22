---
last_updated: 2026-04-22
revision: 3
summary: Vendor-neutral map for coding assistants working in the Conary repository and the compatibility layer around the canonical doc system
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
- Compatibility shims such as `CLAUDE.md`, `GEMINI.md`, and
  `.github/copilot-instructions.md` should stay intentionally thin and point
  back to this layered doc system instead of becoming parallel manuals.
- If a subtree later needs materially different durable instructions, prefer a
  nested `AGENTS.md` scoped to that subtree over bloating the root guidance.

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
- [`docs/operations/bootstrap-follow-up-investigations.md`](../operations/bootstrap-follow-up-investigations.md): deferred architecture and cleanup ideas to revisit after bootstrap is stable

## Working Rules

- Treat `AGENTS.md` as a map, not a manual.
- Prefer `AGENTS.md` as the shared cross-tool filename where the tool supports
  it.
- Keep tool-specific files such as `CLAUDE.md`, `GEMINI.md`, or
  `.github/copilot-instructions.md` short and pointed back at `AGENTS.md`.
- Avoid duplicating or conflicting repo-wide guidance across compatibility
  shims or tool-specific path rules.
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
