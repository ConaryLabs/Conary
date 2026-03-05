# CLAUDE.md

This project uses **Mira** for persistent memory and code intelligence.
Start sessions with `project(action="start", project_path="/home/peter/Conary")`,
then `recall("architecture")` and `recall("progress")` before making changes.

## Build & Test

```bash
cargo build                              # Client-only (default, use for dev)
cargo build --features server            # With Remi server + conaryd daemon
cargo test                               # All tests (1,800+ total)
cargo clippy -- -D warnings              # Lint
```

IMPORTANT: Use debug builds for dev work, never `--release` unless deploying.

## Core Principles

**Database-First**: All state lives in SQLite. No config files for runtime state.

**File Headers**: Every Rust source file starts with its path as a comment:
```rust
// src/main.rs
```

**No Emojis**: Use text markers: `[COMPLETE]`, `[IN PROGRESS]`, `[FAILED]`.

**Rust Standards**: Edition 2024, Rust 1.93, `thiserror` for errors, clippy-clean (pedantic encouraged), tests in same file as code.

## Architecture Glossary

- **Trove**: Core unit (package, component, collection)
- **Changeset**: Atomic transaction (install/remove/rollback)
- **Flavor**: Build variations (arch, features)
- **CAS**: Content-addressable storage for files

Database schema is currently **v44** (40+ tables across 44 migrations). See ROADMAP.md for version history.

## Tool Selection

STOP before using Grep or Glob. Prefer Mira tools for semantic searches:
- `semantic_code_search` over Grep for finding code by intent
- `get_symbols` over grepping for definitions
- `find_callers` / `find_callees` over grepping function names
- `recall` before making architectural changes
- Context7 (`resolve-library-id` then `query-docs`) for external library APIs

Use Grep/Glob only for literal strings, exact filenames, or simple one-off searches.

See `.claude/rules/` for detailed tool selection guides and architecture reference.
