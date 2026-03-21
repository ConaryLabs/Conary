---
last_updated: 2026-03-21
revision: 5
summary: Update schema version to v56, add derivation verification and provenance
---

# Conary Architecture

This document describes the internal architecture of Conary, a modern system
manager written in Rust. It covers the major subsystems, their interactions,
and the data flow for core operations.

## System Overview

```
                              CLI (src/cli/, src/main.rs)
                                        |
                    +-------------------+-------------------+
                    |                   |                   |
              Commands            Daemon Client        Cook Command
          (src/commands/)       (src/daemon/client)  (src/commands/cook.rs)
                    |                   |                   |
  +--------+-------+------+-----------+  +--+------+     +-----+-----+
  |        |       |      |           |  | conaryd |     |  Kitchen   |
  |        |       |      |           |  | daemon  |     |(src/recipe)|
  |        |       |      |           |  +---------+     +-----------+
  |        |       |      |           |
Install  Query   Model  Generation  Bootstrap
Remove   Search  Apply  Build       Stage0/1
Update   SBOM    Diff   Switch      Base/Image
  |        |       |      |           |
  +--------+-------+------+-----------+
           |                     |
    +------+------+        +-----+------+
     | Transaction |        |  Resolver  |
     |   Engine    |        | (src/      |
     | (src/       |        |  resolver/)|
     |  transaction|        +------------+
     |  /)         |               |
     +------+------+        +-----+------+
            |               | Repository |
     +------+------+        | (src/      |
     |  Database   |        |  repository|
     | (src/db/)   |        |  /)        |
     |  SQLite v56 |        +------+-----+
     +------+------+               |
            |               +------+------+
     +------+------+        | Remi Server |
     | Filesystem  |        | (--features |
     | CAS + VFS   |        |  server)    |
     | (src/       |        +-------------+
     |  filesystem)|
     +-------------+
```

## Core Concepts

### Trove

The fundamental unit. A trove represents a package, component, or collection.
Each trove has a name, version (epoch:version-release), and optional flavor.
Troves are stored in the `troves` table with install reason tracking
(explicit vs dependency) and optional label provenance.

### Changeset

An atomic transaction record. Every install, remove, or update creates a
changeset entry. Changesets enable full rollback of both database and
filesystem state. Each changeset carries a UUID for crash recovery
correlation.

### Flavor

Build-time variations expressed as `[ssl, !debug, is: x86_64]`. Flavors use
three operators: `~` (prefers), `!` (not), `~!` (prefers not). Architecture
flavors (`is: x86_64`) constrain package selection to compatible platforms.

### Label

Package provenance in `repository@namespace:tag` format. Labels form a
searchable path with priority ordering and support delegation chains to
other labels or repositories.

### Derived Package

A package created by modifying an existing package (the parent) with
patches and file overrides. Derived packages track their parent and
can be flagged as stale when the parent updates.

## Module Map

The project is a Cargo workspace with 5 crates:

```
conary/                  Root crate -- CLI binary
+-- src/
    +-- main.rs          Entry point, CLI dispatch
    +-- cli/             Clap command definitions
    +-- commands/        Command implementations
        +-- install/     Install pipeline (resolve, prepare, execute)
        +-- model.rs     System model operations
        +-- trust.rs     TUF trust management
        +-- cook.rs      Recipe cooking
        +-- derived.rs   Derived package creation
        +-- adopt/       System adoption

conary-core/             Core library crate
+-- src/
    +-- lib.rs           Public API surface
    +-- db/              Database layer
    |   +-- schema.rs    Schema v56, migration dispatcher
    |   +-- migrations.rs All 52 migration functions
    |   +-- models/      ORM-style model structs
    +-- transaction/     Composefs-native transaction engine
    |   +-- mod.rs       TransactionEngine, state machine (resolve/fetch/commit/build/mount)
    |   +-- planner.rs   VFS preflight conflict detection
    +-- generation/      EROFS generation building and composefs mounting
    |   +-- builder.rs   Build EROFS images from DB state (uses composefs-rs)
    |   +-- mount.rs     composefs mount/unmount, current symlink
    |   +-- metadata.rs  Generation metadata (JSON)
    |   +-- composefs.rs composefs detection and feature probing
    |   +-- gc.rs        Old generation garbage collection
    |   +-- etc_merge.rs Three-way /etc merge across generations
    |   +-- delta.rs     EROFS image delta computation
    |   +-- composefs_rs_eval.rs composefs-rs evaluation (feature-gated)
    +-- resolver/        Dependency resolution
    |   +-- graph.rs     Directed dependency graph
    |   +-- engine.rs    Resolution algorithm
    |   +-- sat.rs       SAT-based conflict resolution
    |   +-- plan.rs      Resolution plan output
    +-- repository/      Remote package sources
    |   +-- metadata.rs  Index parsing (RPM repodata, DEB Packages, Arch DB)
    |   +-- remi.rs      Remi client (CCS chunk fetcher)
    |   +-- chunk_fetcher.rs ChunkFetcher trait + HTTP/local/composite impls
    |   +-- mirror_health.rs Mirror health scoring
    |   +-- mirror_selector.rs Ranked mirror selection
    |   +-- metalink.rs  Metalink XML parser
    |   +-- substituter.rs Content substituter chain
    |   +-- resolution.rs Per-package routing strategies
    |   +-- dependency_model.rs Cross-distro dependency model (provides/requires/groups)
    |   +-- versioning.rs Cross-distro version scheme awareness
    |   +-- resolution_policy.rs Per-repo capability resolution policies
    +-- filesystem/      Storage layer
    |   +-- cas.rs       Content-addressable store (SHA-256 keyed)
    |   +-- vfs/         Virtual filesystem tree (arena allocator)
    |   +-- fsverity.rs  fs-verity content verification
    |   +-- path.rs      safe_join, sanitize_filename, sanitize_path
    +-- packages/        Format parsers
    |   +-- rpm.rs       RPM parser
    |   +-- deb.rs       DEB parser
    |   +-- arch.rs      Arch parser
    |   +-- common.rs    Unified PackageMetadata
    +-- ccs/             Native package format
    |   +-- builder.rs   CCS package builder
    |   +-- manifest.rs  CBOR manifest with Merkle tree
    |   +-- signing.rs   Ed25519 signing
    |   +-- lockfile.rs  ccs.lock dependency pinning
    |   +-- convert/     Legacy-to-CCS conversion
    |   +-- enhancement/ Retroactive CCS hook application
    |   +-- export/      OCI image export
    |   +-- hooks/       systemd, tmpfiles, sysctl, user/group, alternatives
    |   +-- policy.rs    Build policy engine
    +-- model/           Declarative system state
    |   +-- parser.rs    TOML model file parser
    |   +-- diff.rs      Current vs desired state diff
    |   +-- remote.rs    Remote collection fetching
    |   +-- lockfile.rs  Model lockfile for remote includes
    |   +-- signing.rs   Ed25519 collection signing
    |   +-- replatform.rs Cross-distro system replatforming
    +-- recipe/          Source-based package building
    |   +-- kitchen/     Build environment (cook, fetch, provenance)
    |   +-- parser.rs    TOML recipe parser
    |   +-- graph.rs     Multi-recipe build ordering
    |   +-- cache.rs     Build artifact cache
    |   +-- pkgbuild.rs  Arch PKGBUILD converter
    +-- trust/           TUF supply chain trust
    |   +-- client.rs    TUF metadata fetch and verification
    |   +-- metadata.rs  TUF metadata types (root, timestamp, snapshot, targets)
    |   +-- ceremony.rs  Root key ceremony
    |   +-- verify.rs    Signature verification
    +-- capability/      Package capability system
    |   +-- declaration.rs Capability declarations (network, fs, syscalls)
    |   +-- enforcement/ Landlock (filesystem) + seccomp-BPF (syscalls)
    |   +-- inference/   Heuristic capability detection
    |   +-- resolver.rs  Capability-aware dependency resolution
    +-- provenance/      Package DNA tracking
    |   +-- source.rs    Source provenance (URL, VCS, checksums)
    |   +-- build.rs     Build provenance (compiler, flags, env)
    |   +-- signature.rs Signature provenance
    |   +-- content.rs   Content integrity
    |   +-- slsa.rs      SLSA attestation generation
    +-- bootstrap/       System bootstrap from scratch
    +-- automation/      Automated maintenance (security, orphans)
    +-- container/       Namespace isolation for scriptlets
    +-- trigger/         Post-install trigger system
    +-- components/      File-to-component classification
    +-- compression/     Unified decompression (gzip, xz, zstd)
    +-- delta/           Binary delta generation and application
    +-- version/         Version parsing and comparison
    +-- hash.rs          Multi-algorithm hashing (SHA-256, XXH128)

conary-test/             Declarative test infrastructure (TOML manifests, container management)
+-- src/
    +-- config/          TOML manifest and distro config parsing
    +-- engine/          Test suite, runner, assertions
    +-- container/       ContainerBackend trait, bollard implementation
    +-- report/          JSON output, SSE event streaming
    +-- server/          Axum HTTP API, MCP server (rmcp)
    +-- cli.rs           Binary entrypoint

conary-server/           Remi server + conaryd (feature-gated: --features server)
+-- src/
    +-- server/          Remi server
    |   +-- routes.rs    Public + admin Axum routers
    |   +-- handlers/    HTTP handlers (chunks, packages, OCI, TUF, etc.)
    |   +-- conversion.rs On-demand legacy-to-CCS conversion
    |   +-- r2.rs        Cloudflare R2 storage backend
    |   +-- lite.rs      Remi Lite LAN proxy
    |   +-- analytics.rs Download event recording
    |   +-- bloom.rs     Bloom filter for chunk existence
    |   +-- security.rs  Rate limiting and IP banning
    |   +-- federated_index.rs Merged sparse index from upstream peers
    |   +-- delta_manifests.rs Pre-computed version deltas
    |   +-- prewarm.rs   Background package pre-conversion
    +-- daemon/          conaryd local daemon
    |   +-- routes.rs    REST API endpoints
    |   +-- jobs.rs      Priority job queue with SQLite persistence
    |   +-- client.rs    CLI forwarding client with SSE
    |   +-- socket.rs    Unix socket + TCP listener
    |   +-- auth.rs      SO_PEERCRED peer authentication
    |   +-- systemd.rs   Socket activation and watchdog
    +-- federation/      CAS peer-to-peer distribution
    |   +-- peer.rs      Peer registry and scoring
    |   +-- router.rs    Hierarchical chunk routing
    |   +-- manifest.rs  Signed chunk manifests
    |   +-- circuit.rs   Circuit breaker for failing peers
    |   +-- coalesce.rs  Request deduplication
    |   +-- mdns.rs      LAN peer discovery
    +-- bin/
        +-- remi.rs      Remi server binary entry point
        +-- conaryd.rs   conaryd daemon binary entry point
```

## Data Flow: Package Installation

This is the primary operation. The flow from `conary install nginx`:

```
1. RESOLVE
   +-- Parse package specifier (name, version constraint, repo)
   +-- Check per-package routing strategy (binary, remi, recipe, delegate)
   +-- Query repositories or Remi server for package metadata
   +-- Resolve transitive dependencies via dependency graph
   +-- Check for conflicts, pinned packages, redirects

2. PREPARE
   +-- Download package(s) - parallel via rayon if multiple
   |   +-- For Remi: fetch CCS chunks, assemble package
   |   +-- For legacy: download RPM/DEB/Arch package file
   +-- Parse package metadata into unified PackageMetadata
   +-- Detect package format (magic bytes or extension)
   +-- Optional: convert legacy format to CCS on-the-fly

3. TRANSACTION (composefs-native)
   +-- Create TransactionEngine, acquire lock
   +-- PLAN: VFS preflight - detect file conflicts
   +-- FETCH: Store package content in CAS
   +-- DB_COMMIT: Record trove, files, components, dependencies in SQLite
   |   (Point of no return)
   +-- BUILD: Construct EROFS image from DB state (composefs-rs)
   +-- MOUNT: Mount new generation via composefs, update /conary/current symlink
   +-- POST_SCRIPTS: Run post-install scriptlets (sandboxed against composefs mount)
   +-- TRIGGERS: Fire matching triggers (ldconfig, mime, icons, etc.)

4. RECOVERY (on crash)
   +-- Check /conary/current symlink for valid EROFS image
   +-- If invalid: rebuild EROFS from DB state and remount
   +-- If DB corrupted: scan generations/ for latest intact image
```

## Data Flow: Remi Server Request

When a client requests a package from the Remi server:

```
Client                        Remi Server
  |                               |
  |  GET /v1/fedora43/            |
  |      packages/nginx --------->|
  |                               |-- Check conversion cache
  |                               |   (converted_packages table)
  |                               |
  |  200 OK (chunks, version) <---|  [if cached]
  |                               |
  |  202 Accepted + job_id <------|  [if not cached]
  |                               |-- Fetch upstream RPM
  |  GET /v1/jobs/:id ----------->|-- Parse + convert to CCS
  |  200 {status: "converting"}<--|-- Store chunks in CAS
  |  ...polling...                |-- Record in conversion DB
  |  200 {status: "complete"} <---|
  |                               |
  |  GET /v1/chunks/:hash ------->|-- Bloom filter check
  |  200 <chunk bytes> <----------|-- Read from local CAS
  |                               |   or redirect to R2 presigned URL
  |  (repeat for each chunk)      |
```

## System Generations

Conary can manage the entire system filesystem as immutable, atomic
generations using EROFS images and Linux composefs.

### Architecture

```
Current System State
       |
  conary generation build
       |
  +----+----+
  | Snapshot |-- Capture all installed troves from SQLite
  +----+----+
       |
  +----+----+
  |  EROFS   |-- composefs-rs builds read-only filesystem image
  | Builder  |-- chunk-based external CAS references
  +----+----+
       |
  +----+----+
  | composefs|-- Linux 6.2+ overlay with fs-verity content verification
  | Mount    |-- CAS objects referenced by content hash
  +----+----+
       |
  Generation N (immutable, verified)
```

### Generation Lifecycle

1. **Build**: Snapshot current troves, construct EROFS image from CAS
2. **Store**: Save generation metadata (number, timestamp, summary, trove list)
3. **Switch**: Mount new generation via composefs, update boot entries
4. **Rollback**: Switch back to any previous generation
5. **GC**: Remove old generations, keeping N most recent

### Generation Module (conary-core/src/generation/)

The primary builder for composefs generations. Uses the composefs-rs crate
(v0.3.0) to produce EROFS images from the current DB state. Submodules:
builder.rs (EROFS image construction), mount.rs (composefs mount/unmount),
metadata.rs (JSON metadata), gc.rs (old generation cleanup), etc_merge.rs
(three-way /etc merge), delta.rs (EROFS image deltas), composefs.rs
(runtime feature detection).

### composefs Integration

The composefs driver (Linux 6.2+, `CONFIG_EROFS_FS`) provides:
- Content-verified overlays using fs-verity
- Efficient sharing of identical files across generations via CAS
- Atomic generation switching without unmounting

## Bootstrap Pipeline

Build a complete Conary-managed system from scratch. The pipeline has
up to 6 stages, with Stage 2 and Conary being optional:

```
Stage 0: Cross-Compiler (crosstool-ng 1.28.0)
  Build minimal cross-compiler for target arch
       |
Stage 1: Self-Hosted Toolchain (recipes/stage1/*.toml)
  5 packages: linux-headers, binutils, gcc-pass1, glibc, gcc-pass2
  Built with Stage 0 cross-compiler, sandboxed via ContainerConfig
       |
Stage 2: Reproducibility Rebuild (optional, --skip-stage2 to skip)
  Rebuilds Stage 1 packages using the Stage 1 compiler (not cross)
  Ensures bit-reproducible toolchain independent of host
       |
Base System (recipes/base/*.toml)
  ~80 packages resolved via RecipeGraph (topological sort)
  Boot/networking packages are tagged recipes, not hardcoded order
  Per-package checkpointing for resume after interruption
       |
Conary (optional, --skip-conary to skip) (recipes/conary/*.toml)
  Rust bootstrap + Conary self-hosting in the new sysroot
       |
Image Generation
  systemd-repart for rootless image generation (fallback: sfdisk/mkfs)
  UKI support via ukify. Output formats: raw, qcow2, ISO
```

Aligned with LFS 12.4 (binutils 2.45, gcc 15.2.0, glibc 2.42,
kernel 6.16.1). All recipes carry SHA-256 checksums enforced at
build time (`--skip-verify` to override). All stages run in
sandboxed containers via `ContainerConfig::pristine_for_bootstrap()`.

Supports x86_64, aarch64, and riscv64 targets. Dry-run mode
(`--dry-run`) validates the full pipeline without building.

## Database Schema (v56)

All state lives in SQLite. No config files for runtime state. Key tables:

```
Core:
  troves              Installed packages (name, version, flavor, label, pin, reason)
  changesets          Transaction history (install/remove/update, rollback data)
  files               File entries per trove (path, hash, perms, component)
  dependencies        Package dependencies with typed kinds
  provides            Package capability declarations

Components:
  components          Component entries (:runtime, :lib, :devel, :doc, etc.)
  component_dependencies / component_provides

Repository:
  repositories        Configured repos (URL, priority, TUF, default strategy)
  repository_packages Available packages from synced metadata
  repository_provides Cross-distro capability provides (kind, capability, version)
  repository_requirements Cross-distro capability requirements (kind, capability, version_constraint)
  repository_requirement_groups OR-alternative requirement groups
  labels / label_path Package provenance and search order
  mirror_health       Per-mirror latency/throughput/health scores

Security:
  tuf_roots / tuf_keys / tuf_metadata / tuf_targets   TUF trust chain
  capabilities / capability_audits                     Capability enforcement

Provenance:
  provenance_sources / builds / signatures / content / verifications

State:
  system_states / state_members    System state snapshots
  config_files / config_backups    Configuration tracking
  triggers / trigger_dependencies  Post-install trigger DAG
  settings                         Key-value configuration store

Server (Remi):
  converted_packages / subpackage_relationships   Conversion tracking
  chunk_access                                    LRU cache tracking
  download_stats / download_counts                Analytics
  delta_manifests                                 Pre-computed version deltas

Admin API:
  admin_tokens                                    Bearer token auth (name, hash, scopes)
  admin_audit_log                                 Request audit trail (action, IP, timing)
  remote_collections                              Cached remote model includes

Federation / Daemon:
  federation_peers / federation_stats   CAS federation
  daemon_jobs                           conaryd job queue
```

## Feature Gates

The codebase uses Cargo feature flags to keep the client binary lean:

| Feature  | Effect                                       | Binaries           |
|----------|----------------------------------------------|--------------------|
| (none)   | Client CLI only                              | `conary`           |
| server   | Links `conary-server` crate                  | `conary` + `remi` + `conaryd` |
| polkit   | PolicyKit auth in conaryd (requires `server`) | `conaryd`          |

Build examples:
- `cargo build` -- client only
- `cargo build --features server` -- with Remi server + conaryd daemon

## Key Design Decisions

**Database-first**: Every piece of state lives in SQLite. No TOML/YAML/JSON
config files for runtime state. The database is the single source of truth,
queryable with standard SQL tools.

**Composefs-native transactions**: The transaction engine follows a linear
pipeline: resolve -> fetch -> DB commit -> EROFS build -> mount. The DB commit
is the point of no return. Recovery is simple: if the DB says generation N
should be active but the mount does not match, rebuild the EROFS image from DB
state and remount. No journal, no backup phase, no staging directory.

**Content-addressable storage**: Files are stored by SHA-256 hash in a flat
CAS directory. This enables deduplication across packages, instant rollback
by preserving old content, and integrity verification.

**Chunk-level distribution**: Packages are split into variable-size chunks
via FastCDC. Clients only download chunks they don't already have, giving
implicit delta compression without pre-computing version-to-version diffs.

**Unified format pipeline**: All package formats (RPM, DEB, Arch) are parsed
into a common `PackageMetadata` struct. Conversion to the native CCS format
happens transparently, either on the client or via the Remi server.

**Namespace isolation**: Scriptlets run in Linux containers (mount, PID, IPC,
UTS namespaces) with resource limits. Capability enforcement uses Landlock
for filesystem access control and seccomp-BPF for syscall filtering.

## Security Architecture

```
Trust Chain:
  TUF Root --> Timestamp --> Snapshot --> Targets --> Package Hashes
                                                         |
  Package arrives --> Merkle tree verification --> Chunk integrity
                                                         |
  Scriptlet execution --> Namespace isolation --> Capability enforcement
                              |                       |
                         chroot + bind mounts    Landlock + seccomp-BPF
```

- **Repository trust**: TUF (The Update Framework) with threshold signatures,
  key rotation, and expiry enforcement
- **Package integrity**: CCS packages carry CBOR manifests with Merkle trees
  and Ed25519 signatures
- **Runtime isolation**: Scriptlets execute in namespaced containers with
  resource limits and filesystem/syscall restrictions
- **Provenance**: Full DNA tracking from source URL through build environment
  to deployed content, with optional SLSA attestations

## Related Documentation

- [docs/conaryopedia-v2.md](/docs/conaryopedia-v2.md) - Comprehensive technical guide
- [ROADMAP.md](/ROADMAP.md) - Forward-looking development roadmap
- [docs/SCRIPTLET_SECURITY.md](/docs/SCRIPTLET_SECURITY.md) - Scriptlet isolation details
