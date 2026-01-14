# Conary Roadmap

This document tracks the implementation status of Conary features, both completed and planned.

## Completed Features

### Core Architecture

- [COMPLETE] **Trove Model** - Core unit for packages, components, and collections
- [COMPLETE] **Changeset System** - Atomic transactions for all operations
- [COMPLETE] **SQLite Backend** - All state in queryable database (schema v22)
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

- [COMPLETE] **Flavor Parsing** - Parse flavor specifications like `[ssl, !debug, is: x86_64]`
- [COMPLETE] **Flavor Matching** - Select packages by flavor requirements
- [COMPLETE] **Flavor Operators** - Support `~` (prefers), `!` (not), `~!` (prefers not)
- [COMPLETE] **Architecture Flavors** - `is: x86`, `is: x86_64`, `is: aarch64`
- [COMPLETE] **Database Integration** - `flavor_spec` column on troves table (schema v14)

### Package Pinning

- [COMPLETE] **Pin Command** - Pin packages to prevent modification during updates (`conary pin`)
- [COMPLETE] **Unpin Command** - Allow pinned packages to be updated (`conary unpin`)
- [COMPLETE] **List Pinned** - List all pinned packages (`conary list-pinned`)
- [COMPLETE] **Update Protection** - Pinned packages are skipped during `conary update`
- [COMPLETE] **Remove Protection** - Pinned packages cannot be removed until unpinned
- [ ] **Multi-Version Support** - Keep multiple versions of pinned packages (like kernels)

### Parallel Operations

- [COMPLETE] **Parallel Downloads** - Download multiple packages concurrently (via rayon par_iter in `download_dependencies`)
- [COMPLETE] **Parallel Extraction** - Extract package contents in parallel (`extract_packages_parallel` in packages module)
- [COMPLETE] **Download Progress** - Show aggregate progress for parallel downloads (total bytes, package count, speed)
- [COMPLETE] **Parallel Repo Sync** - Sync multiple repositories concurrently

### Transitive Dependencies

- [COMPLETE] **Deep Resolution** - Recursively resolve all dependencies (via `resolve_dependencies_transitive`)
- [COMPLETE] **Dependency Tree** - Show full dependency tree visualization (`conary deptree` command)
- [COMPLETE] **Circular Detection** - Better handling of circular dependencies (marked as `[circular]` in tree)

### Selection Reasons (Inspired by Aeryn OS)

- [COMPLETE] **Reason Text Field** - Add human-readable reason to install tracking (`selection_reason` column, schema v16)
- [COMPLETE] **Dependency Chain** - Track "Required by X" for dependency installs
- [COMPLETE] **Collection Attribution** - Track "Installed via @collection" for collection installs
- [COMPLETE] **Query by Reason** - Filter packages by installation reason (`conary query-reason`)

---

## Medium-Term

### Trigger System (Inspired by Aeryn OS)

A general-purpose handler system for post-installation actions, more flexible than scriptlets.

- [COMPLETE] **Trigger Definition** - Path patterns mapped to handler scripts (schema v17, `triggers` table)
- [COMPLETE] **Handler Registry** - Register handlers for file types (10 built-in triggers: ldconfig, mime, icons, etc.)
- [COMPLETE] **DAG Ordering** - Triggers declare before/after dependencies (`trigger_dependencies` table)
- [COMPLETE] **Topological Execution** - Run triggers in dependency order (Kahn's algorithm)
- [COMPLETE] **Built-in Triggers** - ldconfig, update-mime-database, gtk-update-icon-cache, systemd-reload, fc-cache, depmod, etc.
- [COMPLETE] **CLI Commands** - trigger-list, trigger-show, trigger-enable, trigger-disable, trigger-add, trigger-remove, trigger-run

### System State Snapshots (Inspired by Aeryn OS)

Full system state tracking for cleaner rollback semantics.

- [COMPLETE] **State Table** - Store complete package sets as numbered states (`system_states` table, schema v18)
- [COMPLETE] **State Metadata** - ID, timestamp, summary, description for each state
- [COMPLETE] **State Members** - Package list per state (`state_members` table)
- [COMPLETE] **State Diff** - Compare two states to see what changed (`state-diff` command)
- [COMPLETE] **State Restore Plan** - Show operations needed to rollback to a previous state
- [COMPLETE] **State Pruning** - Garbage collect old states to save space (`state-prune` command)
- [COMPLETE] **Active State Tracking** - Track current system state ID
- [COMPLETE] **Automatic Snapshots** - States created automatically after install/remove operations
- [COMPLETE] **CLI Commands** - state-list, state-show, state-diff, state-restore, state-prune, state-create

### Typed Dependencies (Inspired by Aeryn OS)

Formalize dependency kinds with explicit type prefixes.

- [COMPLETE] **Dependency Kinds** - Package, Soname, Python, Perl, Ruby, Java, PkgConfig, CMake, Binary, File, Interpreter, Abi, KernelModule
- [COMPLETE] **Kind Format** - `kind(target)` syntax e.g., `pkgconfig(zlib)`, `python(flask)`
- [COMPLETE] **Kind Matching** - Resolve dependencies by matching kinds (schema v19 with `kind` column)
- [COMPLETE] **Provider Kinds** - Provides table has `kind` column for typed matching
- [COMPLETE] **Migration** - Automatic migration parses existing `kind(name)` strings into typed format
- [COMPLETE] **CLI Support** - `depends` and `rdepends` display typed dependencies

### Labels System

Inspired by original Conary's label concept for tracking package provenance.

- [COMPLETE] **Label Format** - `repository@namespace:tag` format (parsing, validation, wildcards)
- [COMPLETE] **Label Path** - Configure search order for labels (priority-based ordering)
- [COMPLETE] **Label Tracking** - Track which label a package came from (`label_id` on troves, schema v20)
- [COMPLETE] **Branch History** - Track parent labels via `parent_label_id` relationships
- [COMPLETE] **CLI Commands** - label-list, label-add, label-remove, label-path, label-show, label-set, label-query

### Enhanced Queries

- [COMPLETE] **repquery** - Query available packages in repositories (not just installed)
- [COMPLETE] **Path Query** - `conary query --path /usr/bin/foo` - find package by file
- [COMPLETE] **Info Query** - Detailed package information with `--info` flag
- [COMPLETE] **File Listing** - `--lsl` for ls -l style file listing, `--files` for simple listing

### Configuration Management

- [COMPLETE] **Config File Tracking** - Track which files are configuration (schema v21, `config_files` table)
- [COMPLETE] **Config Source Detection** - Detect config files from RPM %config, DEB conffiles, Arch backup
- [COMPLETE] **Config Backup** - Backup configs before modification (`config_backups` table, CAS storage)
- [COMPLETE] **Config Restore** - Restore configs from backup with pre-restore safety backup
- [COMPLETE] **Config Diff** - Show differences between installed and package configs
- [COMPLETE] **Config Status** - Track pristine/modified/missing status with automatic detection
- [COMPLETE] **Noreplace Support** - Honor %config(noreplace) to preserve user modifications
- [COMPLETE] **CLI Commands** - config-list, config-diff, config-backup, config-restore, config-check, config-backups

### Update Improvements

- [COMPLETE] **updateall** - Update all packages (`conary update` with no args updates all)
- [COMPLETE] **Security Updates** - `--security` flag for security updates only (schema v22)
- [COMPLETE] **Security Metadata** - Track severity, CVE IDs, advisory info on repository packages
- [COMPLETE] **Update Groups** - `conary update-group <name>` updates collection members atomically

---

## Long-Term / Future Consideration

### Container-Isolated Scriptlets (Inspired by Aeryn OS)

Run package scripts in lightweight Linux containers for safety.

- [COMPLETE] **Namespace Isolation** - Mount, PID, IPC, UTS namespaces for scriptlets
- [COMPLETE] **Chroot Isolation** - Isolate scriptlet filesystem from host via chroot
- [COMPLETE] **Bind Mounts** - Controlled access to required host paths (read-only by default)
- [COMPLETE] **Rootless Fallback** - Falls back to resource-limited execution when not root
- [COMPLETE] **Resource Limits** - CPU, memory, file size, process limits for scriptlets
- [COMPLETE] **Dangerous Script Detection** - Automatic risk analysis with pattern matching
- [COMPLETE] **CLI Integration** - `--sandbox` flag (auto, always, never) for install/remove commands

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
| v14 | Enhanced flavor support (parsing, matching, operators, architecture), transitive dependency tree |
| v15 | Package pinning support (`pinned` column on troves) |
| v16 | Selection reason tracking (`selection_reason` column), query-reason command |
| v17 | Trigger system for post-installation actions (ldconfig, mime, icons, systemd, etc.) |
| v18 | System state snapshots for full system state tracking and rollback |
| v19 | Typed dependencies with explicit kind prefixes (python, soname, pkgconfig, etc.) |
| v20 | Labels system for package provenance tracking (labels, label_path tables, label commands) |
| v21 | Configuration file management (config_files, config_backups tables, noreplace support) |
| v22 | Update improvements (security metadata on repository_packages, update-group command) |
| v23 | Container-isolated scriptlets (namespace isolation, resource limits, script analysis, --sandbox CLI) |

---

## Contributing

Contributions welcome. Priority areas:
1. Atomic filesystem updates
2. VFS tree with reparenting
3. Fast hashing option (xxhash)

See README.md for development setup and CLAUDE.md for coding conventions.
