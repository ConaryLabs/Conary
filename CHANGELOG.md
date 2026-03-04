# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.2.0] - 2026-03-02

Major release covering 10 months of development. Every subsystem listed below is implemented and tested.

### Added

#### Remi Server (feature-gated: `--features server`)
- On-demand CCS conversion proxy that converts RPM/DEB/Arch packages to CCS format when requested
- Chunk-level content-addressable storage with LRU eviction and access tracking (schema v27)
- 202 Accepted pattern for async conversion with job polling
- Bloom filter acceleration for chunk existence checks
- Batch endpoints for multi-chunk requests
- Sparse index for efficient client metadata sync
- Full-text package search powered by Tantivy
- Pull-through caching from upstream repositories
- Cloudflare R2 write-through for chunk storage
- Remi-native repository sync via `/v1/{distro}/metadata` API
- Public package index with search at packages.conary.io
- TUF supply chain trust with timestamp, snapshot, targets, and root role delegation
- CORS restrictions, token-bucket rate limiting, audit logging, and configurable ban lists
- Prometheus metrics export at `/v1/admin/metrics/prometheus`
- Podman-based integration test harness

#### conaryd Daemon (feature-gated: `--features daemon`)
- Local REST API for package operations over Unix socket (`/run/conary/conaryd.sock`)
- Optional TCP listener for remote management
- SO_PEERCRED peer credential authentication with permission checking
- SSE event streaming for real-time operation progress
- Persistent job queue in SQLite (survives daemon restart, schema v35)
- Priority-based operation queue
- Systemd socket activation and watchdog integration (`sd-notify`)
- CLI auto-forwarding: commands detect and forward to daemon when available
- Full REST API: packages, search, dependencies, transactions, history, events

#### CCS Native Package Format
- Content-addressable package format with gzipped tar, CBOR manifests, and Merkle tree verification
- Ed25519 package signing and verification (`ccs-keygen`, `ccs-sign`, `ccs-verify`)
- Content-defined chunking via FastCDC (16KB min, 64KB avg, 256KB max) with `--chunked` flag
- Cross-package chunk deduplication -- implicit delta updates without pre-computation
- Build policy engine: DenyPaths, NormalizeTimestamps, StripBinaries, FixShebangs, CompressManpages
- SOURCE_DATE_EPOCH support for reproducible builds
- OCI container export (`ccs-export --format oci`) compatible with podman and docker
- Dev shells (`ccs shell`) for temporary environments without permanent install
- One-shot execution (`ccs run <package> -- <command>`)
- Lockfile support for dependency pinning
- Package redirects for renames and obsoletes (schema v28)
- Per-package resolution routing: binary, remi, recipe, or delegate strategies (schema v29)

#### CAS Federation
- Distributed chunk sharing across Conary nodes for bandwidth savings
- Hierarchical peer topology: leaf -> cell hub -> region hub
- mDNS service discovery (`_conary-cas._tcp.local`) for LAN peers
- Request coalescing to deduplicate concurrent identical chunk requests
- Circuit breaker pattern for failing peers with automatic recovery
- Ed25519-signed manifests for chunk list integrity
- Per-tier allowlists for access control
- Daily statistics tracking with bandwidth savings reports (schema v34)
- CLI: `federation status`, `peers`, `add-peer`, `test`, `scan`, `stats`

#### System Model
- Declarative OS state management in TOML format
- Remote model includes with Remi API resolution
- Model diff engine comparing declared state against current system
- Model apply for atomic state convergence
- Model check with exit codes for CI/CD drift detection
- Model snapshot to capture current system as a model file
- Model publishing with Ed25519 signatures
- Lockfile generation for pinning exact versions across includes
- Version pinning via `[pin]` section with glob patterns

#### Recipe System
- Build packages from source using TOML recipe files
- Kitchen abstraction for isolated build environments
- Variable substitution: `%(version)s`, `%(destdir)s`, `%(builddir)s`
- Recipe validation with warnings for common issues
- PKGBUILD converter for importing Arch Linux recipes
- Recipe resolution strategy: resolver can fetch and cook recipes automatically

#### Hermetic Builds
- Network-isolated build environments using Linux namespaces (PID, UTS, IPC, mount, network)
- Two-phase builds: fetch phase (network allowed) then build phase (network blocked)
- `CLONE_NEWNET` for network isolation (only loopback available during build)
- Dependency-hash-based cache invalidation (BuildStream-grade reproducibility)
- CLI flags: `--hermetic` (maximum isolation), `--fetch-only` (pre-fetch), `--no-isolation` (disable)

#### Capability Enforcement
- Package capability declarations for network, filesystem, and syscall access
- Landlock filesystem restrictions
- seccomp-bpf syscall filtering
- Capability auditing and inference for existing packages
- Schema v33: `capabilities` and `capability_audits` tables

#### Package Provenance (DNA)
- Full provenance tracking: source origin, build environment, signatures, content hashes
- Sigstore integration for provenance signing and verification
- Schema v32: `provenance_sources`, `provenance_builds`, `provenance_signatures`, `provenance_content`, `provenance_verifications` tables
- CycloneDX 1.5 SBOM export (`query sbom`)

#### Retroactive CCS Enhancement
- Background capability inference for converted legacy packages
- Subpackage relationship tracking (schema v36)
- Parallel binary analysis with goblin
- Lazy enhancement triggered on package access
- 26 integration tests for the enhancement pipeline

#### Dependency Resolution
- SAT-based dependency resolver using resolvo, replacing the hand-rolled graph solver
- Typed dependency kinds: Package, Soname, Python, Perl, Ruby, Java, PkgConfig, CMake, Binary, File, Interpreter, Abi, KernelModule (schema v19)
- `kind(target)` syntax: `pkgconfig(zlib)`, `python(flask)`, `soname(libssl.so.3)`
- Self-contained provides database for cross-distro resolution
- Atomic multi-package dependency installation

#### Labels and Federation
- Label format: `repository@namespace:tag` with parsing, validation, and wildcards
- Label path with priority-based search ordering
- Label federation: labels delegate to other labels or link to repositories (schema v30)
- Delegation chains with cycle detection
- CLI: `label-list`, `label-add`, `label-remove`, `label-path`, `label-show`, `label-set`, `label-query`, `label-link`, `label-delegate`

#### Trigger System
- Path-pattern-based triggers for post-installation actions
- DAG ordering with before/after dependencies (Kahn's algorithm)
- 10 built-in triggers: ldconfig, update-mime-database, gtk-update-icon-cache, systemd-reload, fc-cache, depmod, glib-compile-schemas, update-desktop-database, texhash, mandb
- CLI: `trigger-list`, `trigger-show`, `trigger-enable`, `trigger-disable`, `trigger-add`, `trigger-remove`, `trigger-run`

#### System State Snapshots
- Numbered system states with timestamp and summary (schema v18)
- Automatic snapshots after install/remove operations
- State diff: compare any two snapshots
- State restore: compute and apply operations to revert
- State pruning for garbage collection of old snapshots
- CLI: `state-list`, `state-show`, `state-diff`, `state-restore`, `state-prune`, `state-create`

#### Configuration Management
- Config file tracking from RPM `%config`, DEB `conffiles`, Arch `backup` (schema v21)
- Config backup to CAS before modification
- Config restore from backup with pre-restore safety copy
- Config diff between installed and package versions
- `noreplace` support to preserve user modifications
- CLI: `config-list`, `config-diff`, `config-backup`, `config-restore`, `config-check`, `config-backups`

#### Security Updates
- `--security` flag for security-only updates
- Security metadata: severity, CVE IDs, advisory info on repository packages (schema v22)
- `update-group` command for atomic collection updates

#### Container-Isolated Scriptlets
- Namespace isolation (mount, PID, IPC, UTS) for package install scripts
- Chroot filesystem isolation
- Bind mounts with read-only defaults for controlled host access
- Rootless fallback with resource limits (CPU, memory, file size, process count)
- Dangerous script detection with automatic risk analysis
- Cross-distro scriptlet support (RPM, DEB, Arch calling conventions)
- `--sandbox` flag: auto, always, never

#### Bootstrap System
- Stage 1 bootstrap builder for cross-compiling a minimal system
- Base system builder for constructing a complete Conary-managed OS
- Bootable image generation
- Core bootstrap recipes for essential system packages

#### Mirror and Download Infrastructure
- Mirror selection and failover across multiple download sources
- Reference mirrors: split metadata (trusted) from content (CDN) via `--content-url` (schema v24)
- Exponential backoff with jitter for HTTP retries
- Parallel downloads via rayon with aggregate progress reporting
- Parallel repository sync across multiple repositories

#### Other Additions
- Collections/groups for bulk package operations (schema v13)
- Component model: automatic file classification into :runtime, :lib, :devel, :doc, :config, :debuginfo, :test (schema v11)
- Enhanced flavor support with operators: `~` (prefers), `!` (not), `~!` (prefers not), architecture flavors (schema v14)
- Package pinning to prevent updates/removal (schema v15)
- Selection reason tracking: explicit, dependency chain, collection attribution (schema v16)
- Install reason tracking for autoremove support (schema v12)
- Orphan detection and autoremove
- CAS garbage collection (`system gc`)
- System adoption: scan and track packages installed by RPM/APT
- `repquery` for querying repository packages (not just installed)
- Path query: `conary query --path /usr/bin/foo`
- SBOM export in CycloneDX 1.5 format
- Shell completions for Bash, Zsh, Fish, PowerShell
- Auto-generated man pages
- Dry run mode for all destructive operations
- Hybrid mode for coexistence with system package managers

### Changed

- Database schema upgraded from v5 to v36 (40+ tables, 30+ migrations)
- Dependency resolver replaced with SAT-based resolvo (from hand-rolled graph solver)
- Unified package parser: single interface for RPM, DEB, and Arch formats
- Unified decompression: Gzip, Xz, and Zstd with automatic format detection via magic bytes
- Transaction engine uses UUID-based crash recovery correlation (schema v23)
- Repository sync is 40x faster via batch inserts and transactions
- `conary update` with no args now updates all packages

### Fixed

- Stale in-flight entries in download manager causing hangs on retry
- Backoff overflow on high retry counts (arithmetic overflow in exponential calculation)
- Atomic multi-package dependency installation (packages installed as a unit, not individually)
- RPM version normalization for correct epoch:version-release comparison
- File conflict detection for upgrades of adopted packages
- FK constraint errors when removing adopted packages
- Directory removal on package uninstall
- Symlink handling in adoption and file restore
- DEB scriptlet arguments conforming to Debian Policy
- Arch upgrade scriptlet function selection (pre_upgrade vs post_upgrade)
- Excluded package duplicate removal in model apply

### Removed

- 1,504 lines of dead code and redundant logic across two simplification passes
- Legacy install code path (replaced by unified pipeline)
- Duplicate archive extraction logic (centralized in compression module)

## [0.1.0] - 2025-06-01

### Added

- Initial package management: install, remove, update, rollback
- SQLite-backed state management with schema migrations
- RPM and DEB package parsing with full metadata extraction
- Dependency resolution with topological sort and cycle detection
- Content-addressable file storage with SHA-256 integrity
- Basic repository sync with HTTP downloads
- File-level tracking with ownership and permissions
- Changeset-based atomic transactions
