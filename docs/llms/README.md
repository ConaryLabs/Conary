---
last_updated: 2026-04-02
revision: 1
summary: Vendor-neutral map for coding assistants working in the Conary repository
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

## Core Docs

- `docs/ARCHITECTURE.md`: workspace-level architecture and data flow
- `docs/INTEGRATION-TESTING.md`: `conary-test` suites, phases, and runtime expectations
- `docs/modules/bootstrap.md`: bootstrap pipeline background
- `docs/modules/ccs.md`: CCS format and tooling notes
- `docs/modules/federation.md`: Remi federation model and trust constraints
- `docs/modules/query.md`: CLI query surface and related data paths
- `docs/modules/recipe.md`: recipe/build-system background
- `docs/operations/infrastructure.md`: MCP-first operations, deploy, and host notes
- `docs/llms/subsystem-map.md`: stable "look here first" pointers distilled from legacy assistant docs

## Working Rules

- Treat `AGENTS.md` as a map, not a manual.
- Keep tool-specific files such as `CLAUDE.md` or future shims like `GEMINI.md`
  short and pointed back at `AGENTS.md`.
- Prefer MCP tools over ad hoc SSH or curl when the MCP surface already covers
  the workflow.
- When version-specific library behavior matters, check current external
  documentation instead of guessing APIs.

## Freshness Rules

- Prefer linked canonical docs over copied volatile facts.
- Do not duplicate schema counts, workflow counts, or host-specific trivia here.
- If a detail cannot be kept fresh realistically, omit it instead of preserving
  stale lore.
