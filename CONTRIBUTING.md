# Contributing to Conary

Thank you for your interest in contributing to Conary! This document covers everything you need to get started.

## Prerequisites

- **Rust 1.92+** (edition 2024)
- **SQLite** development headers (`libsqlite3-dev` / `sqlite-devel`)
- **Git**

## Building from Source

```bash
# Clone the repository
git clone https://github.com/conary/conary.git
cd conary

# Debug build (default, fast compilation)
cargo build

# With Remi server support
cargo build --features server

# With conaryd daemon support
cargo build --features daemon

# Release build (optimized)
cargo build --release
```

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

# Doc tests only
cargo test --doc
```

All tests should pass before submitting a PR. The CI pipeline runs:
- `cargo fmt -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --verbose`

## Code Style

### General Conventions

- **File headers**: Every Rust source file starts with its path as a comment: `// src/module/file.rs`
- **Database-first**: All runtime state lives in SQLite. No config files for runtime state.
- **No emojis**: Use text markers like `[COMPLETE]`, `[IN PROGRESS]`, `[FAILED]`
- **Clippy-clean**: All code must pass `cargo clippy -- -D warnings`. Pedantic lints encouraged.
- **Tests in same file**: Unit tests go in a `#[cfg(test)] mod tests` block at the bottom of each file.

### Rust Specifics

- Edition 2024
- Use `thiserror` for error types
- Prefer `anyhow` for application-level errors, `thiserror` for library errors
- Minimize `.unwrap()` usage in production paths (use `?` or explicit error handling)

## Module Overview

| Module | Purpose |
|--------|---------|
| `src/db/` | SQLite schema, models, migrations |
| `src/packages/` | RPM/DEB/Arch package parsers |
| `src/compression/` | Unified decompression (Gzip, Xz, Zstd) |
| `src/repository/` | Remote repository metadata sync |
| `src/resolver/` | Dependency graph resolution |
| `src/filesystem/` | Content-addressable storage, file deployment |
| `src/transaction/` | Crash-safe atomic operations |
| `src/model/` | System Model (declarative OS state) |
| `src/ccs/` | CCS native package format |
| `src/server/` | Remi server (feature-gated: `--features server`) |
| `src/daemon/` | conaryd daemon (feature-gated: `--features daemon`) |
| `src/recipe/` | Recipe system for building packages from source |
| `src/capability/` | Package capability declarations and enforcement |
| `src/federation/` | CAS federation for peer chunk sharing |

For detailed module descriptions, see the Architecture section of CLAUDE.md.

## Pull Request Process

1. **Branch naming**: Use descriptive branch names like `fix/rpm-parser-overflow` or `feat/sparse-index`
2. **One concern per PR**: Keep PRs focused. A bug fix + a feature = two PRs.
3. **Tests required**: New features need tests. Bug fixes need a regression test.
4. **Clippy and format**: Run `cargo fmt` and `cargo clippy -- -D warnings` before pushing.
5. **Describe your changes**: The PR description should explain what changed and why.

## Issue Reporting

When filing a bug report, please include:
- Conary version (`conary --version`)
- Linux distribution and version
- Steps to reproduce
- Expected vs actual behavior
- Relevant log output (run with `RUST_LOG=debug` for verbose output)

## Architecture Decisions

Major architectural decisions are documented in the codebase. Before proposing significant changes, please open an issue to discuss the approach. Key design principles:

- **Database-first**: SQLite is the source of truth for all state
- **Content-addressable storage**: Files are stored by hash, enabling deduplication
- **Atomic transactions**: Package operations use journaled changesets for crash safety
- **Feature-gated compilation**: Server and daemon functionality are behind feature flags

## License

By contributing, you agree that your contributions will be licensed under the same license as the project.
