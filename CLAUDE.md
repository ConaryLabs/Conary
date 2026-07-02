# CLAUDE.md

Conary's canonical assistant guidance lives in `AGENTS.md`.

Start with:

1. `AGENTS.md`
2. `docs/llms/README.md`
3. The linked canonical docs for architecture, testing, and operations

Use `docs/modules/feature-ownership.md` through `scripts/agent-context.sh`
when choosing a feature area or checking cross-system gates:

```
bash scripts/agent-context.sh --feature <slug>
```

`--list` shows available slugs; `--run focused` and `--run gate` execute a
card's own proof commands.

This file is intentionally thin. It exists only as a compatibility shim for
Claude setups that default to `CLAUDE.md`.

Do not treat this file as a second source of truth. If a rule, command, or
workflow matters for the repository as a whole, it belongs in `AGENTS.md` or a
linked canonical doc instead.

Keep `.claude/` harness files out of the tracked repo unless the project adopts
a shared Claude-specific harness that needs durable versioned configuration.
