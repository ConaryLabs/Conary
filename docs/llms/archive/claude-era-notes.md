---
last_updated: 2026-07-02
revision: 3
summary: Historical notes about the retired Claude-era assistant harness and the active thin Claude shim
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

`CLAUDE.md` is active again as a thin compatibility shim that points back to
`AGENTS.md` and the vendor-neutral assistant map. The older `.claude/` harness,
`.claude/settings.json`, named-agent roster, and Claude hook helper scripts
under `scripts/dev/` remain retired.

If old local `.claude/` files still exist in a checkout, they are ignored local
leftovers and are not repository guidance. Do not restore tracked Claude harness
files unless the active toolchain needs shared versioned Claude configuration.
