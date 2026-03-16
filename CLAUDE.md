# CLAUDE.md

## Build & Test

```bash
cargo build                              # Client-only (default, use for dev)
cargo build --features server            # With Remi server + conaryd daemon
cargo test                               # ~260 unit tests + 76 integration tests
cargo build -p conary-test               # Test infrastructure crate
cargo test -p conary-test                # Test engine unit tests
cargo clippy -- -D warnings              # Lint
cargo fmt --check                        # Format check
```

IMPORTANT: Use debug builds for dev work, never `--release` unless deploying.

## Core Principles

**Database-First**: All state lives in SQLite. No config files for runtime state.

**File Headers**: Every Rust source file starts with its path as a comment:
```rust
// src/main.rs
```

**No Emojis**: Use text markers: `[COMPLETE]`, `[IN PROGRESS]`, `[FAILED]`.

**Rust Standards**: Edition 2024, Rust 1.94, `thiserror` for errors, clippy-clean (pedantic encouraged), tests in same file as code.

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

**Release:** Run `./scripts/release.sh [conary|erofs|server|test|all]` to auto-bump versions, update CHANGELOG.md, and tag. Use `--dry-run` to preview.

**Publish:** Push a `v*` tag to trigger `.github/workflows/release.yml`, which builds CCS + native packages (RPM/DEB/Arch) in parallel containers and deploys to Remi. Forgejo's `release.yaml` automatically verifies the release landed. See `.claude/rules/infrastructure.md` for details.

## Architecture Glossary

- **Trove**: Core unit (package, component, collection)
- **Changeset**: Atomic transaction (install/remove/rollback)
- **Flavor**: Build variations (arch, features)
- **CAS**: Content-addressable storage for files
- **conary-test**: Test infrastructure -- declarative TOML engine, container management (bollard), HTTP API, MCP (23 tools)

Database schema is currently **v51** (50+ tables, version-gated migration blocks in `schema.rs`). See ROADMAP.md for what's next.

## Tool Selection

- Context7 (`resolve-library-id` then `query-docs`) for external library APIs
- Use Grep/Glob for code searches, exact filenames, or pattern matching

See `.claude/rules/` for detailed tool selection guides, architecture reference, and infrastructure/CI docs.

## MCP Servers

Two MCP servers are configured for direct infrastructure interaction:

| Server | Endpoint | Purpose |
|--------|----------|---------|
| **remi-admin** | `packages.conary.io:8082/mcp` | Remi production server management |
| **conary-test** | `forge.conarylabs.com:9090/mcp` | Test infrastructure on Forge |

**remi-admin** tools: CI workflows (`ci_dispatch`, `ci_list_runs`, `ci_get_run`, `ci_get_logs`), mirror sync, token management, repo inspection, federation peers, audit log, test data (`test_list_runs`, `test_get_run`, `test_get_test`, `test_get_logs`, `test_health`), canonical mapping (`canonical_rebuild`), chunk GC (`chunk_gc`).

**conary-test** tools: Start/monitor test runs (`start_run`, `get_run`, `list_runs`), inspect results (`get_test`, `get_test_logs`), rerun failures (`rerun_test`), manage images (`build_image`, `list_images`, `prune_images`, `image_info`), cleanup containers, reload manifests, deployment ops (`deploy_source`, `rebuild_binary`, `restart_service`, `deploy_status`, `build_fixtures`, `publish_fixtures`, `flush_pending`).

Use these MCP tools instead of SSH/curl for infrastructure operations. After a service restart on Forge, the MCP session goes stale -- restart Claude Code to reconnect.

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
