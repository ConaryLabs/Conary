@AGENTS.md

# CLAUDE.md

Claude Code reads `CLAUDE.md`, while Conary's shared assistant contract lives in
`AGENTS.md`. The import above keeps Claude aligned with the repo-wide contract
without duplicating it here.

After the imported contract, use:

1. `docs/llms/README.md`
2. `bash scripts/agent-context.sh --feature <slug>` or
   `bash scripts/agent-context.sh --path <file>`
3. The linked canonical docs for architecture, testing, modules, and operations

This file is intentionally thin. Do not turn it into a second source of truth.
If a rule, command, or workflow matters for the repository as a whole, update
`AGENTS.md` or a linked canonical doc instead.

Keep old `.claude/` harness files out of the tracked repo unless the project
adopts a shared Claude-specific harness that needs durable versioned
configuration.
