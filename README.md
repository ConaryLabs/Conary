# Conary

## Vision

Package management is fundamentally broken. You're locked to one distro's format, updates are coin flips that might brick your system, and when shit goes wrong you're reinstalling from scratch like it's 1999. We're stuck with tools designed when "rollback" meant restore from backup and "atomic" wasn't even a consideration.

Conary is package management rebuilt on principles that should've been standard a decade ago - inspired by the original Conary package manager that was criminally ahead of its time.

### Changesets, Not Just Packages
Every operation is a **changeset** - a transactional move from one system state to another. Installing isn't "add package X", it's "apply changeset that includes X and its dependencies". Rollback isn't cleanup, it's just applying the inverse changeset. This is atomic by design - it works completely or not at all. No half-configured systems, no dependency limbo.

### Troves All The Way Down
The core unit is a **trove** - whether it's a single library, a component (`:runtime`, `:devel`, `:doc`), or an entire collection of packages. Hierarchical and composable. Install just what you need. Query at any level. It's the same concept whether you're asking about one binary or your entire desktop environment.

### Components: Fine-Grained Installation
Every package is automatically split into **components** - `:runtime`, `:lib`, `:devel`, `:doc`, `:config`, and more. Install only what you need: `nginx:runtime` without the docs, or `openssl:devel` for building without the runtime. Components have their own dependencies, and scriptlets only run when appropriate components are installed.

### Collections: Package Groups
Group related packages into **collections** - meta-packages that let you install entire software stacks with one command. Create a `web-stack` collection containing nginx, postgresql, and redis. Install it all at once, manage membership dynamically.

### Flavors For Modern Builds
Build-time variations matter more than ever. Cross-compilation, musl vs glibc, feature flags, different architectures - these are encoded as **flavors**. One package definition, multiple builds, clean metadata. `nginx[ssl,http3]` vs `nginx[!ssl]` - you get what you specify, tracked properly.

### File-Level Tracking & Delta Updates
Every file is tracked in the database with its hash, ownership, and permissions. You can query exactly what owns what, detect conflicts, and verify integrity at any time. Updates use **binary deltas** for large files - why download 500MB when only 5MB changed? Bandwidth-constrained users rejoice. The infrastructure supports it naturally because changesets already track exactly what changed at the file level.

### Format Agnostic
RPM, DEB, Arch packages - Conary speaks all of them. Stop letting package format dictate your entire OS choice.

### Time Travel Built In
Every system state is tracked in SQLite. Rollback isn't an afterthought - it's core functionality. Bad update? Go back. Want to test something? Branch your system state. Every changeset is logged, every state is queryable.

### System Adoption
Already have packages installed by your distro's package manager? **Adopt** them into Conary's database without reinstalling. Conary scans your system, detects installed packages, and tracks them alongside Conary-managed ones. Unified management without disruption.

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
- **Database schema v13** with automatic migrations

## Status

**Core Architecture Complete** - All major features implemented and tested. Component model, collections, system adoption, multi-format support, and dependency resolution all working.

### Commands Available

**Package Management:**
- `conary init` - Initialize database and storage
- `conary install <package>` - Install packages from file or repository (supports --version, --repo, --dry-run, --no-scripts)
- `conary remove <package>` - Remove installed packages (checks dependencies)
- `conary update [package]` - Update packages with delta-first logic
- `conary autoremove` - Remove orphaned dependencies no longer needed
- `conary verify [package]` - Verify file integrity with SHA-256
- `conary restore <package>` - Restore modified/deleted files from CAS
- `conary restore-all` - Restore all modified files across all packages

**Query & Information:**
- `conary query [pattern]` - List installed packages
- `conary depends <package>` - Show package dependencies
- `conary rdepends <package>` - Show reverse dependencies (what depends on this)
- `conary whatbreaks <package>` - Show what would break if package removed
- `conary whatprovides <capability>` - Find what package provides a capability
- `conary history` - Show all changeset operations
- `conary scripts <package.rpm>` - Display scriptlets from a package file

**Component Commands:**
- `conary list-components <package>` - Show components of an installed package
- `conary query-component <pkg:comp>` - Query files in a specific component

**Collection Commands:**
- `conary collection-create <name>` - Create a new package collection
- `conary collection-list` - List all collections
- `conary collection-show <name>` - Show collection details and members
- `conary collection-add <name> --members <pkg1,pkg2>` - Add packages to collection
- `conary collection-remove <name> --members <pkg1,pkg2>` - Remove packages from collection
- `conary collection-delete <name>` - Delete a collection
- `conary collection-install <name>` - Install all packages in a collection

**System Adoption:**
- `conary adopt <package>` - Adopt a single system package into Conary
- `conary adopt-system` - Scan and adopt all system packages
- `conary adopt-status` - Show adoption status summary
- `conary conflicts` - Show file conflicts between packages

**Repository Management:**
- `conary repo-add <name> <url>` - Add a new package repository
- `conary repo-list` - List configured repositories
- `conary repo-remove <name>` - Remove a repository
- `conary repo-enable <name>` - Enable a repository
- `conary repo-disable <name>` - Disable a repository
- `conary repo-sync [name]` - Synchronize repository metadata
- `conary search <pattern>` - Search for packages in repositories

**GPG Key Management:**
- `conary key-import <path>` - Import a GPG public key
- `conary key-list` - List imported GPG keys
- `conary key-remove <fingerprint>` - Remove a GPG key

**System Operations:**
- `conary rollback <id>` - Rollback any changeset, including filesystem changes
- `conary delta-stats` - Show delta update statistics and bandwidth savings
- `conary completions <shell>` - Generate shell completion scripts

### Core Features

**Multi-Format Support:**
- **RPM packages** - Full support including scriptlets, dependencies, and rich metadata
- **DEB packages** - Debian/Ubuntu package format support
- **Arch packages** - pkg.tar.zst and pkg.tar.xz format support
- Automatic format detection via magic bytes or file extension

**Component Model:**
- Automatic file classification into components: `:runtime`, `:lib`, `:devel`, `:doc`, `:config`, `:debuginfo`, `:test`
- Component-level installation - install only what you need
- Smart scriptlet gating - scripts only run when `:runtime` or `:lib` components are installed
- Arch-aware library detection (supports multiarch paths)

**Language Dependency Detection:**
- Automatic detection of Python, Perl, Ruby, and Java modules
- Soname tracking for shared libraries
- Proper provides/requires relationships for language ecosystems

**Collections:**
- Create meta-packages grouping related software
- Optional members support
- Install entire collections with one command
- Track collection membership dynamically

**Dependency Management:**
- Graph-based solver with topological sort and cycle detection
- Full RPM version support with semver comparison
- Track installation reason (explicit vs dependency)
- Autoremove orphaned dependencies safely
- whatprovides query for capability lookup

**Content-Addressable Storage:**
- Git-style file storage with automatic deduplication
- Restore modified files from the object store
- SHA-256 verification of all installed files

**Atomic Operations:**
- All operations wrapped in database transactions
- Full rollback support - database AND filesystem changes reversed atomically
- Conflict detection for files owned by other packages

**Delta Updates:**
- Binary delta compression using zstd dictionary compression
- 90%+ space savings on updates
- Automatic fallback from delta to full download
- Bandwidth tracking and statistics

**Repository System:**
- HTTP downloads with automatic retry and exponential backoff
- JSON-based repository index format
- Metadata caching with configurable expiry
- Priority-based repository selection

**GPG Signature Verification:**
- Import and manage trusted GPG keys
- Verify package signatures before installation
- Strict mode available for signature enforcement

**System Adoption:**
- Scan and adopt packages from RPM/APT databases
- Unified management of distro and Conary packages
- Conflict detection and resolution

### Shell Completions

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

### Man Pages

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

### Repository Usage

```bash
# Add a repository
conary repo-add myrepo https://example.com/packages

# List repositories
conary repo-list

# Synchronize package metadata
conary repo-sync

# Search for packages
conary search nginx

# Install from repository by name
conary install nginx

# Install specific version
conary install nginx --version=1.20.1

# Install from specific repository
conary install nginx --repo=myrepo

# Preview installation without installing
conary install nginx --dry-run
```

### Collection Usage

```bash
# Create a collection
conary collection-create web-stack --description "Web server stack" --members nginx,postgresql,redis

# List collections
conary collection-list

# Show collection details
conary collection-show web-stack

# Add more packages
conary collection-add web-stack --members memcached,nodejs

# Install all packages in a collection
conary collection-install web-stack

# Remove packages from collection
conary collection-remove web-stack --members memcached

# Delete collection (doesn't uninstall packages)
conary collection-delete web-stack
```

### Testing

- **289 tests passing** (264 lib + 3 bin + 22 integration)
- Comprehensive test coverage for CAS, transactions, dependency resolution, repository management, delta operations, component classification, collections, and core operations
- Integration tests for full install/remove/rollback workflows

### What's Next

Parallel downloads for dependencies, improved transitive dependency resolution, web UI for system state visualization, and package building tools. See ROADMAP.md for details.
