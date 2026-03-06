# Conary Roadmap

This document tracks the implementation status of Conary features, both completed and planned.

## Completed Features

### Core Architecture

- [COMPLETE] **Trove Model** - Core unit for packages, components, and collections
- [COMPLETE] **Changeset System** - Atomic transactions for all operations
- [COMPLETE] **SQLite Backend** - All state in queryable database (schema v44)
- [COMPLETE] **Content-Addressable Storage** - Git-style file deduplication
- [COMPLETE] **File-Level Tracking** - SHA-256 hashes, ownership, permissions for all files
- [COMPLETE] **Schema Migrations** - Automatic database evolution (v1-v44)

### Package Formats

- [COMPLETE] **RPM Support** - Full parsing including scriptlets, dependencies, rich metadata
- [COMPLETE] **DEB Support** - Debian/Ubuntu package format
- [COMPLETE] **Arch Support** - pkg.tar.zst and pkg.tar.xz formats
- [COMPLETE] **CCS Support** - Native Conary package format with CBOR manifest, Merkle tree, Ed25519 signatures
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
- [COMPLETE] **Orphan Detection** - Find dependencies no longer needed (with `orphan_since` grace period tracking, schema v39)
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
- [COMPLETE] **Reference Mirrors** - Split metadata (trusted) from content (CDN) sources via `--content-url`
- [COMPLETE] **Mirror Health Tracking** - Per-mirror latency, throughput, failure tracking, composite health scores (`mirror_health` table, schema v44)
- [COMPLETE] **Mirror Selector** - Ranked mirror selection using health scores and geographic hints
- [COMPLETE] **Metalink Parser** - Parse metalink files for mirror list with priority and geographic preference
- [COMPLETE] **Substituter Chain** - Composable content substituters for transparent package resolution

### System Model (Declarative OS)

Inspired by original Conary's CML (Conary Model Language), declare desired system state in TOML.

- [COMPLETE] **Model File Format** - TOML schema with install/exclude/pin/search/optional sections
- [COMPLETE] **State Capture** - Snapshot current system to model file (`model-snapshot`)
- [COMPLETE] **State Diff** - Compare model against current state (`model-diff`)
- [COMPLETE] **Drift Detection** - CI/CD-friendly check with exit codes (`model-check`)
- [COMPLETE] **State Sync** - Apply model to reach desired state (`model-apply`)
- [COMPLETE] **Version Pinning** - Pin packages to version patterns in model
- [COMPLETE] **Remote Include Resolution** - Fetch collections from Remi endpoints for `[include]` directives (schema v41)
- [COMPLETE] **Model Lockfile** - Pin remote include content hashes to prevent silent upstream drift
- [COMPLETE] **Model Signing** - Ed25519 signatures for published collections with verification
- [COMPLETE] **Model Publishing** - Publish collections to Remi server (`model-publish`)

### Delta Updates

- [COMPLETE] **Binary Deltas** - zstd dictionary compression
- [COMPLETE] **Delta Generation** - Create deltas between versions
- [COMPLETE] **Delta Application** - Apply deltas to upgrade packages
- [COMPLETE] **Automatic Fallback** - Fall back to full download if delta fails
- [COMPLETE] **Bandwidth Statistics** - Track bytes saved across updates
- [COMPLETE] **Delta Manifests** - Pre-computed chunk set differences between package versions for efficient upgrades (schema v44)

### Security

- [COMPLETE] **GPG Key Import** - Import trusted public keys
- [COMPLETE] **Key Management** - List, remove imported keys
- [COMPLETE] **Signature Verification** - Verify package signatures
- [COMPLETE] **Strict Mode** - Require valid signatures for all packages

### TUF Supply Chain Trust

The Update Framework (TUF) for repository metadata verification.

- [COMPLETE] **TUF Metadata** - Root, timestamp, snapshot, targets roles (schema v43)
- [COMPLETE] **Trust Client** - Fetch and verify TUF metadata with threshold signatures
- [COMPLETE] **Key Management** - TUF key generation, storage, and rotation support
- [COMPLETE] **Root Ceremony** - Generate and sign root metadata with key thresholds
- [COMPLETE] **Target Verification** - Verify package hashes against signed targets metadata
- [COMPLETE] **CLI Commands** - trust init, trust status, trust verify, trust key-gen
- [COMPLETE] **Server Integration** - Remi serves TUF metadata and supports timestamp refresh

### Capability Enforcement

Declare and enforce package capabilities (network, filesystem, syscall access).

- [COMPLETE] **Capability Declarations** - Packages declare required capabilities (network, filesystem, syscalls; schema v33)
- [COMPLETE] **Capability Audit** - Audit installed packages against declared capabilities
- [COMPLETE] **Capability Inference** - Heuristic-based capability detection from package contents
- [COMPLETE] **Capability Resolver** - Resolve capability requirements during dependency solving
- [COMPLETE] **Landlock Enforcement** - Filesystem access control via Linux Landlock LSM
- [COMPLETE] **Seccomp-BPF Enforcement** - Syscall filtering via seccomp-BPF

### Package Provenance (DNA)

Full provenance tracking from source to deployment.

- [COMPLETE] **Source Provenance** - Track source URLs, VCS info, checksums
- [COMPLETE] **Build Provenance** - Record compiler, flags, environment, timestamps
- [COMPLETE] **Signature Provenance** - Track signing keys, timestamps, verification status
- [COMPLETE] **Content Provenance** - File-level content hashes and integrity verification
- [COMPLETE] **SLSA Integration** - Generate SLSA provenance attestations
- [COMPLETE] **Database Storage** - Provenance tables (sources, builds, signatures, content, verifications; schema v32)

### System Operations

- [COMPLETE] **Full Rollback** - Reverse database AND filesystem changes
- [COMPLETE] **File Restore** - Restore modified/deleted files from CAS
- [COMPLETE] **Integrity Verification** - Verify installed files against hashes
- [COMPLETE] **Conflict Detection** - Detect file conflicts between packages
- [COMPLETE] **History Tracking** - Complete audit log of all operations

### System Generations

- [COMPLETE] **EROFS Image Builder** - conary-erofs crate builds immutable filesystem images (LZ4/LZMA)
- [COMPLETE] **composefs Integration** - Linux 6.2+ overlay with fs-verity verification
- [COMPLETE] **Generation Build** - Snapshot current state into numbered EROFS generation
- [COMPLETE] **Generation Switch** - Live-switch to any generation without reboot
- [COMPLETE] **Generation Rollback** - Switch back to previous generation
- [COMPLETE] **Generation GC** - Remove old generations (configurable keep count)
- [COMPLETE] **Generation Info** - Show detailed metadata for any generation
- [COMPLETE] **System Takeover** - Adopt entire existing system into generation management
- [COMPLETE] **CLI Commands** - generation list, build, switch, rollback, gc, info; system takeover

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

### CCS Package Building

Native package format (CCS - Conary Component Specification) with build policies and container export.

- [COMPLETE] **CCS Package Format** - Gzipped tar with CBOR manifest and Merkle tree verification
- [COMPLETE] **Ed25519 Signatures** - Package signing and verification
- [COMPLETE] **Build Policies** - Trait-based policy engine (DenyPaths, NormalizeTimestamps, StripBinaries, FixShebangs, CompressManpages)
- [COMPLETE] **SOURCE_DATE_EPOCH** - Reproducible build support with timestamp normalization
- [COMPLETE] **OCI Export** - Export packages to OCI container images (podman/docker compatible)
- [COMPLETE] **CLI Commands** - ccs-init, ccs-build, ccs-inspect, ccs-verify, ccs-sign, ccs-keygen, ccs-install, ccs-export
- [COMPLETE] **Lockfiles** - `ccs.lock` pinning exact versions, content hashes, and source URLs of all transitive dependencies
- [COMPLETE] **Retroactive Enhancement** - Apply CCS hooks (systemd, tmpfiles, sysctl, user/group, alternatives) to converted legacy packages (schema v36-v37)

### Container-Isolated Scriptlets

Run package scripts in lightweight Linux containers for safety.

- [COMPLETE] **Namespace Isolation** - Mount, PID, IPC, UTS namespaces for scriptlets
- [COMPLETE] **Chroot Isolation** - Isolate scriptlet filesystem from host via chroot
- [COMPLETE] **Bind Mounts** - Controlled access to required host paths (read-only by default)
- [COMPLETE] **Rootless Fallback** - Falls back to resource-limited execution when not root
- [COMPLETE] **Resource Limits** - CPU, memory, file size, process limits for scriptlets
- [COMPLETE] **Dangerous Script Detection** - Automatic risk analysis with pattern matching
- [COMPLETE] **CLI Integration** - `--sandbox` flag (auto, always, never) for install/remove commands

### Developer Experience (Inspired by Nix)

Workflow features that make Nix beloved by developers.

- [COMPLETE] **Dev Shells** - `ccs shell <packages>` for temporary environments without permanent install
- [COMPLETE] **Lockfiles** - `ccs.lock` pinning exact versions and hashes of all transitive dependencies
- [COMPLETE] **One-Shot Run** - `ccs run <package> -- <command>` to execute without installing
- [ ] **Shell Integration** - Automatic environment activation when entering project directories

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
- [COMPLETE] **Label Federation** - Labels can delegate to other labels or link to repositories (schema v30)
- [COMPLETE] **Delegation Chains** - Resolve packages through label chains with cycle detection
- [COMPLETE] **CLI Commands** - label-list, label-add, label-remove, label-path, label-show, label-set, label-query, label-link, label-delegate

### Enhanced Queries

- [COMPLETE] **repquery** - Query available packages in repositories (not just installed)
- [COMPLETE] **Path Query** - `conary query --path /usr/bin/foo` - find package by file
- [COMPLETE] **Info Query** - Detailed package information with `--info` flag
- [COMPLETE] **File Listing** - `--lsl` for ls -l style file listing, `--files` for simple listing
- [COMPLETE] **SBOM Export** - Generate CycloneDX 1.5 Software Bill of Materials (`query sbom`)

### Storage Management

- [COMPLETE] **CAS Garbage Collection** - Remove unreferenced objects from content store (`system gc`)

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

## Repository 2026: Chunk-Level Distribution

Move from file-level to chunk-level for massive efficiency gains. CDC gives "delta compression for free" - no need to pre-compute version-to-version deltas.

**Phase 1: Content-Defined Chunking (CDC)** [COMPLETE]
- [x] **FastCDC Chunking** - Variable-size chunks based on content (16KB min, 64KB avg, 256KB max)
- [x] **Chunk-Level CAS** - Store chunks instead of files, cross-package deduplication
- [x] **Implicit Deltas** - Client has 48/50 chunks, downloads only 2 missing
- [x] **`--chunked` Flag** - Build CDC-enabled packages with `ccs-build --chunked`
- [x] **Chunk Statistics** - Build summary shows chunked files, dedup savings

**Phase 2: Remi (On-Demand Conversion Proxy)** [COMPLETE]
- [x] **Server Module** - Feature-gated (`--features server`) Axum HTTP server
- [x] **On-Demand Conversion** - Convert legacy packages (RPM/DEB/Arch) to CCS when requested
- [x] **202 Accepted Pattern** - Async conversion with job polling for long-running operations
- [x] **Chunk CAS Storage** - Store converted chunks in content-addressed storage
- [x] **Client Integration** - RemiClient with automatic polling and chunk assembly
- [x] **LRU Cache Design** - Evict old chunks to manage disk space (implementation pending)
- [x] **Deployed** - Running on packages.conary.io (Hetzner dedi, 12 cores, 64GB, 2x1TB NVMe RAID 0)

**Phase 3: HTTP Chunk Repository** [COMPLETE]
- [x] **ChunkFetcher Trait** - Transport abstraction (`fn fetch(hash) -> bytes`) with HTTP, local, and composite implementations
- [x] **HTTP/2 Client** - Parallel chunk fetching with configurable concurrency
- [x] **Sparse Index** - crates.io-style per-package JSON documents, CDN-cacheable
- [x] **R2 Storage Backend** - Cloudflare R2 object storage for CDN-backed chunk distribution with presigned URL redirects
- [x] **Bloom Filter Protection** - In-memory Bloom filter to reject invalid chunk hashes without disk I/O
- [x] **Negative Cache** - Cache conversion failures to avoid repeated attempts
- [x] **Batch Operations** - Batch chunk existence checks and multi-chunk fetch endpoints

**Phase 4: Production Hardening** [COMPLETE]
- [x] **CORS Restrictions** - Public vs restricted CORS layers for chunk/admin endpoints
- [x] **Token-Bucket Rate Limiting** - Per-IP rate limiting with configurable RPS and burst
- [x] **Audit Logging** - Middleware-based audit logging for federation and admin requests
- [x] **Ban List** - IP ban list enforcement for misbehaving clients
- [x] **Admin API Separation** - Localhost-only admin router for privileged operations (conversion, cache, recipes)
- [x] **Cloudflare Integration** - CF-Connecting-IP header extraction, IP range validation

**Phase 5: Observability** [COMPLETE]
- [x] **Prometheus Metrics** - Atomic counters for requests, hits, misses, errors; exposed at `/metrics`
- [x] **Download Analytics** - Buffered download event recording with periodic aggregation (schema v40)
- [x] **Package Statistics** - Popular packages, recent additions, overview endpoints (`/v1/stats/*`)
- [x] **Server Info** - Admin endpoint with runtime and storage details
- [x] **Popularity Tracking** - Download count aggregation (30-day, 7-day, total) for ranking

**Phase 6: Advanced Distribution** [COMPLETE]
- [x] **Delta Manifests** - Pre-computed chunk set differences between versions (`delta_manifests` table, schema v44)
- [x] **Smart Pre-warming** - Background conversion of popular packages before they are requested
- [x] **Federated Sparse Index** - Merge package metadata from upstream Remi instances with TTL caching
- [x] **OCI Distribution API** - Expose CCS packages as OCI artifacts (v2 spec: catalog, manifests, blobs, tags)
- [x] **Remi Lite Proxy** - Zero-config LAN proxy with pull-through caching and mDNS auto-discovery
- [x] **Web Package Index** - SvelteKit frontend for browsing packages, versions, dependencies, and stats

**P2P Plugin (Future)**
- [ ] **IPFS Fetcher Plugin** - Check local IPFS node before CDN fallback
- [ ] **BitTorrent DHT Plugin** - Peer discovery for popular chunks
- [ ] **Transport Priority** - P2P -> CDN -> Mirror fallback chain

Design principle: Don't embed P2P in core. Build clean fetch API, let plugins add P2P later. Enterprise blocks P2P anyway.

### CAS Federation

Distributed chunk sharing across Conary nodes for bandwidth savings.

- [COMPLETE] **Hierarchical Peer Model** - Region hub (WAN), cell hub (LAN), leaf (client) tiers
- [COMPLETE] **Peer Discovery** - mDNS (`_conary-cas._tcp.local`) and manual configuration
- [COMPLETE] **Chunk Router** - Hierarchical routing (cell -> region -> upstream) with preference ordering
- [COMPLETE] **Request Coalescing** - Deduplicate concurrent identical chunk requests
- [COMPLETE] **Circuit Breaker** - Automatic failover for unresponsive peers
- [COMPLETE] **Signed Manifests** - Ed25519-signed chunk manifests for integrity verification
- [COMPLETE] **Federation Statistics** - Per-peer success rates, latency tracking, daily stats (schema v34)
- [COMPLETE] **CLI Commands** - federation status, peers, add-peer, test, scan, stats
- [COMPLETE] **Server Directory** - Federation peer directory endpoint for discovery

### conaryd Daemon

Local daemon providing REST API for package operations, acting as the "Guardian of State" with exclusive transaction lock ownership.

- [COMPLETE] **REST API** - Unix socket primary (`/run/conary/conaryd.sock`) with optional TCP
- [COMPLETE] **Job Queue** - Priority-ordered operation queue with SQLite persistence (schema v35)
- [COMPLETE] **SSE Streaming** - Real-time progress events for transaction monitoring
- [COMPLETE] **CLI Forwarding** - CLI auto-detects daemon and forwards operations when available
- [COMPLETE] **Peer Authentication** - SO_PEERCRED for Unix socket identity verification
- [COMPLETE] **System Lock** - System-wide flock for exclusive transaction access
- [COMPLETE] **Systemd Integration** - Socket activation, watchdog, and idle timeout support
- [COMPLETE] **Audit Logging** - Per-operation audit trail with peer credentials

### Recipe System (Cooking)

Building packages from source using recipe files, following original Conary's culinary metaphors.

- [COMPLETE] **Recipe Parser** - Parse TOML recipe files with package, source, build, and patches sections
- [COMPLETE] **Kitchen Abstraction** - Isolated build environment for cooking packages
- [COMPLETE] **Variable Substitution** - Support %(version)s, %(destdir)s, %(builddir)s, etc.
- [COMPLETE] **Recipe Validation** - Validate recipes with warnings for common issues
- [COMPLETE] **Cook Command** - `conary cook <recipe>` to build packages from source
- [COMPLETE] **Recipe Resolution Strategy** - Resolver can fetch and cook recipes automatically
- [COMPLETE] **Hermetic Builds** - Network-isolated builds with PID/UTS/IPC/mount/net namespaces
- [COMPLETE] **Build Cache** - Content-addressed artifact caching with invalidation on dependency changes
- [COMPLETE] **Recipe Graph** - Dependency graph for multi-recipe build ordering with cycle breaking for bootstrap
- [COMPLETE] **PKGBUILD Converter** - Convert Arch Linux PKGBUILD files to Conary recipe format
- [COMPLETE] **Provenance Capture** - Automatic build provenance recording during cooking
- [ ] **Source Components** - Store :source troves in repository
- [ ] **Factory System** - Templates for common package types

### Derived Packages

- [COMPLETE] **Derived Package Builder** - Create packages based on existing ones with modifications (overrides, patches)
- [COMPLETE] **Version Policy** - Configurable version derivation (match parent, custom, append suffix)
- [COMPLETE] **Database Tracking** - Track parent-child relationships in `derived_packages` table (schema v26)
- [COMPLETE] **CLI Commands** - derive command for creating derived packages

### Package Evolution

- [COMPLETE] **Redirect Packages** - Package redirects for renames/obsoletes with automatic resolution during install (schema v28)
- [COMPLETE] **Package Splits** - Track when packages split (`firefox` -> `firefox-bin` + `firefox-lib`) via split redirects
- [COMPLETE] **Obsoletes Handling** - Clean removal of deprecated packages during updates via obsolete redirects
- [COMPLETE] **Subpackage Relationships** - Track RPM/DEB subpackage structure (base, component type; schema v36)

### Bootstrap System

Build a complete Conary system from scratch. Pipeline: Stage 0 -> Stage 1 -> Stage 2 (optional) -> BaseSystem -> Conary (optional) -> Image. Aligned with LFS 12.4 (binutils 2.45, gcc 15.2.0, glibc 2.42, kernel 6.16.1).

- [COMPLETE] **Stage 0** - Minimal cross-compilation toolchain bootstrap
- [COMPLETE] **Stage 1** - Self-hosting build environment
- [COMPLETE] **Stage 2** - Extended toolchain (optional, skippable with `--skip-stage2`)
- [COMPLETE] **Toolchain Management** - Compiler and build tool versioning
- [COMPLETE] **Base System** - Core system packages with per-package checkpointing
- [COMPLETE] **Conary Stage** - Build Conary itself for self-hosting (optional, skippable with `--skip-conary`)
- [COMPLETE] **Image Generation** - Bootable disk images via systemd-repart (fallback to sfdisk/mkfs)
- [COMPLETE] **RecipeGraph Ordering** - Dependency-driven build ordering (not hardcoded)
- [COMPLETE] **Checksum Enforcement** - SHA-256 verification on all sources (`--skip-verify` to bypass)
- [COMPLETE] **Dry-Run Validation** - `conary bootstrap dry-run` validates pipeline without building
- [COMPLETE] **ContainerConfig Sandboxing** - All stages run in sandboxed environments
- [COMPLETE] **Recipe Files** - TOML recipes in `recipes/stage1/`, `recipes/base/`, `recipes/conary/`

### Automated Maintenance

AI-assisted and scheduled maintenance operations.

- [COMPLETE] **Security Update Checks** - Automated security vulnerability scanning
- [COMPLETE] **Orphan Cleanup** - Scheduled orphaned package removal with grace periods
- [COMPLETE] **Action Engine** - Pluggable action system for automated operations
- [COMPLETE] **Scheduler** - Configurable maintenance scheduling

### VFS Tree with Reparenting (Inspired by Aeryn OS)

Virtual filesystem tree for efficient file operations.

- [COMPLETE] **Arena Allocator** - Efficient node storage for large trees (`src/filesystem/vfs/mod.rs`)
- [COMPLETE] **O(1) Path Lookup** - HashMap for instant path-to-node resolution
- [COMPLETE] **Subtree Reparenting** - Efficiently move entire subtrees (reparent, reparent_with_rename)
- [ ] **Component Merging** - Merge component trees for installation

### Fast Hashing Option (Inspired by Aeryn OS)

Optional xxhash for non-cryptographic use cases.

- [COMPLETE] **XXH128 Support** - XXH3-128 implementation via `xxhash-rust` crate (`src/hash.rs`)
- [COMPLETE] **Hash Selection** - Configure hash algorithm per use case (SHA-256, Blake3, XXH128)
- [COMPLETE] **Dedup with XXH128** - Faster deduplication checks with non-cryptographic hashing
- [COMPLETE] **Verify with SHA-256** - SHA-256 retained for security verification

---

## Long-Term / Future Consideration

### Atomic Filesystem Updates (Inspired by Aeryn OS)

Implemented via system generations using EROFS + composefs.

- [COMPLETE] **EROFS Image Builder** - conary-erofs crate for building immutable filesystem images
- [COMPLETE] **composefs Integration** - Linux 6.2+ overlay with fs-verity content verification
- [COMPLETE] **Generation Build** - Snapshot system state into EROFS generation
- [COMPLETE] **Generation Switch** - Live-switch to any generation without reboot
- [COMPLETE] **Generation Rollback** - Switch back to previous generation
- [COMPLETE] **Generation GC** - Remove old generations
- [COMPLETE] **System Takeover** - Adopt entire existing system into generation management
- [ ] **renameat2 RENAME_EXCHANGE** - Atomic directory swap (alternative to composefs on older kernels)

### Repository Server (Full)

- [ ] **Conary Repository Service** - Network-accessible source repository (beyond Remi package proxy)
- [ ] **Version Control** - Repository as version control system
- [ ] **Commit/Checkout** - Check in/out package sources
- [ ] **Branch Management** - Create and manage branches

### OS Composition (Inspired by Foresight Linux)

- [ ] **Group Packages** - `type = "group"` packages containing only references to other packages
- [ ] **Nested Groups** - Groups can include other groups (`group-server` includes `group-base`)
- [ ] **Optional Members** - Groups can have required and optional members
- [ ] **Migrate Command** - Migrate system to new group version atomically

### Advanced Features

- [ ] **Info Packages** - Create system users/groups via packages
- [ ] **Capsule Packages** - Encapsulate foreign packages

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
- **Nix** - Dev shells, lockfiles, one-shot run, reproducible builds
- **TUF** - Supply chain trust framework for repository metadata
- **crates.io** - Sparse index design for package metadata

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
| v23 | Transaction engine crash recovery (tx_uuid column on changesets) |
| v24 | Reference mirrors (content_url on repositories), System Model (declarative OS state) |
| - | CCS Native Package Format (CBOR manifest, Merkle tree, Ed25519 signatures) |
| - | Build Policy System (trait-based policies, SOURCE_DATE_EPOCH, reproducible builds) |
| - | OCI Container Export (podman/docker compatible image generation) |
| v25 | Legacy to CCS conversion tracking (`converted_packages` table) |
| v26 | Derived packages for model-apply operations |
| v27 | Remi chunk access tracking for LRU cache |
| v28 | Package redirects (renames, obsoletes), SBOM export, CAS garbage collection, dev shells |
| v29 | Per-package routing (package_resolution table), unified resolution strategies |
| v30 | Label federation (repository_id, delegate_to_label_id), recipe cooking system |
| v31 | Repository default resolution strategy (binary, remi, recipe, delegate) |
| v32 | Package DNA / full provenance tracking (sources, builds, signatures, content, verifications) |
| v33 | Capability declarations and audits (network, filesystem, syscalls) |
| v34 | CAS federation peers and daily bandwidth statistics |
| v35 | conaryd daemon persistent job queue |
| v36 | Retroactive CCS enhancement framework, subpackage relationships |
| v37 | Enhancement priority scheduling |
| v38 | Server-side conversion tracking (package identity, chunk manifest, CCS path) |
| v39 | Orphan tracking with grace periods (orphan_since column on troves) |
| v40 | Download statistics for package popularity ranking |
| v41 | Remote collection cache for model include resolution |
| v42 | Ed25519 signatures for remote collections |
| v43 | TUF supply chain trust (root, keys, metadata, targets tables; repository TUF columns) |
| v44 | Mirror health tracking, delta manifests |

---

## Contributing

Contributions welcome. Priority areas:
1. Shell integration for dev shells (direnv-style)
2. P2P chunk distribution plugins
3. VFS component merging
4. Multi-version package support (kernels)
5. Web interface improvements

See README.md for development setup and CLAUDE.md for coding conventions.
