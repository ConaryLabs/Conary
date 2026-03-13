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

## Building from Source

```bash
# Debug build (default, fast compilation)
cargo build

# With Remi server + conaryd daemon
cargo build --features server

# Release build (optimized, slower to compile)
cargo build --release
```

The project is a Cargo workspace with 5 crates: `conary` (CLI), `conary-core` (library), `conary-erofs` (EROFS image builder), `conary-server` (Remi + conaryd), and `conary-test` (test infrastructure).

## Running Tests

```bash
# All library + integration tests
cargo test

# Run a specific test module
cargo test --test database

# Library tests only
cargo test --lib

# Integration tests only
cargo test --test '*'
```

All tests must pass before submitting a PR. The CI pipeline runs:

1. `cargo fmt -- --check` -- formatting
2. `cargo clippy --all-targets --all-features -- -D warnings` -- lints
3. `cargo test --verbose` -- all tests

Run these locally before pushing to save CI round-trips:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## Code Style

### General Conventions

- **File headers**: Every Rust source file starts with its path as a comment:
  ```rust
  // src/module/file.rs
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
- Feature-gate optional functionality: server and daemon code behind `--features server`

### Commit Messages

Write clear, descriptive commit messages. Use the imperative mood in the subject line (e.g., "Add sparse index support" not "Added sparse index support"). Keep the subject under 72 characters.

## Module Overview

The project is a Cargo workspace with 5 crates:

**`conary`** (root) -- CLI binary

| Module | Purpose |
|--------|---------|
| `src/cli/` | CLI definitions and argument parsing |
| `src/commands/` | Command implementations |

**`conary-core`** -- Core library

| Module | Purpose |
|--------|---------|
| `src/db/` | SQLite schema, models, and migrations |
| `src/packages/` | RPM/DEB/Arch package parsers (unified via `common.rs` `PackageMetadata`) |
| `src/compression/` | Unified decompression (Gzip, Xz, Zstd) with format detection |
| `src/repository/` | Remote repository metadata sync |
| `src/resolver/` | SAT-based dependency graph resolution |
| `src/filesystem/` | Content-addressable storage and file deployment |
| `src/delta/` | Binary delta updates |
| `src/version/` | Version parsing and constraint matching |
| `src/container/` | Scriptlet sandboxing via Linux namespace isolation |
| `src/trigger/` | Post-install trigger system |
| `src/scriptlet/` | Scriptlet execution with cross-distro support |
| `src/label/` | Package provenance labels |
| `src/flavor/` | Build variation specifications |
| `src/components/` | Component classification |
| `src/transaction/` | Crash-safe atomic operations with journal-based recovery |
| `src/model/` | System Model -- declarative OS state |
| `src/ccs/` | CCS native package format (builder, policy engine, OCI export) |
| `src/recipe/` | Recipe system for building packages from source |
| `src/capability/` | Capability declarations -- audit, enforcement, inference |
| `src/provenance/` | Package DNA and full provenance tracking |
| `src/automation/` | Automated maintenance (security updates, orphan cleanup) |
| `src/bootstrap/` | Bootstrap a complete Conary system from scratch |
| `src/hash.rs` | Multi-algorithm hashing (SHA-256, XXH128) |

**`conary-erofs`** -- EROFS image builder for composefs integration

**`conary-server`** -- Remi server + conaryd daemon (feature-gated: `--features server`)

| Module | Purpose |
|--------|---------|
| `src/server/` | Remi on-demand CCS conversion proxy |
| `src/daemon/` | conaryd REST API, SSE events, job queue, systemd integration |
| `src/federation/` | CAS federation -- peer discovery, chunk routing, mTLS, mDNS |

**`conary-test`** -- Declarative test infrastructure (TOML manifests, container management)

| Module | Purpose |
|--------|---------|
| `src/config/` | TOML manifest and distro config parsing |
| `src/engine/` | Test suite, runner, assertions |
| `src/container/` | ContainerBackend trait, bollard implementation |
| `src/report/` | JSON output, SSE event streaming |
| `src/server/` | Axum HTTP API, MCP server (rmcp) |

## Pull Request Process

1. **Create a branch** from `main` with a descriptive name:
   - `fix/rpm-parser-overflow`
   - `feat/sparse-index`
   - `docs/update-architecture`

2. **Keep PRs focused**: One logical change per PR. A bug fix and a new feature should be separate PRs.

3. **Include tests**: New features need tests. Bug fixes need a regression test that fails without the fix.

4. **Run CI checks locally** before pushing:
   ```bash
   cargo fmt
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test
   ```

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
- **Feature-gated compilation**: Server and daemon functionality live in the `conary-server` crate, enabled via `--features server` to keep the default binary lean.

Before proposing significant architectural changes, please open an issue to discuss the approach. This helps avoid wasted effort and ensures alignment with the project direction.

## Getting Help

If you have questions about contributing, feel free to start a thread in [GitHub Discussions](https://github.com/ConaryLabs/Conary/discussions) or open an issue on the [GitHub repository](https://github.com/ConaryLabs/Conary). We are happy to help newcomers find their way around the codebase.

## License

Conary is licensed under the [MIT License](LICENSE). By submitting a pull request, you agree that your contributions will be licensed under the same terms.
