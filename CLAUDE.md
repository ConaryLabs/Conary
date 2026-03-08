# CLAUDE.md

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

## Commit Convention

Use [Conventional Commits](https://www.conventionalcommits.org/). Every commit message MUST start with a type prefix:

| Prefix | When to use | Version bump |
|--------|-------------|-------------|
| `feat:` | New feature or capability | Minor |
| `fix:` | Bug fix | Patch |
| `docs:` | Documentation only | None |
| `refactor:` | Code restructure, no behavior change | None |
| `test:` | Test additions or changes | None |
| `chore:` | Build, tooling, dependencies | None |
| `security:` | Security fix | Patch |
| `perf:` | Performance improvement | Patch |

Add `!` after the type for breaking changes: `feat!: remove legacy API`.

Scopes are optional: `feat(resolver): add SAT backtracking`.

**Release:** Run `./scripts/release.sh [conary|erofs|server|all]` to auto-bump versions, update CHANGELOG.md, and tag. Use `--dry-run` to preview.

## Architecture Glossary

- **Trove**: Core unit (package, component, collection)
- **Changeset**: Atomic transaction (install/remove/rollback)
- **Flavor**: Build variations (arch, features)
- **CAS**: Content-addressable storage for files

Database schema is currently **v46** (40+ tables across 46 migrations). See ROADMAP.md for what's next.

## Tool Selection

- Context7 (`resolve-library-id` then `query-docs`) for external library APIs
- Use Grep/Glob for code searches, exact filenames, or pattern matching

See `.claude/rules/` for detailed tool selection guides, architecture reference, and infrastructure/CI docs.

## Doc Versioning

When modifying files in `docs/`, add or update a YAML frontmatter header:

```yaml
---
last_updated: 2026-03-06
revision: 1
summary: Brief description of what changed
---
```

- `last_updated`: Set to today's date
- `revision`: Increment on meaningful updates (not typo fixes). Start at 1 for new docs.
- `summary`: One line describing the most recent change
- **Excluded files:** ROADMAP.md, CHANGELOG.md, CONTRIBUTING.md, files in `docs/plans/`

## Agents

Six composable agents, dispatched by `portage`:

| Agent | Role | Invoke |
|-------|------|--------|
| **portage** | Task dispatcher -- classifies and orchestrates | "Use portage to [task]" |
| **lintian** | Code reviewer (read-only, has memory) | "Use lintian to review [scope]" |
| **emerge** | Parallel implementer | "Use emerge to fix [findings]" |
| **valgrind** | Debugger (has memory) | "Use valgrind to debug [issue]" |
| **autopkgtest** | QA/test hardener | "Use autopkgtest on [scope]" |
| **sbuild** | Release verifier | "Use sbuild to prep release" |

For most tasks, just describe what you need and let portage pick the pipeline.
