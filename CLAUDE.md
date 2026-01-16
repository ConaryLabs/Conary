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
| `src/packages/` | RPM/DEB/Arch parsers (unified via `common.rs` PackageMetadata) |
| `src/compression/` | Unified decompression (Gzip, Xz, Zstd) with format detection |
| `src/repository/` | Remote repos, metadata sync |
| `src/resolver/` | Dependency graph, topological sort |
| `src/filesystem/` | CAS, file deployment |
| `src/delta/` | Binary delta updates |
| `src/version/` | Version parsing, constraints |
| `src/container/` | Scriptlet sandboxing, namespace isolation |
| `src/trigger/` | Post-install trigger system |
| `src/scriptlet/` | Scriptlet execution, cross-distro support |
| `src/label/` | Package provenance labels |
| `src/flavor/` | Build variation specs |
| `src/components/` | Component classification |
| `src/transaction/` | Crash-safe atomic operations, journal-based recovery |
| `src/ccs/` | CCS native package format, builder, policy engine, OCI export |

## Database Schema

Currently v23. Tables: troves, changesets, files, flavors, provenance, dependencies, repositories, repository_packages, file_contents, file_history, package_deltas, delta_stats, provides, scriptlets, components, component_dependencies, component_provides, collection_members, triggers, trigger_dependencies, changeset_triggers, system_states, state_members, labels, label_path, config_files, config_backups.

Key schema additions:
- v8: `provides` - capability tracking for dependency resolution
- v9: `scriptlets` - package install/remove hooks
- v11: `components`, `component_dependencies`, `component_provides` - component model
- v12: `install_reason` column on troves - for autoremove support
- v13: `collection_members` - package group/collection support
- v14: `flavor_spec` column on troves - Conary-style flavor specifications
- v15: `pinned` column on troves - package pinning support
- v16: `selection_reason` column on troves - for tracking why packages were installed
- v17: `triggers`, `trigger_dependencies`, `changeset_triggers` - trigger system
- v18: `system_states`, `state_members` - system state snapshots
- v19: `kind` column on provides and dependencies - typed dependency matching
- v20: `labels`, `label_path` tables, `label_id` on troves - package provenance tracking
- v21: `config_files`, `config_backups` tables - configuration file tracking and backup
- v22: security columns on `repository_packages` - security update tracking
- v23: `tx_uuid` column on changesets - transaction engine crash recovery correlation

## Testing

```bash
cargo test                    # All tests
cargo test --lib             # Library tests only
cargo test --test '*'        # Integration tests only
cargo test --test database   # Run specific test module
```

508 tests total (459 lib + 3 bin + 35 integration + 11 doctests).

Integration tests are organized in `tests/`:
- `database.rs` - DB init, transactions (6 tests)
- `workflow.rs` - Install/remove/rollback (4 tests)
- `query.rs` - Queries, dependencies, provides (9 tests)
- `component.rs` - Component classification (7 tests)
- `features.rs` - Language deps, collections, state, config (9 tests)
- `common/mod.rs` - Shared test helpers
