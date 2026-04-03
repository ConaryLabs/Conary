# Contributing to Conary

Thank you for your interest in contributing to Conary. Whether you are fixing a typo, reporting a bug, or building a major feature, your contribution matters. This guide covers everything you need to get started.

## Table of Contents

- [Getting Started](#getting-started)
- [Building from Source](#building-from-source)
- [Running Tests](#running-tests)
- [Code Style](#code-style)
- [Module Overview](#module-overview)
- [Pull Request Process](#pull-request-process)
- [Issue Reporting](#issue-reporting)
- [Architecture Decisions](#architecture-decisions)
- [License](#license)

## Getting Started

### Prerequisites

- **Rust 1.94+** (edition 2024) -- install via [rustup](https://rustup.rs/)
- **SQLite** development headers (`libsqlite3-dev` on Debian/Ubuntu, `sqlite-devel` on Fedora/RHEL, `sqlite` on Arch)
- **Git**
- **Linux** -- Conary uses Linux-specific APIs (namespaces, landlock, seccomp) and does not currently build on macOS or Windows

### Your First Contribution

Not sure where to start? Look for issues labeled [`good first issue`](https://github.com/ConaryLabs/Conary/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22). These are scoped to be completable in a single session and don't require deep knowledge of the codebase.

Good first contributions include:
- Adding or improving unit tests (look for modules with low coverage)
- Fixing clippy warnings or improving error messages
- Documentation improvements in doc comments
- Small bug fixes in package parsers

### Fork and Clone

```bash
# Fork the repository on GitHub, then:
git clone https://github.com/YOUR_USERNAME/Conary.git
cd Conary

# Add upstream remote
git remote add upstream https://github.com/ConaryLabs/Conary.git
```

### Using Coding Assistants

If you work with an LLM coding tool, start with:

1. `AGENTS.md`
2. `docs/llms/README.md`
3. `docs/INTEGRATION-TESTING.md` when validation spans `conary-test`
4. `docs/operations/infrastructure.md` for MCP, deploy, and host workflow notes

Tool-specific files such as `CLAUDE.md` are compatibility shims. Prefer the
linked canonical docs over copied instructions or stale local lore.

## Building from Source

```bash
# Debug builds
cargo build -p conary
cargo build -p remi
cargo build -p conaryd

# Release build (optimized, slower to compile)
cargo build -p conary --release
```

The project root is a virtual Cargo workspace with six members:
`apps/conary`, `apps/remi`, `apps/conaryd`, `apps/conary-test`,
`crates/conary-core`, and `crates/conary-mcp`. EROFS support uses
`composefs-rs` directly in `crates/conary-core`.

## Running Tests

```bash
# CLI + core
cargo test -p conary
cargo test -p conary-core

# Service-owned code
cargo test -p remi
cargo test -p conaryd

# Test harness
cargo test -p conary-test

# Run a specific test module
cargo test --test database

# Library tests only
cargo test --lib

# Integration tests only
cargo test --test '*'
```

All tests must pass before submitting a PR. At minimum, run the verification path that matches the code you touched:

1. `cargo fmt --check` -- formatting
2. `cargo clippy --workspace --all-targets -- -D warnings` -- workspace lint gate
3. `cargo test -p conary` -- CLI tests
4. `cargo test -p remi` -- when touching Remi/server/federation code
5. `cargo test -p conaryd` -- when touching daemon code

Run these locally before pushing to save CI round-trips:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p conary
```

If your change touches Remi, daemon code, federation, or service-owned shared types, also run:

```bash
cargo test -p remi
cargo test -p conaryd
```

## Code Style

### General Conventions

- **File headers**: Every Rust source file starts with its repo-relative path as a comment:
  ```rust
  // apps/conary/src/commands/example.rs
  ```
- **Database-first**: All runtime state lives in SQLite. No config files (INI, TOML, YAML, JSON) for runtime state.
- **No emojis**: Use text markers like `[COMPLETE]`, `[IN PROGRESS]`, `[FAILED]` in output and documentation.
- **Clippy-clean**: All code must pass `cargo clippy -- -D warnings`. Pedantic lints are encouraged.
- **Tests in same file**: Unit tests go in a `#[cfg(test)] mod tests` block at the bottom of each source file, not in separate test files.

### Rust Specifics

- Edition 2024, minimum supported Rust version 1.94
- Use `thiserror` for library/module error types
- Use `anyhow` for application-level error propagation
- Minimize `.unwrap()` in production code paths -- prefer `?` or explicit error handling
- Keep ownership explicit: service and daemon code live in `apps/remi` and `apps/conaryd`, not behind a root feature flag

### Commit Messages

This project uses [Conventional Commits](https://www.conventionalcommits.org/). Every commit message must start with a type prefix:

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

Scopes are optional but encouraged: `feat(resolver): add SAT backtracking`.

Use the imperative mood in the subject line (e.g., "add sparse index support" not "added sparse index support"). Keep the subject under 72 characters. The release pipeline (`scripts/release.sh`) uses these prefixes to determine version bumps and generate changelogs.

## Module Overview

The project is a virtual Cargo workspace with six members: four app crates and
two shared crates.

**`apps/conary`** -- CLI binary

| Module | Purpose |
|--------|---------|
| `apps/conary/src/cli/` | CLI definitions and argument parsing |
| `apps/conary/src/app.rs` | Startup/bootstrap wiring |
| `apps/conary/src/dispatch.rs` | Top-level command routing |
| `apps/conary/src/commands/` | Command implementations |

**`crates/conary-core`** -- Core library

| Module | Purpose |
|--------|---------|
| `crates/conary-core/src/db/` | SQLite schema, models, and migrations |
| `crates/conary-core/src/packages/` | RPM/DEB/Arch package parsers unified through `PackageMetadata` |
| `crates/conary-core/src/compression/` | Unified decompression (Gzip, Xz, Zstd) with format detection |
| `crates/conary-core/src/repository/` | Remote repository metadata sync, mirror logic, and Remi client |
| `crates/conary-core/src/resolver/` | SAT-based dependency graph resolution |
| `crates/conary-core/src/filesystem/` | Content-addressable storage and file deployment |
| `crates/conary-core/src/delta/` | Binary delta updates |
| `crates/conary-core/src/version/` | Version parsing and constraint matching |
| `crates/conary-core/src/container/` | Scriptlet sandboxing via Linux namespace isolation |
| `crates/conary-core/src/trigger/` | Post-install trigger system |
| `crates/conary-core/src/scriptlet/` | Scriptlet execution with cross-distro support |
| `crates/conary-core/src/label.rs` | Package provenance labels |
| `crates/conary-core/src/flavor/` | Build variation specifications |
| `crates/conary-core/src/components/` | Component classification |
| `crates/conary-core/src/transaction/` | Composefs-native transaction pipeline and conflict preflight |
| `crates/conary-core/src/model/` | System Model and remote include handling |
| `crates/conary-core/src/ccs/` | CCS native package format (builder, policy engine, OCI export) |
| `crates/conary-core/src/recipe/` | Recipe system for building packages from source |
| `crates/conary-core/src/capability/` | Capability declarations, enforcement, and inference |
| `crates/conary-core/src/provenance/` | Package DNA and provenance tracking |
| `crates/conary-core/src/automation/` | Automated maintenance (security updates, orphan cleanup) |
| `crates/conary-core/src/bootstrap/` | Bootstrap a complete Conary system from scratch |
| `crates/conary-core/src/generation/` | EROFS generation building, composefs mounting, CAS GC |
| `crates/conary-core/src/derivation/` | CAS-layered derivation engine for bootstrap |
| `crates/conary-core/src/trust/` | TUF supply chain trust |
| `crates/conary-core/src/canonical/` | Cross-distro canonical name mapping (AppStream, Repology) |
| `crates/conary-core/src/self_update.rs` | Self-update version checking, download, atomic replacement |
| `crates/conary-core/src/hash.rs` | Multi-algorithm hashing (SHA-256, XXH128) |

**`crates/conary-mcp`** -- Shared transport-agnostic MCP helpers

| Module | Purpose |
|--------|---------|
| `crates/conary-mcp/src/lib.rs` | MCP server plumbing shared across workspace apps |

**`apps/remi`** -- Remi server + federation service

| Module | Purpose |
|--------|---------|
| `apps/remi/src/server/` | Remi on-demand CCS conversion proxy, search, admin API, and MCP server |
| `apps/remi/src/federation/` | CAS federation -- peer discovery, chunk routing, allowlists, TLS pinning |

**`apps/conaryd`** -- conaryd daemon

| Module | Purpose |
|--------|---------|
| `apps/conaryd/src/daemon/` | conaryd REST API, SSE events, job queue, and systemd integration |

**`apps/conary-test`** -- Declarative test infrastructure (TOML manifests, container management)

| Module | Purpose |
|--------|---------|
| `apps/conary-test/src/config/` | TOML manifest and distro config parsing |
| `apps/conary-test/src/engine/` | Test suite, runner, assertions |
| `apps/conary-test/src/container/` | ContainerBackend trait and container lifecycle |
| `apps/conary-test/src/report/` | JSON output and SSE event streaming |
| `apps/conary-test/src/server/` | Axum HTTP API and MCP server (rmcp) |

## Pull Request Process

1. **Create a branch** from `main` with a descriptive name:
   - `fix/rpm-parser-overflow`
   - `feat/sparse-index`
   - `docs/update-architecture`

2. **Keep PRs focused**: One logical change per PR. A bug fix and a new feature should be separate PRs.

3. **Include tests**: New features need tests. Bug fixes need a regression test that fails without the fix.

4. **Run CI checks locally** before pushing:
   ```bash
   cargo fmt --check
   cargo clippy -- -D warnings
   cargo test
   ```

   Add the `cargo test -p remi` and `cargo test -p conaryd` pair when your change touches service-owned code.

5. **Write a clear PR description** explaining what changed and why. If it addresses an issue, reference it (e.g., "Fixes #42").

6. **Respond to review feedback** constructively. Maintainers may request changes -- this is normal and part of keeping code quality high.

## Issue Reporting

### Bug Reports

When filing a bug report, please include:

- Conary version (`conary --version`)
- Linux distribution and version
- Steps to reproduce
- Expected vs. actual behavior
- Relevant log output (run with `RUST_LOG=debug` for verbose output)

### Feature Requests

Feature requests are welcome. Please search existing issues first to avoid duplicates, and describe:

- The problem you are trying to solve
- Your proposed solution (if you have one)
- Any alternatives you considered

## Architecture Decisions

Conary has a few core design principles that inform how contributions should be structured. Understanding these will help your PR get accepted:

- **Database-first**: SQLite is the single source of truth for all package state. Do not introduce config files, caches outside the database, or in-memory-only state for data that should persist.
- **Content-addressable storage**: Files are stored by hash, enabling deduplication and efficient delta updates.
- **Atomic transactions**: Package operations use journaled changesets for crash safety. Partial installs should never leave the system in a broken state.
- **Package-owned service surfaces**: Remi and conaryd live in their own app crates and should be built and tested directly with `cargo build -p remi`, `cargo build -p conaryd`, `cargo test -p remi`, and `cargo test -p conaryd`.

Before proposing significant architectural changes, please open an issue to discuss the approach. This helps avoid wasted effort and ensures alignment with the project direction.

## Documentation Hygiene

- Treat active docs as current-state references, not historical logs.
- Move completed review prompts/specs/plans into the appropriate `archive/` subtree instead of keeping them in active paths.
- When editing files under `docs/`, update YAML frontmatter (`last_updated`, `revision`, `summary`) unless the file is intentionally exempt.

## Getting Help

If you have questions about contributing, feel free to start a thread in [GitHub Discussions](https://github.com/ConaryLabs/Conary/discussions) or open an issue on the [GitHub repository](https://github.com/ConaryLabs/Conary). We are happy to help newcomers find their way around the codebase.

## License

Conary is licensed under the [MIT License](LICENSE). By submitting a pull request, you agree that your contributions will be licensed under the same terms.
