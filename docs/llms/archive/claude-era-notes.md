---
last_updated: 2026-05-22
revision: 2
summary: Historical notes about the retired Claude-era assistant harness, its migration, and the later removal of tracked Claude active artifacts
---

# Claude-Era Harness Notes

## Why The Old `.claude/` Tree Existed

The previous harness leaned on Claude-specific loading behavior:

- `rules/` files let Claude load subsystem guidance conditionally by path
- named agents acted as workflow shorthand for review, debugging, QA, and release checks
- local settings and memory files accumulated real operator knowledge over time

That made Claude productive, but it also trapped durable Conary knowledge in a
tool-specific structure that other assistants could not reuse directly.

## What Was Worth Preserving

- Subsystem pointers, invariants, and "look here first" guidance
- MCP-first operational habits
- The habit of checking current external docs before guessing third-party APIs

## What Did Not Survive As Active Guidance

- Named-agent rosters as the primary collaboration model
- Claude-specific path-loading metadata such as `paths:` frontmatter
- Claude hook guardrails such as sensitive-file blocking and post-edit clippy
  feedback
- Local settings files as an implicit place to stash operator access details
- Volatile inventories such as schema counts, workflow counts, and other
  fast-moving implementation trivia

## Migration Outcome

The durable pieces moved into:

- [`AGENTS.md`](../../../AGENTS.md) for the repo contract
- [`docs/llms/README.md`](../README.md) and [`docs/llms/subsystem-map.md`](../subsystem-map.md) for vendor-neutral assistant guidance
- [`docs/operations/infrastructure.md`](../../operations/infrastructure.md) for non-secret operations notes
- [`docs/operations/LOCAL_ACCESS.example.md`](../../operations/LOCAL_ACCESS.example.md) as the tracked template for the ignored `docs/operations/LOCAL_ACCESS.md` local note

The tracked Claude-specific active artifacts were later removed because this
workspace no longer uses Claude. That includes the `CLAUDE.md` compatibility
shim, `.claude/settings.json`, and the Claude hook helper scripts under
`scripts/dev/`.

If old local `.claude/` files still exist in a checkout, they are ignored local
leftovers and are not repository guidance. Do not restore tracked Claude
entrypoints or harness files unless the active toolchain changes.
