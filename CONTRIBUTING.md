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

- **Rust 1.92+** (edition 2024) -- install via [rustup](https://rustup.rs/)
- **SQLite** development headers (`libsqlite3-dev` on Debian/Ubuntu, `sqlite-devel` on Fedora/RHEL)
- **Git**
- **Linux** -- Conary uses Linux-specific APIs (namespaces, landlock, seccomp) and does not currently build on macOS or Windows

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

# With Remi server support
cargo build --features server

# With conaryd daemon support (includes server)
cargo build --features daemon

# Release build (optimized, slower to compile)
cargo build --release
```

Note: the `daemon` feature implies `server`, so `--features daemon` includes all server functionality.

## Running Tests

```bash
# All library + integration tests
cargo test

# Include daemon tests
cargo test --features daemon

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

- Edition 2024, minimum supported Rust version 1.92
- Use `thiserror` for library/module error types
- Use `anyhow` for application-level error propagation
- Minimize `.unwrap()` in production code paths -- prefer `?` or explicit error handling
- Feature-gate optional functionality: server code behind `--features server`, daemon code behind `--features daemon`

### Commit Messages

Write clear, descriptive commit messages. Use the imperative mood in the subject line (e.g., "Add sparse index support" not "Added sparse index support"). Keep the subject under 72 characters.

## Module Overview

The codebase is organized into focused modules under `src/`. Here is a summary to help you find what you are looking for:

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
| `src/model/` | System Model -- declarative OS state (parser, diff, state capture, remote includes, publishing) |
| `src/ccs/` | CCS native package format (builder, policy engine, OCI export, lockfile, redirects) |
| `src/server/` | Remi server -- on-demand CCS conversion proxy (feature-gated) |
| `src/cli/` | CLI definitions and argument parsing |
| `src/commands/` | Command implementations |
| `src/commands/install/` | Package installation pipeline (resolve, prepare, execute) |
| `src/recipe/` | Recipe system for building packages from source (hermetic builds, PKGBUILD conversion) |
| `src/capability/` | Capability declarations (network, filesystem, syscalls) -- audit, enforcement, inference |
| `src/provenance/` | Package DNA and full provenance tracking (source, build, signatures, content) |
| `src/automation/` | Automated maintenance (security updates, orphan cleanup) |
| `src/bootstrap/` | Bootstrap a complete Conary system from scratch |
| `src/federation/` | CAS federation -- peer discovery, chunk routing, manifests, mTLS, mDNS |
| `src/daemon/` | conaryd daemon -- REST API, SSE events, job queue, systemd integration (feature-gated) |
| `src/hash/` | Hashing utilities for file integrity |
| `src/progress/` | Progress reporting for long-running operations |

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
- **Feature-gated compilation**: Server (`--features server`) and daemon (`--features daemon`) functionality are behind Cargo feature flags to keep the default binary lean.

Before proposing significant architectural changes, please open an issue to discuss the approach. This helps avoid wasted effort and ensures alignment with the project direction.

## Getting Help

If you have questions about contributing, feel free to open a discussion or issue on the [GitHub repository](https://github.com/ConaryLabs/Conary). We are happy to help newcomers find their way around the codebase.

## License

Conary is licensed under the [MIT License](LICENSE). By submitting a pull request, you agree that your contributions will be licensed under the same terms.
