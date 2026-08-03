---
last_updated: 2026-08-03
revision: 39
summary: Describe workspace architecture, source-authority projections, repository trust, package transactions, lifecycle execution, typed carrier security, generation GC, and service boundaries
---

# Conary Architecture

This document describes the internal architecture of Conary, a modern system
manager written in Rust. It covers the major subsystems, their interactions,
and the data flow for core operations.

## System Overview

```
apps/conary/ (CLI)
  cli/ + app.rs + dispatch.rs + dispatch/
      |
      +-- install / update / remove
      +-- repo / publish / cook / new / try / query / model / ccs / collection
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
      +-- remote metadata, static repos, Remi client, mirrors, substituters

Supporting workspace members
  apps/remi/         public/admin package service, search, federation, MCP
  apps/conaryd/      local daemon, auth, job queue, REST/SSE routes, package execution
  apps/conary-test/  integration harness, HTTP API, MCP, container runners
  crates/conary-bootstrap/ shared tracing/runtime/exit helpers for workspace binaries
  crates/conary-agent-contract/ transport-neutral agent operation contract
  crates/conary-mcp/ shared MCP adapter helpers
```

## Core Concepts

### Trove

The fundamental unit. A trove represents a package, component, or collection.
Each trove has a name, version (epoch:version-release), and optional flavor.
Troves are stored in the `troves` table with install reason tracking
(explicit vs dependency) and optional label provenance.

### Changeset

A durable transaction record. Install, remove, and update paths create a
changeset entry so pending, applied, failed, and rolled-back outcomes remain
explicit. Database, guarded live-root, and generation paths have different
recovery guarantees; a changeset does not promise universal filesystem
rollback. Each changeset carries a UUID for crash-recovery correlation.

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

The project is a virtual Cargo workspace with 8 members:

```
apps/conary/             CLI binary
+-- src/
    +-- main.rs          Thin entrypoint
    +-- app.rs           Bootstrap and top-level app wiring
    +-- dispatch.rs      Public dispatch entrypoint and child-router hub
    +-- dispatch/        Root, system, CCS, model, automation, and namespace routers
    +-- cli/             Clap command definitions
    +-- commands/        Command implementations (install, repo, publish, query, model/remove/bootstrap hubs + child modules, ccs, system)

crates/conary-core/      Core library crate
+-- src/
    +-- lib.rs           Internal workspace crate surface, not a stable external API
    +-- operations.rs    Shared operation vocabulary across CLI and daemon boundaries
    +-- db/              Database layer
    |   +-- schema.rs    Current pre-alpha schema epoch initializer and rebuild gate
    |   +-- current_schema/ One schema split into package-manager, repository, and Remi ownership files
    |   +-- models/      ORM-style model structs
    |   |   +-- try_session.rs M1b package try session state
    +-- transaction/     Composefs-native transaction engine
    |   +-- mod.rs       TransactionEngine, state machine (resolve/fetch/commit/build/select)
    |   +-- package_relations.rs Typed source-ABI relation planning and validation
    |   +-- recovery.rs  Exact committed-generation recovery
    +-- config_transaction.rs Exact RPM, Debian, Arch, and Conary config decisions plus durable generation snapshot types
    +-- generation/      EROFS generation building and composefs mounting
    |   +-- builder.rs   Public generation-builder hub
    |   +-- builder/create.rs Generation creation orchestration
    |   +-- builder/rebuild.rs Recovery rebuild orchestration
    |   +-- builder/boot_assets.rs Runtime/generation boot asset resolution
    |   +-- builder/initramfs.rs Dracut initramfs generation support
    |   +-- builder/kernel.rs Kernel release discovery
    |   +-- builder/root_validation.rs Self-contained runtime root validation
    |   +-- builder/sysroot.rs CAS-backed runtime sysroot materialization
    |   +-- builder/runtime_inputs.rs CAS-backed runtime input classification, validation, and security.capability xattr attachment for persisted file capabilities
    |   +-- root_manifest.rs Exact immutable-root and mutable-state manifest contract
    |   +-- root_manifest/scan.rs Selected-root capture and CAS ingestion
    |   +-- root_manifest/materialize.rs Exact typed-root reconstruction
    |   +-- root_manifest/composefs.rs Typed manifest to EROFS serialization
    |   +-- artifact.rs  Generation artifact contract, CAS manifest, and boot assets
    |   +-- artifact/tests.rs Artifact contract and tamper-regression coverage
    |   +-- export.rs    Raw/qcow2 generation artifact disk export
    |   +-- export/tests.rs Export carrier and provenance regression coverage
    |   +-- mount.rs     composefs mount/unmount, current symlink
    |   +-- metadata.rs  Generation metadata (JSON)
    |   +-- composefs.rs composefs detection and feature probing
    |   +-- gc.rs        Typed local CAS reachability and object collection
    |   +-- delta.rs     EROFS image delta computation
    |   +-- composefs_rs_eval.rs composefs-rs evaluation (feature-gated)
    +-- activation/      Exact runtime work projected onto immutable generations
    |   +-- systemd.rs   Typed systemctl invocation and canonical boot argv
    |   +-- systemd/grammar.rs Shared parser/proxy systemctl token grammar
    |   +-- security_policy.rs SELinux/AppArmor provider union and live-edge split
    |   +-- security_policy/ Current upstream helper grammars and executable identity
    +-- scriptlet/       Exact selected-root lifecycle execution
    |   +-- process.rs   Chroot, mount namespace, and provider bind-mount boundary
    |   +-- activation_capture.rs Actual activation-provider argv capture
    |   +-- native_lifecycle.rs Source-ABI lifecycle entry execution
    +-- resolver/        Dependency resolution
    |   +-- sat.rs       SAT-backed solver and transaction plan construction
    |   +-- plan.rs      Resolution plan output types and helpers
    |   +-- provides_index.rs Provider index construction for dependency matching
    |   +-- provider/    Provider loading, matching, traits, and shared types
    |   +-- canonical.rs Canonical package identity helpers
    |   +-- component_resolver.rs Component-aware resolution helpers
    |   +-- conflict.rs  Conflict reporting and policy support
    |   +-- identity.rs  Dependency identity normalization
    +-- repository/      Remote package sources
    |   +-- static_repo/ Static repository format, publishing, sync, and key persistence
    |   +-- trust.rs     Tagged Debian, RPM, and Arch repository authority contracts
    |   +-- trust/openpgp.rs Trust-role dispatch over the pinned and Arch keyring owners
    |   +-- trust/openpgp/arch/ Pacman keyring grammar, trust snapshot, and ALPM signature semantics
    |   +-- parsers/     Authenticated RPM repodata, Debian Packages, and Arch DB grammars
    |   +-- sync.rs      Trust preparation and atomic repository metadata persistence
    |   +-- download.rs  Metadata checksum plus ecosystem package-signature termination
    |   +-- remi.rs      Remi client hub (sync client)
    |   +-- remi/        Remi protocol DTOs, refusal formatting, async client, and tests
    |   +-- chunk_fetcher.rs ChunkFetcher trait + HTTP/local/composite impls
    |   +-- mirror_health.rs Mirror health scoring
    |   +-- mirror_selector.rs Ranked mirror selection
    |   +-- substituter.rs Content substituter chain
    |   +-- resolution.rs Per-package routing strategies
    |   +-- dependency_model.rs Cross-distro dependency model (provides/requires/groups)
    |   +-- versioning.rs Cross-distro version scheme awareness
    |   +-- resolution_policy.rs Exact source scope, mixing, and eligibility policy
    |   +-- effective_policy.rs Shared runtime source-policy loading from pins + settings
    +-- filesystem/      Storage layer
    |   +-- cas.rs       Content-addressable store (SHA-256 keyed)
    |   +-- vfs/         Virtual filesystem tree (arena allocator)
    |   +-- fsverity.rs  fs-verity content verification
    |   +-- path.rs      safe_join, sanitize_filename, sanitize_path
    +-- packages/        Format parsers
    |   +-- rpm.rs       RPM parser
    |   +-- rpm/authority.rs Lossless RPM identity and declared-capability records
    |   +-- deb.rs       DEB parser
    |   +-- deb/authority.rs Lossless Debian identity and declared-capability records
    |   +-- arch.rs      Arch parser
    |   +-- arch/authority.rs Lossless ALPM identity and declared-capability records
    |   +-- source_authority.rs Closed format dispatch and consumer projections
    |   +-- common.rs    Temporary config/lifecycle/payload bridge; #105 removal target
    +-- ccs/             Native package format
    |   +-- builder.rs   CCS package builder
    |   +-- manifest.rs  Root CCS TOML/CBOR manifest schema and validation hub
    |   +-- manifest/    Declarative hook schemas and manifest tests
    |   +-- manifest_provenance.rs Provenance DTOs embedded by the root manifest
    |   +-- signing.rs   Ed25519 signing
    |   +-- convert/     RPM/DEB/Arch-to-CCS conversion
    |   +-- enhancement/ Exact post-conversion provenance recording
    |   +-- export/      OCI image export
    |   +-- hooks/       typed host capability inventory plus systemd, service, tmpfiles, sysctl, user/group, alternatives adapters
    |   +-- policy.rs    Build policy engine
    +-- model/           Declarative system state
    |   +-- parser.rs    TOML model file parser
    |   +-- diff.rs      Current vs desired state diff
    |   +-- remote.rs    Remote collection fetching
    |   +-- lockfile.rs  Model lockfile for remote includes
    |   +-- signing.rs   Ed25519 collection signing
    |   +-- replatform.rs Cross-distro system replatforming + executable transaction planning
    +-- recipe/          Source-based package building
    |   +-- format.rs    Recipe format types and build-stage definitions
    |   +-- parser.rs    TOML recipe parser
    |   +-- scaffold.rs  Exact named recipe scaffolding and deterministic materialization
    |   +-- hermetic/    M2a unsigned hermetic evidence, policy, source identity, and diagnostics
    |   +-- kitchen/     Build environment (cook, fetch, offline build, provenance)
    |   +-- build graph  Multi-recipe build ordering
    |   +-- cache.rs     Build artifact cache
    +-- trust/           TUF supply chain trust
    |   +-- client.rs    TUF metadata fetch and verification
    |   +-- metadata.rs  TUF metadata types (root, timestamp, snapshot, targets)
    |   +-- ceremony.rs  Root key ceremony
    |   +-- verify.rs    Signature verification
    +-- capability/      Package capability system
    |   +-- declaration.rs Capability declarations (network, fs, syscalls)
    |   +-- enforcement/ Landlock (filesystem) + seccomp-BPF (syscalls)
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
    +-- dependencies/    Exact typed dependency-class grammar
    +-- derived/         Derived package metadata and build support
    +-- trigger/         Post-install trigger system
    +-- components/      Exact standard component-name types
    +-- compression/     Unified decompression (gzip, xz, zstd)
    +-- delta/           Binary delta generation and application
    +-- self_update.rs   Self-update support
    +-- version/         Version parsing and comparison
    +-- hash.rs          Multi-algorithm hashing (SHA-256, XXH128)

crates/conary-bootstrap/ Shared app bootstrap helpers
+-- src/
    +-- lib.rs           Tracing init, Tokio runtime entry, and shared finish helpers

crates/conary-agent-contract/ Transport-neutral agent operation contract
+-- src/
    +-- result.rs        Operation envelopes, risk, confirmation, evidence, and errors
    +-- resource.rs      Canonical agent resource URI helpers
    +-- catalog.rs       Resource and prompt catalogs plus cache policy metadata

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
    |   +-- conversion.rs On-demand foreign-package-to-CCS conversion
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
    |   +-- routes.rs    Route hub and public route DTO re-exports
    |   +-- routes/      Router assembly, API DTOs, errors, auth, DB/SSE helpers, endpoints
    |   +-- jobs.rs      Priority job queue with SQLite persistence
    |   +-- client.rs    CLI forwarding client with SSE
    |   +-- socket.rs    Unix socket listener and socket-file lifecycle (TCP currently rejected)
    |   +-- auth.rs      SO_PEERCRED peer authentication
    |   +-- systemd.rs   Socket activation and watchdog
    +-- bin/conaryd.rs   conaryd binary entry point

crates/conary-mcp/       Shared MCP adapter helpers
+-- src/
    +-- lib.rs           MCP-specific primitives reused by workspace apps
```

`conary-core` is currently an internal workspace crate. Its broad module exports
exist for workspace app reuse and integration tests, not as a stable external
API or SDK promise. The crate is marked `publish = false`; a curated public
facade would need its own design if Conary later supports external library
consumers.

## Data Flow: Package Installation

This is the primary operation. The flow from
`conary install nginx --yes`:

```
1. RESOLVE
   +-- Parse package specifier (name, version constraint, repo)
   +-- Check per-package routing strategy (Remi, delegate, or one exact
       authenticated repository-package row)
   +-- Query repositories or Remi server for package metadata
   |   +-- Static indexes carry typed provides and requirement expressions
   |   +-- String dependency lists are not resolution authority
   +-- Resolve transitive dependencies via dependency graph
   +-- Check for conflicts, pinned packages, redirects

2. PREPARE
   +-- Download package(s) - parallel via rayon if multiple
   |   +-- For Remi: fetch CCS chunks, assemble package
   |   +-- For foreign formats: download RPM/DEB/Arch package file
   +-- Parse identity and provisions into lossless RPM, Debian, or ALPM authority
   |   +-- Explicit fallible projections serve resolution and CCS v3 signing
   |   +-- #105 removes the remaining common config-declaration bridge
   +-- Detect package format (magic bytes or extension)
   +-- Convert RPM/DEB/Arch input to source-independent CCS on-the-fly
   +-- Resolve source requirements against the typed host capability inventory
   |   +-- arch/ABI/libc/loader, init, LSM, filesystem, boot/kernel, helpers
   |   +-- Source format and host capabilities are orthogonal typed axes
   |   +-- No pairwise distro converters or distro-name/string gates
   +-- Preflight typed lifecycle plans before dry-run or mutation
   |   +-- Exact source-ABI stage, argv, trigger, and payload boundaries
   |   +-- No source package-manager process or database dependency
   |   +-- Missing semantics fail as required implementation defects

3. TRANSACTION (composefs-native)
   +-- Create TransactionEngine, acquire lock
   +-- PLAN: VFS preflight - detect file conflicts
   +-- FETCH: Store package content in CAS
   +-- DB_COMMIT: Record trove, files, components, dependencies in SQLite
   |   (Point of no return)
   +-- BUILD: Construct complete generation artifact from DB/CAS state
   +-- SELECT: Update /conary/current for next boot
   +-- POST_SCRIPTS: Run post-install scriptlets under the transaction policy
   +-- TRIGGERS: Fire matching triggers (ldconfig, mime, icons, etc.)

4. RECOVERY (on crash)
   +-- Check /conary/current for a valid generation artifact
   +-- If invalid: rebuild the selected artifact from DB/CAS state
   +-- If explicit boot-selection recovery is requested: scan generations/
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

## Data Flow: Hermetic Recipe Cook And Project Publish

M2a makes `conary cook --isolated` the hermetic recipe build path. The command
loads the local hermetic builder config, refuses recipes with build dependencies
until dependency content locks exist, prefetches sources, and then runs Kitchen
with network disabled, pristine/no-host-mount execution, reproducibility
controls, exact source and dependency identities, diagnostic command-risk
reports, builder environment identity, and local host-vs-hermetic divergence
diagnostics. Marker-file and command-text ecosystem inference is not an
authority boundary; the signed input identity and actual offline execution are.

Project-form `conary publish <target>` uses the same hermetic Kitchen path
before adding the resulting CCS package to a static repository. M2a records
unsigned hermetic evidence in CCS provenance, but it does not create signed
build-attestation envelopes. Artifact-form
`conary publish <pkg.ccs> <target>` still rejects until the M2b attestation and
publish gates land.

## System Generations

Conary can build and select immutable system-generation artifacts using EROFS
images and Linux composefs. This remains an advanced, explicitly gated path in
the limited preview.

Full system adoption establishes the first complete generation input without
turning unowned host state into package ownership. One exact selected-root scan
preserves package-owned anchors and payload claims, reconciles their
materialized node/content authority, and assigns only the remaining retained
paths to one `CapturedRoot` trove. `AdoptedFull`, `Taken`, `Repository`, `File`,
and `CapturedRoot` are the finite complete generation-input sources;
`AdoptedTrack` remains metadata-only. The scanner excludes the finite
ephemeral/API/device/user domains plus Conary's own normalized runtime and
database subtrees through both lexical and resolved path authority, and refuses
a runtime root that resolves to `/`. It preserves numeric ownership, full mode and node type,
timestamps, symlink targets, CAS content, xattrs, and global hardlink topology.
Generation runtime collection consumes that persisted authority without
consulting a native package manager or its database.

### Architecture

```
Generation-aware package mutation
       |
  materialize latest authoritative selected root
       |
  +-----------+
  | Isolated  |-- Apply payload, native lifecycle, CCS hooks, triggers,
  | Root      |-- and config decisions without mutating the live root
  +-----------+
       |
  +-----------+
  | Capture   |-- Exact typed generation-root + mutable-state manifests
  +-----------+-- Candidate is durable before the SQLite transaction commits
       |
  +-----------+
  |  EROFS    |-- composefs-rs serializes the generation-root manifest
  | Builder   |-- Regular content uses verified external CAS references
  +-----------+
       |
  +-----------+
  | composefs |-- Linux 6.2+ overlay with fs-verity content verification
  | Mount     |-- Exact mutable-state/config projection is generation-local
  +-----------+
       |
 Generation N (immutable, verified)
       |
  conary system generation export --format raw|qcow2|iso
       |
  validated manifests + carrier capabilities + CAS -> exact state/security projection -> staged ESP/rootfs
```

### Generation Lifecycle

1. **Select**: Materialize the latest cumulative selected-root candidate, or the current generation when no publication debt is pending
2. **Mutate**: Apply payload changes, typed native lifecycle, CCS hooks, triggers, and config decisions inside that isolated root
3. **Record**: Capture exact immutable and mutable-state manifests, persist the selected-root candidate, and record recoverable publication debt before committing package state
4. **Build**: Validate the captured manifests and serialize the immutable manifest to EROFS using verified CAS content
5. **Materialize state**: Project `/etc/...` manifest paths into the generation-local `/etc` overlay upper without retaining the `/etc` prefix, then apply the ordered typed config transactions; `/var` and `/srv` remain live mutable-root state
6. **Publish**: Advance one persisted replay phase at a time: artifact ready, current link durable, configuration status projected, matching system state active, and generation-bound database backup durable. Only the final phase makes publication debt terminal
7. **Recover**: Under the runtime mutation lock, resume at the persisted phase and replay each remaining idempotent effect. A matching `/conary/current` link proves only link publication; it never implies configuration projection or database backup completion
8. **Compensate**: Rollback records a new exact compensating selected root and removes terminal candidates
9. **GC**: Remove old generations only after retaining every recoverable publication candidate and its typed CAS roots

Raw, qcow2, and ISO export copy both typed manifests and their manifest-listed
CAS objects. Export reconstructs `/var` and `/srv` in the carrier root and
projects `/etc` into `/conary/etc-state/<generation>` above one explicit empty
lower directory. Read-only carriers copy that seeded config upper to runtime
tmpfs before mounting it, so writability never replaces or bypasses the
artifact's typed state authority. Carrier projection restores the verified
generation-root metadata on `/` and the exact manifest metadata on `/usr`,
`/etc`, `/boot`, and each root symlink; the disk backend excludes only `/boot/`
contents so it does not replace the signed mountpoint with a default directory.
Boot entries disable source-`fstab` generation because a self-contained carrier
owns a new partition topology. Writable carriers declare their `CONARY_ESP`
mount through the boot contract, while the original `/etc/fstab` bytes remain
part of the unchanged mutable-state seed.

Generation build also seals a versioned carrier-capability projection from the
persisted target inventory into artifact manifest v3. Host-inventory document
version 4 captures the target `/usr` node's exact opaque `security.selinux`
value when that xattr is present; absence or an unsupported xattr facility is
an explicit `None`, while any other probe failure aborts initialization.
Export applies a sealed value only to regular files and directories in the
copied carrier CAS subtree. Composefs logical nodes retain their own exact
manifest xattrs; this separate target-supplied value authorizes the external
immutable objects that back their bytes. Conary does not parse the context or
select it from a distro, path, policy name, or built-in label table.
Host-inventory document version 3 must be replaced by rerunning
`conary system init`, and artifact manifest v2 generations must be rebuilt
before export. For a bootstrap target that is not running yet, the exact
captured `/usr` manifest node supplies the same target fact; bootstrap artifact
writers may not substitute the build host's context or an empty default.

### Generation Module (`crates/conary-core/src/generation/`)

The primary builder for composefs generations. Uses the composefs-rs crate
(v0.3.0) to serialize validated exact `GenerationRootManifest` input to EROFS.
Explicit generation builds project installed state into the same typed root
contract. Package mutation publication accepts only its persisted cumulative
selected-root candidate; there is no database-snapshot publication fallback.
Submodules: builder.rs (public
generation-builder hub),
builder/create.rs and builder/rebuild.rs (generation creation and recovery
rebuild orchestration), builder/carrier_capabilities.rs (persisted target
capability projection), builder/boot_assets.rs, builder/initramfs.rs,
builder/kernel.rs, and builder/sysroot.rs (runtime boot asset and sysroot
materialization support), builder/root_validation.rs and
builder/runtime_inputs.rs (self-contained runtime input validation), and
root_manifest/composefs.rs (exact typed-root EROFS serialization). artifact.rs
owns the exportable generation contract and boot assets; export.rs owns
raw/qcow2 disk export from validated artifacts; mount.rs owns composefs
mount/unmount; metadata.rs owns JSON
metadata), gc.rs (typed CAS reachability and object collection), delta.rs
(EROFS image deltas), and
composefs.rs (runtime feature detection). Exact config policy and persisted
snapshot types live in `crates/conary-core/src/config_transaction.rs`;
`apps/conary/src/commands/generation/config_transaction.rs` captures live
identities and atomically materializes generation-local `/etc` uppers. There is
no generic hash-conflict/manual-merge path. Target immutable-backing security
discovery and application live in
`crates/conary-core/src/ccs/hooks/capabilities/filesystem_security.rs`.

### composefs Integration

The composefs driver (Linux 6.2+, `CONFIG_EROFS_FS`) provides:
- Content-verified overlays using fs-verity
- Efficient sharing of identical files across generations via CAS
- Atomic generation switching without unmounting

## Bootstrap Pipeline

Build a complete Conary-managed system from scratch. The pipeline has
6 phases whose package selection and ordering are guided by LFS 13.0-systemd,
with documented recipe-level deviations where Conary intentionally differs:

```
Phase 1: CrossTools (LFS Ch5)
  Cross-toolchain for target arch
  Produces: $LFS/tools/
       |
Phase 2: TempTools (LFS Ch6-7)
  Temporary tools (17 cross-compiled + 6 chroot packages)
       |
Phase 3: FinalSystem (LFS Ch8)
  Complete Linux system (80 packages; Chapter 8 set with
  systemd-boot-over-GRUB deviation)
  Built inside chroot
       |
Phase 4: SystemConfig (LFS Ch9)
  Network, fstab, kernel, bootloader configuration
       |
Phase 5: BootableImage (LFS Ch10)
  systemd-repart for bootstrap sysroot disk images
  Output formats: raw/qcow2 disk images, bootstrap preview ISO scaffolding,
  or EROFS generation artifact
       |
Phase 6: Tier2 (BLFS + Conary)
  PAM, OpenSSH, curl, Rust, Conary self-hosting
```

Do not treat this section as the authoritative version inventory. Use
`recipes/versions.toml`, individual recipe headers, and the active bootstrap
specs/plans when exact package versions or intentional divergences matter.
Tier 2 recipes and self-host-specific staged inputs enforce SHA-256 checksums;
earlier bootstrap phases still carry MD5-only recipe entries in the current
tree. Recipe execution uses the bootstrap container configuration where the
phase supports it; the self-hosting VM wrapper deliberately runs chroot-owning
phases through a rootful handoff so the Rust bootstrap code owns `/dev`,
`/proc`, `/sys`, `/run`, and `chroot` setup.

Bootstrap trust has a TOFU boundary: the first trusted TUF root metadata and
bootstrap source manifests must arrive through an authenticated out-of-band
channel or another operator-controlled path. Bootstrap source identities are
mandatory; placeholder checksums and verification-bypass modes are rejected.

Supports x86_64, aarch64, and riscv64 targets. Dry-run mode
(`--dry-run`) validates the full pipeline without building.

## Database Schema

All runtime state lives in SQLite. The pre-alpha database contract has one
current schema epoch initialized by `crates/conary-core/src/db/schema.rs`.
The schema itself is split by ownership under
`crates/conary-core/src/db/current_schema/sql/`: local package-manager state,
repository/service state, and Remi conversion/administration state.

Databases from retired schema revisions are rejected with an explicit rebuild
requirement. Conary does not carry a schema compatibility chain or attempt to
preserve derived queue and workflow history while there are no external users.
Rebuilds must come from authoritative package, repository, and conversion
inputs.

The stable table families are:

- Installed state: troves, changesets, files, components, dependencies, and provides
- Repository and resolution state: repositories, synced package metadata, capability inputs, labels, and canonical mapping data
- System state and configuration: state snapshots, config tracking, triggers, redirects, and settings
- Try state: active/kept/rolled-back package try sessions and selected generation metadata
- Security and provenance: TUF metadata, provenance records, admin tokens, and audit data
- Service and federation state: conversion/cache/download analytics, federation peers, and test-run persistence

When exact table names or counts matter, inspect `crates/conary-core/src/db/models/`
and the current ownership SQL instead of relying on this overview.

## Package Graph

The root manifest is now a virtual workspace. Build the owning crate directly:

| Package | Purpose | Typical command |
|---------|---------|-----------------|
| `conary` | Package-manager CLI | `cargo build -p conary` |
| `remi` | Remi conversion/proxy service | `cargo build -p remi` |
| `conaryd` | Local daemon: query routes plus install/remove/update/enhance jobs | `cargo build -p conaryd` |
| `conary-test` | Test harness | `cargo build -p conary-test` |
| `conary-bootstrap` | Shared binary bootstrap helpers | `cargo build -p conary-bootstrap` |
| `conary-core` | Shared library | `cargo build -p conary-core` |
| `conary-agent-contract` | Transport-neutral agent operation contract | `cargo build -p conary-agent-contract` |
| `conary-mcp` | Shared MCP helpers | `cargo build -p conary-mcp` |

## Key Design Decisions

**Database-first with SQLite-native recovery backups**: Every piece of state
lives in SQLite. No TOML/YAML/JSON config files drive runtime state. The live
database is the single source of truth, queryable with standard SQL tools.
Conary writes SQLite-native checkpoint backups around first-wave
adoption/unadoption mutations and writes a generation-bound SQLite backup under
`/conary/generations/<n>/state/` when a generation publication reaches the
selected-generation boundary.

**Composefs-native transactions**: Every package mutation follows one linear
pipeline: resolve -> fetch -> materialize an isolated selected root -> run typed
lifecycle, payload, config, and trigger work inside that root and one SQLite
transaction -> persist the exact selected-root candidate -> commit SQLite ->
publish the recorded generation. The selected root starts from the latest
retryable candidate, the current generation artifact, or authoritative DB/CAS
state when no generation exists. There is no mutable-host package execution
path.

Before the SQLite commit, any lifecycle, payload, config, trigger, or validation
failure rolls back the database transaction and discards the selected root.
After commit, a publication failure leaves typed debt plus the exact candidate
for deterministic retry. `LiveRootTransaction` remains an internal journal for
the disposable selected-root session; it is not authority to mutate the host
root. DB backups remain recovery artifacts, not a second mutable source of
truth. If `/conary/current` names a missing or invalid artifact, recovery
rebuilds from DB/CAS state while explicit boot-selection recovery owns scanning,
promotion, and remounting.
Generation-aware CCS installs follow the same model for file capabilities by
persisting file-capability authority in SQLite first, attaching
`security.capability` during runtime-input collection, requiring immediate
publication instead of `--defer-generation`, and surfacing the resulting
capability-xattr count through generation inspection metadata.

SQLite-native backups recover Conary manager visibility for packages and
generations represented by the backed-up DB. They do not recover missing
package payloads, private keys, remote repository history, or native
package-manager transaction history.

**Content-addressable storage**: Files are stored by SHA-256 hash in a flat
CAS directory. This enables deduplication across packages, integrity
verification, and rollback-oriented recovery when the required content and
state references have been preserved.

**Chunk-level distribution**: Packages are split into variable-size chunks
via FastCDC. Clients only download chunks they don't already have, giving
implicit delta compression without pre-computing version-to-version diffs.

**Signed remote system models**: Remote model collections have one exact wire
contract. `conary model publish` requires an Ed25519 signing key and an
authenticated Remi admin request (`REMI_ADMIN_TOKEN` or
`CONARY_REMI_ADMIN_TOKEN`). Remi verifies the signature and canonical content
hash before atomically replacing persisted collection state, then serves the
same signed data and signer identity. Every online or cached remote include is
reverified against the model's explicit `[include].trusted_keys`; unsigned
collections, missing trust roots, signer-ID mismatches, and stale cache
metadata fail closed.

**Source package authority pipeline**: RPM, Debian, and Arch parsers retain
exact identity and declared provisions in format-specific authority records.
The closed `SourcePackageAuthority` dispatch exposes named, fallible
projections for dependency resolution and CCS v3 signing; exact identity is
never recovered from or inserted into the declared-capability list. Source
configuration declarations likewise remain format-specific and carry an
explicit matched/absent payload association. Signed CCS, installed config
state, and generation transactions preserve that distinction without
synthesizing content. The retired common package/config adapters are gone. The
W5 contract in
[`docs/specs/source-package-authority.md`](specs/source-package-authority.md)
owns the lossless format-specific authority and consumer boundaries.
Conversion remains transparent to the caller, but normalization is owned by
the named consumer rather than initial parsing.

**Namespace isolation**: Scriptlets run in Linux containers (mount, PID, IPC,
UTS namespaces) with resource limits. Capability enforcement uses mandatory
Landlock rules for exact absolute path roots and seccomp-BPF for exact
target-ABI syscall names. Filesystem wildcards, syscall wildcards, named
syscall profiles, cross-ABI aliases, and Linux process-capability inference are
not authority; packages declare exact `capabilities.linux.required` values.

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

- [ROADMAP.md](/ROADMAP.md) - Forward-looking development roadmap
- [docs/SCRIPTLET_SECURITY.md](/docs/SCRIPTLET_SECURITY.md) - Scriptlet isolation details
