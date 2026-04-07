---
last_updated: 2026-04-07
revision: 9
summary: Refresh workspace layout for shared operation vocabulary, canonical daemon defaults, and current service boundaries
---

# Conary Architecture

This document describes the internal architecture of Conary, a modern system
manager written in Rust. It covers the major subsystems, their interactions,
and the data flow for core operations.

## System Overview

```
apps/conary/ (CLI)
  cli/ + app.rs + dispatch.rs
      |
      +-- install / update / remove
      +-- repo / query / model / ccs / collection
      +-- system generation / state / takeover
      +-- bootstrap / provenance / capability / federation
      |
      v
crates/conary-core/
  repository --> resolver --> transaction --> generation / filesystem --> db
      |               |             |                |                  |
      |               |             |                +-- composefs/EROFS
      |               |             +-- CAS + SQLite commit lifecycle
      |               +-- SAT resolution + routing policy
      +-- remote metadata, Remi client, mirrors, substituters

Supporting workspace members
  apps/remi/         public/admin package service, search, federation, MCP
  apps/conaryd/      local daemon, auth, job queue, REST/SSE routes
  apps/conary-test/  integration harness, HTTP API, MCP, container runners
  crates/conary-mcp/ shared transport-agnostic MCP helpers
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

The project is a virtual Cargo workspace with 7 members:

```
apps/conary/             CLI binary
+-- src/
    +-- main.rs          Thin entrypoint
    +-- app.rs           Bootstrap and top-level app wiring
    +-- dispatch.rs      Command routing and live-host safety gates
    +-- cli/             Clap command definitions
    +-- commands/        Command implementations (install, repo, query, model, ccs, bootstrap, system)

crates/conary-core/      Core library crate
+-- src/
    +-- lib.rs           Public API surface
    +-- operations.rs    Shared operation vocabulary across CLI and daemon boundaries
    +-- db/              Database layer
    |   +-- schema.rs    Schema v65, migration dispatcher
    |   +-- migrations/  Migration functions grouped into v1_v20.rs, v21_v40.rs, v41_current.rs
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
    |   +-- format.rs    Recipe format types and build-stage definitions
    |   +-- parser.rs    TOML recipe parser
    |   +-- kitchen/     Build environment (cook, fetch, provenance)
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
    +-- dependencies/    Language/package dependency analysis helpers
    +-- derived/         Derived package metadata and build support
    +-- trigger/         Post-install trigger system
    +-- components/      File-to-component classification
    +-- compression/     Unified decompression (gzip, xz, zstd)
    +-- delta/           Binary delta generation and application
    +-- self_update.rs   Self-update support
    +-- version/         Version parsing and comparison
    +-- hash.rs          Multi-algorithm hashing (SHA-256, XXH128)

crates/conary-bootstrap/ Shared app bootstrap helpers
+-- src/
    +-- lib.rs           Tracing init, Tokio runtime entry, and shared finish helpers

apps/conary-test/        Declarative test infrastructure (TOML manifests, container management)
+-- src/
    +-- config/          TOML manifest and distro config parsing
    +-- engine/          Test suite, runner, assertions
    +-- container/       ContainerBackend trait, bollard implementation
    +-- report/          JSON output, SSE event streaming
    +-- server/          Axum HTTP API, MCP server (rmcp)
    +-- cli.rs           Binary entrypoint

apps/remi/               Remi server + federation
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
    +-- federation/      CAS peer-to-peer distribution
    |   +-- peer.rs      Peer registry and scoring
    |   +-- router.rs    Hierarchical chunk routing
    |   +-- manifest.rs  Signed chunk manifests
    |   +-- circuit.rs   Circuit breaker for failing peers
    |   +-- coalesce.rs  Request deduplication
    |   +-- mdns.rs      LAN peer discovery
    +-- bin/remi.rs      Remi server binary entry point

apps/conaryd/            conaryd local daemon
+-- src/
    +-- daemon/          conaryd local daemon
    |   +-- mod.rs       Daemon config defaults, runtime wiring, and JobKind re-export
    |   +-- routes.rs    REST API endpoints
    |   +-- jobs.rs      Priority job queue with SQLite persistence
    |   +-- client.rs    CLI forwarding client with SSE
    |   +-- socket.rs    Unix socket listener and socket-file lifecycle (TCP currently rejected)
    |   +-- auth.rs      SO_PEERCRED peer authentication
    |   +-- systemd.rs   Socket activation and watchdog
    +-- bin/conaryd.rs   conaryd binary entry point

crates/conary-mcp/       Shared MCP helpers
+-- src/
    +-- lib.rs           Transport-agnostic MCP primitives reused by workspace apps
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
  |  GET /v1/packages/            |
  |      fedora/nginx ----------->|
  |                               |-- Check converted package cache
  |                               |   and conversion job state
  |                               |
  |  200 OK (chunks, version) <---|  [if cached]
  |                               |
  |  202 Accepted + job_id <------|  [if not cached]
  |                               |-- Fetch upstream RPM
  |  GET /v1/jobs/{id} ---------->|-- Parse + convert to CCS
  |  200 {status: "converting"}<--|-- Store chunks in CAS
  |  ...polling...                |-- Record conversion result in SQLite
  |  200 {status: "complete"} <---|
  |                               |
  |  GET /v1/chunks/{hash} ------>|-- Bloom filter check
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
  conary system generation build
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

### Generation Module (`crates/conary-core/src/generation/`)

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
6 phases aligned with Linux From Scratch 13:

```
Phase 1: CrossTools (LFS Ch5)
  Cross-toolchain for target arch
  Produces: $LFS/tools/
       |
Phase 2: TempTools (LFS Ch6-7)
  Temporary tools (17 cross-compiled + 6 chroot packages)
       |
Phase 3: FinalSystem (LFS Ch8)
  Complete Linux system (77 packages)
  Built inside chroot
       |
Phase 4: SystemConfig (LFS Ch9)
  Network, fstab, kernel, bootloader configuration
       |
Phase 5: BootableImage (LFS Ch10)
  systemd-repart for rootless image generation (fallback: sfdisk/mkfs)
  Output formats: raw, qcow2, ISO, EROFS
       |
Phase 6: Tier2 (BLFS + Conary)
  PAM, OpenSSH, curl, Rust, Conary self-hosting
```

Aligned with LFS 13 (binutils 2.45, gcc 15.2.0, glibc 2.42,
kernel 6.16.1). All recipes carry SHA-256 checksums enforced at
build time (`--skip-verify` to override). All stages run in
sandboxed containers via `ContainerConfig::pristine_for_bootstrap()`.

Bootstrap trust has a TOFU boundary: the first trusted TUF root metadata and
bootstrap source manifests must arrive through an authenticated out-of-band
channel or another operator-controlled path. `--skip-verify` is only an
explicit bootstrap escape hatch and does not establish repository trust by
itself.

Supports x86_64, aarch64, and riscv64 targets. Dry-run mode
(`--dry-run`) validates the full pipeline without building.

## Database Schema (v65)

All runtime state lives in SQLite, and migrations are dispatched from
`crates/conary-core/src/db/schema.rs`.

The stable table families are:

- Installed state: troves, changesets, files, components, dependencies, and provides
- Repository and resolution state: repositories, synced package metadata, capability inputs, labels, and canonical mapping data
- System state and configuration: state snapshots, config tracking, triggers, redirects, and settings
- Security and provenance: TUF metadata, provenance records, admin tokens, and audit data
- Service and federation state: conversion/cache/download analytics, federation peers, and test-run persistence

When exact table names or counts matter, inspect `crates/conary-core/src/db/models/`
and the active migration functions instead of relying on this overview.

## Package Graph

The root manifest is now a virtual workspace. Build the owning crate directly:

| Package | Purpose | Typical command |
|---------|---------|-----------------|
| `conary` | Package-manager CLI | `cargo build -p conary` |
| `remi` | Remi conversion/proxy service | `cargo build -p remi` |
| `conaryd` | Local daemon | `cargo build -p conaryd` |
| `conary-test` | Test harness | `cargo build -p conary-test` |
| `conary-core` | Shared library | `cargo build -p conary-core` |
| `conary-mcp` | Shared MCP helpers | `cargo build -p conary-mcp` |

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
