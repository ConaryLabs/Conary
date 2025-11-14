# Conary

## Vision

Package management is fundamentally broken. You're locked to one distro's format, updates are coin flips that might brick your system, and when shit goes wrong you're reinstalling from scratch like it's 1999. We're stuck with tools designed when "rollback" meant restore from backup and "atomic" wasn't even a consideration.

Conary is package management rebuilt on principles that should've been standard a decade ago - inspired by the original Conary package manager that was criminally ahead of its time.

### Changesets, Not Just Packages
Every operation is a **changeset** - a transactional move from one system state to another. Installing isn't "add package X", it's "apply changeset that includes X and its dependencies". Rollback isn't cleanup, it's just applying the inverse changeset. This is atomic by design - it works completely or not at all. No half-configured systems, no dependency limbo.

### Troves All The Way Down
The core unit is a **trove** - whether it's a single library, a component (`:runtime`, `:devel`, `:doc`), or an entire collection of packages. Hierarchical and composable. Install just what you need. Query at any level. It's the same concept whether you're asking about one binary or your entire desktop environment.

### Flavors For Modern Builds
Build-time variations matter more than ever. Cross-compilation, musl vs glibc, feature flags, different architectures - these are encoded as **flavors**. One package definition, multiple builds, clean metadata. `nginx[ssl,http3]` vs `nginx[!ssl]` - you get what you specify, tracked properly.

### File-Level Tracking & Delta Updates
Every file is tracked in the database with its hash, ownership, and permissions. You can query exactly what owns what, detect conflicts, and verify integrity at any time. Updates use **binary deltas** for large files - why download 500MB when only 5MB changed? Bandwidth-constrained users rejoice. The infrastructure supports it naturally because changesets already track exactly what changed at the file level.

### Format Agnostic
RPM, DEB, Arch packages - they're all just compressed files with metadata. Stop letting package format dictate your entire OS choice. Conary speaks all of them.

### Time Travel Built In
Every system state is tracked in SQLite. Rollback isn't an afterthought - it's core functionality. Bad update? Go back. Want to test something? Branch your system state. Every changeset is logged, every state is queryable.

### Provenance Tracking
Know where your software comes from. Every trove tracks its source, branch, and build chain. Supply chain security isn't optional in 2025.

### Memory Safe Foundation
Written in Rust because package managers touch everything on your system and should never segfault or have buffer overflows. The infrastructure layer should be bulletproof.

### Queryable State
SQLite backend means you can actually query your system: "What installed this dependency?", "What will break if I remove X?", "Show me everything from this repo", "What changesets modified this trove?" No more grepping logs and parsing command output.

---

The goal isn't to replace distros - it's to decouple package management from distro politics and give users the reliability and flexibility they deserve.

## Technical Foundation

- **Rust 1.91.1** (stable) with **Edition 2024**
- **SQLite** via **rusqlite** - synchronous, battle-tested, perfect for changeset operations
- **File-level tracking** - Every file hashed and recorded for integrity, conflict detection, and delta updates
- **Conary-inspired architecture** - troves, changesets, flavors, and components modernized for 2025

## Status

Early development. Watch this space.
