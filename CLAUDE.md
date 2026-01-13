# CLAUDE.md

This project uses **Mira** for persistent memory and code intelligence.

## Session Start

```
session_start(project_path="/home/peter/Conary")
```

Then `recall("architecture")` and `recall("progress")` before making changes.

## Code Navigation (Use These First)

**Always prefer Mira tools over Grep/Glob for code exploration:**

| Need | Tool | Why |
|------|------|-----|
| Search by meaning | `semantic_code_search` | Understands intent, not just keywords |
| File structure | `get_symbols` | Functions, structs, traits in a file |
| Check past decisions | `recall` | What we decided and why |
| Find callers | `find_callers` | What calls a function |
| Find callees | `find_callees` | What a function calls |

**When to use Grep:** Only for literal string searches (error messages, specific constants).

**When to use Glob:** Only for finding files by exact name pattern.

## Build & Test

```bash
cargo build --release
cargo test
cargo clippy -- -D warnings
```

## Core Principles

**Database-First**: All state lives in SQLite. No config files. No INI/TOML/YAML/JSON for runtime state.

**File Headers**: Every Rust source file starts with its path as a comment:
```rust
// src/main.rs
```

**No Emojis**: Use text markers instead:
- `[COMPLETE]` not checkmarks
- `[IN PROGRESS]` not spinners
- `[FAILED]` not X marks

**Rust Standards**:
- Edition 2024, Rust 1.91.1
- `thiserror` for error types
- Clippy-clean (pedantic encouraged)
- Tests in same file as code

## Architecture Quick Reference

- **Trove**: Core unit (package, component, collection)
- **Changeset**: Atomic transaction (install/remove/rollback)
- **Flavor**: Build variations (arch, features)
- **CAS**: Content-addressable storage for files

## Key Modules

| Module | Purpose |
|--------|---------|
| `src/db/` | SQLite schema, models, migrations |
| `src/packages/` | RPM/DEB/Arch parsers |
| `src/repository/` | Remote repos, metadata sync |
| `src/resolver/` | Dependency graph, topological sort |
| `src/filesystem/` | CAS, file deployment |
| `src/delta/` | Binary delta updates |
| `src/version/` | Version parsing, constraints |

## Database Schema

Currently v13. Tables: troves, changesets, files, flavors, provenance, dependencies, repositories, repository_packages, file_contents, file_history, package_deltas, delta_stats, provides, scriptlets, components, component_dependencies, component_provides, collection_members.

Key schema additions:
- v8: `provides` - capability tracking for dependency resolution
- v9: `scriptlets` - package install/remove hooks
- v11: `components`, `component_dependencies`, `component_provides` - component model
- v12: `install_reason` column on troves - for autoremove support
- v13: `collection_members` - package group/collection support

## Testing

```bash
cargo test                    # All tests
cargo test --lib             # Library tests only
cargo test --test '*'        # Integration tests only
```

289 tests total (264 lib + 3 bin + 22 integration).
