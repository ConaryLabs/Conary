---
last_updated: 2026-04-02
revision: 1
summary: Historical notes about the retired Claude-era assistant harness and why its durable pieces were migrated
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
- Two useful hook guardrails: sensitive-file blocking and post-edit clippy feedback

## What Did Not Survive As Active Guidance

- Named-agent rosters as the primary collaboration model
- Claude-specific path-loading metadata such as `paths:` frontmatter
- Local settings files as an implicit place to stash operator access details
- Volatile inventories such as schema counts, workflow counts, and other
  fast-moving implementation trivia

## Migration Outcome

The durable pieces moved into:

- [`AGENTS.md`](../../../AGENTS.md) for the repo contract
- [`docs/llms/README.md`](../README.md) and [`docs/llms/subsystem-map.md`](../subsystem-map.md) for vendor-neutral assistant guidance
- [`docs/operations/infrastructure.md`](../../operations/infrastructure.md) for non-secret operations notes
- [`docs/operations/LOCAL_ACCESS.example.md`](../../operations/LOCAL_ACCESS.example.md) as the tracked template for the ignored `docs/operations/LOCAL_ACCESS.md` local note

The remaining Claude-specific artifacts are compatibility shims or explicit
tool-coupled helpers rather than the repository's source of truth.
