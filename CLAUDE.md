# CLAUDE.md

## Project: Conary Package Manager

Modern, Rust-based package manager supporting RPM, DEB, and Arch packages with atomic operations, rollback capabilities, and delta updates. Inspired by the original Conary package manager.

## Core Principles

### Database-First Architecture
**CRITICAL**: This is a **database-backed system**. No configuration files. No state in text files. No INI, TOML, YAML, or JSON for runtime state or configuration.

- All state lives in SQLite
- All configuration lives in SQLite
- Package metadata lives in SQLite
- System state and history lives in SQLite

Text files are only acceptable for:
- Source code
- Documentation (README.md, this file, PROGRESS.md)
- Build configuration (Cargo.toml, Cargo.lock)

If you're tempted to write a config file, you're doing it wrong. Put it in the database.

### Code Standards

**File Headers**
Every Rust source file MUST start with its full path as a comment:
```rust
// src/main.rs
// or
// src/db/schema.rs
```

**Modern Rust (2025)**
- Rust 1.91.1 stable, Edition 2024
- Use modern patterns: async where appropriate, const generics, GATs when useful
- Prefer `?` operator over `match` for error handling
- Use `thiserror` for error types, `anyhow` only at application boundaries
- Clippy-clean code (pedantic lints encouraged)
- Format with `rustfmt` defaults

**Dependencies**
Keep the dependency tree lean. This is system-level infrastructure.
- `rusqlite` for database
- Choose dependencies carefully - fewer is better
- No unnecessary async if sync works fine
- Justify any heavy dependencies

**Testing**
- Unit tests in the same file as the code
- Integration tests in `tests/`
- Database tests use in-memory SQLite or temp files
- Test coverage matters for a package manager

**No Emojis**
- No emojis in source code
- No emojis in documentation files (README.md, PROGRESS.md, CLAUDE.md, ROADMAP.md)
- No emojis in commit messages (title or body)
- Use plain text markers instead:
  - Use `[COMPLETE]` or `(complete)` instead of ✅
  - Use `[IN PROGRESS]` or `(in progress)` instead of 🔄
  - Use `[FAILED]` or `(failed)` instead of ❌
  - Use `NEW` or `(new)` instead of ⭐

### Architecture Concepts

**Conary-Inspired Terminology**
- **Trove**: The core unit - package, component, or collection
- **Changeset**: Transactional state changes, not individual package operations
- **Flavor**: Build-time variations (arch, features, toolchain)
- **Component**: Auto-split packages (`:runtime`, `:devel`, `:doc`)

**Database Schema**
Design for:
- File-level tracking with hashes
- Changeset history and rollback
- Provenance tracking (source, branch, build chain)
- Efficient queries for dependency resolution
- Support for multiple package formats (RPM, DEB, Arch)

### Progress Tracking

Keep `PROGRESS.md` updated with:
- What was implemented
- What's next
- Design decisions and rationale
- Known issues or TODOs
- Session-by-session progress

Update it frequently. It's the source of truth for "where are we?"

## Getting Started

1. Check `PROGRESS.md` for current state
2. Review existing schema/structure
3. Write tests before implementation
4. Update `PROGRESS.md` when done

## Questions/Clarifications

When in doubt, ask. This is foundational infrastructure - getting it right matters more than getting it fast.
