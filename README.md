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

**Foundation Complete** - Six phases implemented and tested. The core architecture is solid and working.

### What's Working Now

**Commands Available:**
- `conary init` - Initialize database and storage
- `conary install <package>` - Install RPM packages with full file deployment
- `conary remove <package>` - Remove installed packages (checks dependencies)
- `conary query [pattern]` - List installed packages
- `conary verify [package]` - Verify file integrity with SHA-256
- `conary history` - Show all changeset operations
- `conary rollback <id>` - Rollback any changeset, including filesystem changes
- `conary depends <package>` - Show package dependencies
- `conary rdepends <package>` - Show reverse dependencies (what depends on this)
- `conary whatbreaks <package>` - Show what would break if package removed
- `conary repo-add <name> <url>` - Add a new package repository
- `conary repo-list` - List configured repositories
- `conary repo-remove <name>` - Remove a repository
- `conary repo-enable <name>` - Enable a repository
- `conary repo-disable <name>` - Disable a repository
- `conary repo-sync [name]` - Synchronize repository metadata
- `conary search <pattern>` - Search for packages in repositories
- `conary update [package]` - Update packages with delta-first logic
- `conary delta-stats` - Show delta update statistics and bandwidth savings
- `conary completions <shell>` - Generate shell completion scripts

**Core Features Implemented:**
- **Content-Addressable Storage**: Git-style file storage with automatic deduplication
- **Atomic Operations**: All operations wrapped in transactions - they work completely or not at all
- **Full Rollback**: Database changes AND filesystem changes reversed atomically
- **Conflict Detection**: Smart detection of file conflicts, errors on untracked files
- **File Integrity**: SHA-256 verification of all installed files
- **Schema Migrations**: Database evolves cleanly (currently v5)
- **Changeset Model**: Every operation tracked as a changeset for complete auditability
- **Dependency Resolution**: Graph-based solver with topological sort and cycle detection
- **Version Constraints**: Full RPM version support with semver comparison
- **Delta Updates**: Binary delta compression using zstd dictionary compression (90%+ space savings)
- **Bandwidth Tracking**: Statistics on delta effectiveness and bytes saved across all updates

**Shell Completions:**

Generate completions for your shell:

```bash
# Bash
conary completions bash > /etc/bash_completion.d/conary

# Zsh
conary completions zsh > /usr/share/zsh/site-functions/_conary

# Fish
conary completions fish > ~/.config/fish/completions/conary.fish

# PowerShell
conary completions powershell > conary.ps1
```

**Man Pages:**

Man pages are automatically generated during build and located in `man/conary.1`. View with:

```bash
man ./man/conary.1
```

Or install system-wide:

```bash
sudo cp man/conary.1 /usr/share/man/man1/
sudo mandb
man conary
```

**Repository Management:**

Conary supports remote package repositories for discovering and installing packages:

```bash
# Add a repository
conary repo-add myrepo https://example.com/packages

# List repositories
conary repo-list

# Synchronize package metadata
conary repo-sync

# Search for packages
conary search nginx

# Install from repository (coming soon)
# conary install nginx
```

**Testing:**
- 90 tests passing (73 lib + 7 bin + 10 integration)
- Comprehensive test coverage for CAS, transactions, dependency resolution, repository management, delta operations, and core operations
- Integration tests for full install/remove/rollback workflows

**Core Features Implemented (continued):**
- **Repository Management**: Add remote repositories, sync metadata, search packages
- **HTTP Downloads**: Automatic retry with exponential backoff for reliable downloads
- **JSON Metadata**: Simple JSON-based repository index format
- **Metadata Caching**: Configurable expiry time to minimize bandwidth usage
- **Delta-First Updates**: Automatic fallback from delta to full download if delta unavailable or fails
- **zstd Compression**: Dictionary-based compression using old file as dictionary for excellent ratios

### What's Next

Additional package formats (DEB, Arch), full update command with package installation, dependency resolution during updates, package signing and GPG verification. See ROADMAP.md for details.
