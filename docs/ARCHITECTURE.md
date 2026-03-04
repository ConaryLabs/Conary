# Conary Architecture

This document describes the internal architecture of Conary, a modern package
manager written in Rust. It covers the major subsystems, their interactions,
and the data flow for core operations.

## System Overview

```
                            CLI (src/cli/, src/main.rs)
                                      |
                    +-----------------+-----------------+
                    |                 |                 |
              Commands          Daemon Client      Cook Command
          (src/commands/)     (src/daemon/client)  (src/commands/cook.rs)
                    |                 |                 |
     +--------------+-----+     +----+----+     +------+------+
     |              |     |     | conaryd |     |   Kitchen   |
     |              |     |     | daemon  |     | (src/recipe)|
     |              |     |     +---------+     +-------------+
     |              |     |
  Install       Query   Model
  Remove        Search  Apply/Diff
  Update        SBOM    Snapshot
     |              |     |
     +------+-------+-----+--------+
            |                      |
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
     |  SQLite v44 |        +------+-----+
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

## Module Map

```
src/
+-- main.rs              Entry point, CLI dispatch
+-- lib.rs               Public API surface
+-- cli/                 Clap command definitions
+-- commands/            Command implementations
|   +-- install/         Install pipeline (resolve, prepare, execute)
|   +-- model.rs         System model operations
|   +-- trust.rs         TUF trust management
|   +-- cook.rs          Recipe cooking
|   +-- derived.rs       Derived package creation
|   +-- adopt/           System adoption
+-- db/                  Database layer
|   +-- schema.rs        Schema v44, migration dispatcher
|   +-- migrations.rs    All 44 migration functions
|   +-- models/          ORM-style model structs
+-- transaction/         Crash-safe atomic operations
|   +-- journal.rs       Append-only recovery journal
|   +-- planner.rs       VFS preflight conflict detection
|   +-- recovery.rs      Roll-forward/roll-back recovery
+-- resolver/            Dependency resolution
|   +-- graph.rs         Directed dependency graph
|   +-- engine.rs        Resolution algorithm
|   +-- sat.rs           SAT-based conflict resolution
|   +-- plan.rs          Resolution plan output
+-- repository/          Remote package sources
|   +-- metadata.rs      Index parsing (RPM repodata, DEB Packages, Arch DB)
|   +-- remi.rs          Remi client (CCS chunk fetcher)
|   +-- chunk_fetcher.rs ChunkFetcher trait + HTTP/local/composite impls
|   +-- mirror_health.rs Mirror health scoring
|   +-- mirror_selector.rs Ranked mirror selection
|   +-- metalink.rs      Metalink XML parser
|   +-- substituter.rs   Content substituter chain
|   +-- resolution.rs    Per-package routing strategies
+-- filesystem/          Storage layer
|   +-- cas.rs           Content-addressable store (SHA-256 keyed)
|   +-- vfs/             Virtual filesystem tree (arena allocator)
|   +-- deployer.rs      CAS-to-filesystem file deployment
+-- packages/            Format parsers
|   +-- rpm.rs           RPM parser
|   +-- deb.rs           DEB parser
|   +-- arch.rs          Arch parser
|   +-- common.rs        Unified PackageMetadata
+-- ccs/                 Native package format
|   +-- builder.rs       CCS package builder
|   +-- manifest.rs      CBOR manifest with Merkle tree
|   +-- signing.rs       Ed25519 signing
|   +-- lockfile.rs      ccs.lock dependency pinning
|   +-- convert/         Legacy-to-CCS conversion
|   +-- enhancement/     Retroactive CCS hook application
|   +-- export/          OCI image export
|   +-- hooks/           systemd, tmpfiles, sysctl, user/group, alternatives
|   +-- policy.rs        Build policy engine
+-- model/               Declarative system state
|   +-- parser.rs        TOML model file parser
|   +-- diff.rs          Current vs desired state diff
|   +-- remote.rs        Remote collection fetching
|   +-- lockfile.rs      Model lockfile for remote includes
|   +-- signing.rs       Ed25519 collection signing
+-- recipe/              Source-based package building
|   +-- kitchen/         Build environment (cook, fetch, provenance)
|   +-- parser.rs        TOML recipe parser
|   +-- graph.rs         Multi-recipe build ordering
|   +-- cache.rs         Build artifact cache
|   +-- pkgbuild.rs      Arch PKGBUILD converter
+-- trust/               TUF supply chain trust
|   +-- client.rs        TUF metadata fetch and verification
|   +-- metadata.rs      TUF metadata types (root, timestamp, snapshot, targets)
|   +-- ceremony.rs      Root key ceremony
|   +-- verify.rs        Signature verification
+-- capability/          Package capability system
|   +-- declaration.rs   Capability declarations (network, fs, syscalls)
|   +-- enforcement/     Landlock (filesystem) + seccomp-BPF (syscalls)
|   +-- inference/       Heuristic capability detection
|   +-- resolver.rs      Capability-aware dependency resolution
+-- provenance/          Package DNA tracking
|   +-- source.rs        Source provenance (URL, VCS, checksums)
|   +-- build.rs         Build provenance (compiler, flags, env)
|   +-- signature.rs     Signature provenance
|   +-- content.rs       Content integrity
|   +-- slsa.rs          SLSA attestation generation
+-- federation/          CAS peer-to-peer distribution (server feature)
|   +-- peer.rs          Peer registry and scoring
|   +-- router.rs        Hierarchical chunk routing
|   +-- manifest.rs      Signed chunk manifests
|   +-- circuit.rs       Circuit breaker for failing peers
|   +-- coalesce.rs      Request deduplication
|   +-- mdns.rs          LAN peer discovery
+-- server/              Remi server (server feature)
|   +-- routes.rs        Public + admin Axum routers
|   +-- handlers/        HTTP handlers (chunks, packages, OCI, TUF, etc.)
|   +-- conversion.rs    On-demand legacy-to-CCS conversion
|   +-- r2.rs            Cloudflare R2 storage backend
|   +-- lite.rs          Remi Lite LAN proxy
|   +-- analytics.rs     Download event recording
|   +-- bloom.rs         Bloom filter for chunk existence
|   +-- security.rs      Rate limiting and IP banning
|   +-- federated_index.rs  Merged sparse index from upstream peers
|   +-- delta_manifests.rs  Pre-computed version deltas
|   +-- prewarm.rs       Background package pre-conversion
+-- daemon/              conaryd local daemon (daemon feature)
|   +-- routes.rs        REST API endpoints
|   +-- jobs.rs          Priority job queue with SQLite persistence
|   +-- client.rs        CLI forwarding client with SSE
|   +-- socket.rs        Unix socket + TCP listener
|   +-- auth.rs          SO_PEERCRED peer authentication
|   +-- systemd.rs       Socket activation and watchdog
+-- bootstrap/           System bootstrap from scratch
+-- automation/          Automated maintenance (security, orphans)
+-- container/           Namespace isolation for scriptlets
+-- trigger/             Post-install trigger system
+-- components/          File-to-component classification
+-- compression/         Unified decompression (gzip, xz, zstd)
+-- delta/               Binary delta generation and application
+-- version/             Version parsing and comparison
+-- hash.rs              Multi-algorithm hashing (SHA-256, Blake3, XXH128)
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

3. TRANSACTION (crash-safe)
   +-- Create TransactionEngine with journal
   +-- PLAN: VFS preflight - detect file conflicts before touching disk
   +-- PREPARE: Extract files, compute hashes
   +-- PRE_SCRIPTS: Run pre-install scriptlets (sandboxed)
   +-- BACKUP: Preserve existing files being overwritten
   +-- STAGE: Deploy files from CAS to staging area
   +-- FS_APPLY: Move staged files to final locations
   +-- DB_APPLY: Record trove, files, components, dependencies in SQLite
   |   (Point of no return - roll forward after this)
   +-- POST_SCRIPTS: Run post-install scriptlets (sandboxed)
   +-- TRIGGERS: Fire matching triggers (ldconfig, mime, icons, etc.)
   +-- SNAPSHOT: Create system state snapshot

4. CLEANUP
   +-- Delete journal on success
   +-- On crash: journal enables deterministic recovery
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

## Database Schema (v44)

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

Server (Remi):
  converted_packages / subpackage_relationships   Conversion tracking
  chunk_access                                    LRU cache tracking
  download_stats / download_counts                Analytics
  delta_manifests                                 Pre-computed version deltas
  remote_collections                              Cached remote model includes

Federation / Daemon:
  federation_peers / federation_stats   CAS federation
  daemon_jobs                           conaryd job queue
```

## Feature Gates

The codebase uses Cargo feature flags to keep the client binary lean:

| Feature  | Modules Enabled              | Binary     |
|----------|------------------------------|------------|
| (none)   | Core client                  | `conary`   |
| server   | server/, federation/         | `conary`   |
| daemon   | daemon/                      | `conary`   |

Build examples:
- `cargo build` -- client only
- `cargo build --features server` -- with Remi server
- `cargo build --features daemon` -- with conaryd daemon

## Key Design Decisions

**Database-first**: Every piece of state lives in SQLite. No TOML/YAML/JSON
config files for runtime state. The database is the single source of truth,
queryable with standard SQL tools.

**Journal-based crash recovery**: The transaction engine writes an append-only
journal before making filesystem changes. On crash, the journal determines
whether to roll forward (past the point of no return at DB_APPLY) or roll
back (before DB_APPLY).

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
- [ROADMAP.md](/ROADMAP.md) - Feature status and version history
- [docs/SCRIPTLET_SECURITY.md](/docs/SCRIPTLET_SECURITY.md) - Scriptlet isolation details
