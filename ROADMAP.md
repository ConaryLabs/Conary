# Conary Roadmap

This document tracks the implementation status of Conary features, both completed and planned.

## Completed Features

### Core Architecture

- [COMPLETE] **Trove Model** - Core unit for packages, components, and collections
- [COMPLETE] **Changeset System** - Atomic transactions for all operations
- [COMPLETE] **SQLite Backend** - All state in queryable database (schema v13)
- [COMPLETE] **Content-Addressable Storage** - Git-style file deduplication
- [COMPLETE] **File-Level Tracking** - SHA-256 hashes, ownership, permissions for all files
- [COMPLETE] **Schema Migrations** - Automatic database evolution (v1-v13)

### Package Formats

- [COMPLETE] **RPM Support** - Full parsing including scriptlets, dependencies, rich metadata
- [COMPLETE] **DEB Support** - Debian/Ubuntu package format
- [COMPLETE] **Arch Support** - pkg.tar.zst and pkg.tar.xz formats
- [COMPLETE] **Format Detection** - Automatic detection via magic bytes or extension

### Component Model

- [COMPLETE] **Automatic Classification** - Files classified into :runtime, :lib, :devel, :doc, :config, :debuginfo, :test
- [COMPLETE] **Component Storage** - Components table with parent trove linkage
- [COMPLETE] **File-Component Linkage** - Each file linked to its component
- [COMPLETE] **Scriptlet Gating** - Scripts only run when :runtime or :lib installed
- [COMPLETE] **Arch-Aware Libs** - Multiarch path detection (/lib64, /usr/lib/x86_64-linux-gnu, etc.)
- [COMPLETE] **Component Queries** - list-components, query-component commands

### Collections (Groups)

- [COMPLETE] **Collection Creation** - Create named package groups
- [COMPLETE] **Member Management** - Add/remove packages from collections
- [COMPLETE] **Optional Members** - Flag members as optional
- [COMPLETE] **Bulk Installation** - Install all collection members at once
- [COMPLETE] **Collection Queries** - List, show, search collections

### Dependency Management

- [COMPLETE] **Graph-Based Solver** - Topological sort with cycle detection
- [COMPLETE] **Version Constraints** - Full RPM version comparison
- [COMPLETE] **Install Reason Tracking** - Explicit vs dependency installation
- [COMPLETE] **Orphan Detection** - Find dependencies no longer needed
- [COMPLETE] **Autoremove** - Safe removal of orphaned packages
- [COMPLETE] **whatprovides** - Query what package provides a capability
- [COMPLETE] **whatbreaks** - Show what would break if package removed
- [COMPLETE] **rdepends** - Reverse dependency queries

### Language Dependency Detection

- [COMPLETE] **Python Modules** - Detect python() provides
- [COMPLETE] **Perl Modules** - Detect perl() provides
- [COMPLETE] **Ruby Modules** - Detect ruby() provides
- [COMPLETE] **Java Packages** - Detect java() provides
- [COMPLETE] **Soname Tracking** - Shared library soname provides

### Repository System

- [COMPLETE] **Repository Management** - Add, remove, enable, disable repos
- [COMPLETE] **Metadata Sync** - Download and cache repository indexes
- [COMPLETE] **Package Search** - Search across repositories
- [COMPLETE] **Priority-Based Selection** - Higher priority repos preferred
- [COMPLETE] **HTTP Downloads** - Retry with exponential backoff
- [COMPLETE] **Metadata Caching** - Configurable expiry time

### Delta Updates

- [COMPLETE] **Binary Deltas** - zstd dictionary compression
- [COMPLETE] **Delta Generation** - Create deltas between versions
- [COMPLETE] **Delta Application** - Apply deltas to upgrade packages
- [COMPLETE] **Automatic Fallback** - Fall back to full download if delta fails
- [COMPLETE] **Bandwidth Statistics** - Track bytes saved across updates

### Security

- [COMPLETE] **GPG Key Import** - Import trusted public keys
- [COMPLETE] **Key Management** - List, remove imported keys
- [COMPLETE] **Signature Verification** - Verify package signatures
- [COMPLETE] **Strict Mode** - Require valid signatures for all packages

### System Operations

- [COMPLETE] **Full Rollback** - Reverse database AND filesystem changes
- [COMPLETE] **File Restore** - Restore modified/deleted files from CAS
- [COMPLETE] **Integrity Verification** - Verify installed files against hashes
- [COMPLETE] **Conflict Detection** - Detect file conflicts between packages
- [COMPLETE] **History Tracking** - Complete audit log of all operations

### System Adoption

- [COMPLETE] **Single Package Adoption** - Adopt individual system packages
- [COMPLETE] **System Scan** - Scan and adopt all installed packages
- [COMPLETE] **Adoption Status** - Show adoption progress summary
- [COMPLETE] **Conflict Resolution** - Handle adopted package conflicts

### CLI

- [COMPLETE] **Shell Completions** - Bash, Zsh, Fish, PowerShell
- [COMPLETE] **Man Pages** - Auto-generated documentation
- [COMPLETE] **Dry Run Mode** - Preview operations without executing
- [COMPLETE] **Scriptlet Display** - View package scriptlets before install

---

## In Progress / Short-Term

### Enhanced Flavors

- [ ] **Flavor Parsing** - Parse flavor specifications like `[ssl, !debug, is: x86_64]`
- [ ] **Flavor Matching** - Select packages by flavor requirements
- [ ] **Flavor Operators** - Support `~` (prefers), `!` (not), `~!` (prefers not)
- [ ] **Architecture Flavors** - `is: x86`, `is: x86_64`, `is: aarch64`

### Package Pinning

- [ ] **Pin Command** - Pin packages to prevent modification during updates
- [ ] **Unpin Command** - Allow pinned packages to be updated
- [ ] **Multi-Version Support** - Keep multiple versions of pinned packages (like kernels)

### Parallel Operations

- [ ] **Parallel Downloads** - Download multiple packages concurrently
- [ ] **Parallel Extraction** - Extract package contents in parallel
- [ ] **Download Progress** - Show aggregate progress for parallel downloads

### Transitive Dependencies

- [ ] **Deep Resolution** - Recursively resolve all dependencies
- [ ] **Dependency Tree** - Show full dependency tree visualization
- [ ] **Circular Detection** - Better handling of circular dependencies

### Selection Reasons (Inspired by Aeryn OS)

- [ ] **Reason Text Field** - Add human-readable reason to install tracking
- [ ] **Dependency Chain** - Track "Required by X" for dependency installs
- [ ] **Collection Attribution** - Track "Installed via collection Y"
- [ ] **Query by Reason** - Filter packages by installation reason

---

## Medium-Term

### Trigger System (Inspired by Aeryn OS)

A general-purpose handler system for post-installation actions, more flexible than scriptlets.

- [ ] **Trigger Definition** - Path patterns mapped to handler scripts
- [ ] **Handler Registry** - Register handlers for file types (ldconfig, mime, icons, etc.)
- [ ] **DAG Ordering** - Triggers declare before/after dependencies
- [ ] **Topological Execution** - Run triggers in dependency order
- [ ] **Built-in Triggers** - ldconfig, update-mime-database, gtk-update-icon-cache, systemd-reload

### System State Snapshots (Inspired by Aeryn OS)

Full system state tracking for cleaner rollback semantics.

- [ ] **State Table** - Store complete package sets as numbered states
- [ ] **State Metadata** - ID, timestamp, summary, description for each state
- [ ] **State Diff** - Compare two states to see what changed
- [ ] **State Restore** - Rollback to any previous state by ID
- [ ] **State Pruning** - Garbage collect old states to save space
- [ ] **Active State Tracking** - Track current system state ID

### Typed Dependencies (Inspired by Aeryn OS)

Formalize dependency kinds with explicit type prefixes.

- [ ] **Dependency Kinds** - PackageName, SharedLibrary, PkgConfig, Interpreter, CMake, Python, Binary
- [ ] **Kind Format** - `kind(target)` syntax e.g., `pkgconfig(zlib)`, `python(flask)`
- [ ] **Kind Matching** - Resolve dependencies by matching kinds
- [ ] **Provider Kinds** - Packages declare what kinds they provide
- [ ] **Migration** - Convert existing string deps to typed format

### Labels System

Inspired by original Conary's label concept for tracking package provenance.

- [ ] **Label Format** - `repository@namespace:tag` format
- [ ] **Label Path** - Configure search order for labels
- [ ] **Label Tracking** - Track which label a package came from
- [ ] **Branch History** - Track parent labels in version strings

### Enhanced Queries

- [ ] **repquery** - Query available packages in repositories (not just installed)
- [ ] **Path Query** - `conary query --path /usr/bin/foo` - find package by file
- [ ] **Info Query** - Detailed package information with `--info` flag
- [ ] **File Listing** - `--lsl` for ls -l style file listing

### Configuration Management

- [ ] **Config File Merging** - Preserve user config during upgrades
- [ ] **Config File Tracking** - Track which files are configuration
- [ ] **Config Backup** - Backup configs before modification
- [ ] **Config Diff** - Show differences between installed and package configs

### Update Improvements

- [ ] **updateall** - Update all packages to latest versions
- [ ] **Critical Updates** - `--apply-critical` for security updates only
- [ ] **Update Groups** - Update entire groups atomically

---

## Long-Term / Future Consideration

### Container-Isolated Scriptlets (Inspired by Aeryn OS)

Run package scripts in lightweight Linux containers for safety.

- [ ] **Namespace Isolation** - Mount, PID, IPC, UTS namespaces for scriptlets
- [ ] **Pivot Root** - Isolate scriptlet filesystem from host
- [ ] **Bind Mounts** - Controlled access to required host paths
- [ ] **Rootless Containers** - Support unprivileged container execution
- [ ] **Resource Limits** - CPU, memory, time limits for scriptlets
- [ ] **Dangerous Script Detection** - Flag scripts that need sandboxing

### Atomic Filesystem Updates (Inspired by Aeryn OS)

Use atomic operations to swap entire filesystem trees.

- [ ] **Staging Directory** - Build complete filesystem tree before deployment
- [ ] **renameat2 RENAME_EXCHANGE** - Atomic directory swap on Linux
- [ ] **Content-Addressable /usr** - Deduplicated, immutable /usr trees
- [ ] **Instant Rollback** - Swap back to previous tree atomically
- [ ] **Fallback Strategy** - Graceful degradation on non-Linux systems

### VFS Tree with Reparenting (Inspired by Aeryn OS)

Virtual filesystem tree for efficient file operations.

- [ ] **Arena Allocator** - Efficient node storage for large trees
- [ ] **O(1) Path Lookup** - HashMap for instant path-to-node resolution
- [ ] **Subtree Reparenting** - Efficiently move entire subtrees
- [ ] **Component Merging** - Merge component trees for installation

### Fast Hashing Option (Inspired by Aeryn OS)

Optional xxhash for non-cryptographic use cases.

- [ ] **xxhash Support** - Add xxh128 as alternative to SHA-256
- [ ] **Hash Selection** - Configure hash algorithm per use case
- [ ] **Dedup with xxhash** - Faster deduplication checks
- [ ] **Verify with SHA-256** - Keep SHA-256 for security verification

### Package Building

The original Conary had a full recipe system for building packages. This would be a major undertaking.

- [ ] **Recipe Parser** - Parse Conary recipe files
- [ ] **Source Components** - Store :source troves in repository
- [ ] **Build Actions** - addArchive, addPatch, Make, MakeInstall, etc.
- [ ] **Policy Actions** - ComponentSpec, Config, Ownership, etc.
- [ ] **Factory System** - Templates for common package types
- [ ] **Derived Packages** - Create packages based on existing ones
- [ ] **Shadowing** - Branch packages for customization

### Repository Server

- [ ] **Conary Repository Service** - Network-accessible repository
- [ ] **Version Control** - Repository as version control system
- [ ] **Commit/Checkout** - Check in/out package sources
- [ ] **Branch Management** - Create and manage branches

### Advanced Features

- [ ] **Migrate Command** - Migrate system to new group version
- [ ] **Info Packages** - Create system users/groups via packages
- [ ] **Redirect Packages** - Redirect one package to another
- [ ] **Capsule Packages** - Encapsulate foreign packages

### Web Interface

- [ ] **System State Dashboard** - Visual view of installed packages
- [ ] **Changeset Browser** - Browse and compare changesets
- [ ] **Dependency Graph** - Visual dependency tree
- [ ] **Update Preview** - Preview updates before applying

---

## Not Planned

These features from original Conary are not planned for implementation:

- **rBuilder Integration** - Proprietary appliance builder
- **cvc Tool** - Conary version control (replaced by standard git workflows)
- **Appliance Groups** - Specific to rPath's appliance model
- **GNOME/KDE Package Templates** - Too specific, general templates sufficient

---

## Inspiration Sources

- **Original Conary** (rPath) - Troves, changesets, flavors, components, labels, groups
- **Aeryn OS / Serpent OS** - Atomic updates, triggers, state snapshots, typed deps, container isolation

---

## Version History

| Version | Major Features |
|---------|----------------|
| v1-v5 | Core trove/changeset model, CAS, basic operations |
| v6-v8 | Repository system, delta updates, dependency resolution |
| v9-v10 | Scriptlet support, system adoption, GPG verification |
| v11 | Component model with classification and dependencies |
| v12 | Install reason tracking (explicit vs dependency) |
| v13 | Collections/groups support |

---

## Contributing

Contributions welcome. Priority areas:
1. Trigger system implementation
2. System state snapshots
3. Typed dependencies
4. Enhanced flavor support
5. Package pinning
6. Parallel downloads

See README.md for development setup and CLAUDE.md for coding conventions.
