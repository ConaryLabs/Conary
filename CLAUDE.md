# CLAUDE.md

Conary's canonical assistant guidance lives in `AGENTS.md`.

Start with:

1. `AGENTS.md`
2. `docs/llms/README.md`
3. The linked canonical docs for architecture, testing, and operations

This file is intentionally thin. It exists only as a compatibility shim for
tools that still look for `CLAUDE.md`.

Do not treat this file as a second source of truth. If a rule, command, or
workflow matters for the repository as a whole, it belongs in `AGENTS.md` or a
linked canonical doc instead.

Keep tool-specific local notes in ignored local files such as `CLAUDE.local.md`
or `docs/operations/LOCAL_ACCESS.md`, not in tracked assistant guidance.
