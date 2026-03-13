# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [v0.5.0] - 2026-03-13

### Other
- refactor(conary): remove repo solving stopgaps after native normalization
- test(core): add cross-distro SAT and policy regression coverage
- feat(core): make installed solver inputs version-scheme aware
- feat(core): load native repo semantics into SAT provider
- feat(core): add seamless cross-distro root package selection
- feat(core): query normalized repo capabilities for dependency resolution
- feat(core): persist native repo semantics during sync
- feat(core): normalize arch repo semantics
- feat(core): normalize debian repo semantics
- feat(core): normalize fedora repo semantics
- feat(core): persist native repo semantics and version schemes
- feat(core): add native repo semantics and resolution policy types
- fix(model): address spec compliance gaps in source policy
- feat(model): promote source policy to first-class Conary contract
- fix(core): skip conditional repo rpm dependencies in SAT requests
- fix(core): allow repo capability lookup beyond virtual heuristics
- fix(core): resolve repo capability deps by provide version
- fix(core): preserve repo dependency constraints in SAT resolution
- fix(conary): promote grouped remi deps in satisfy mode
- fix(core): allow larger remi metadata responses
- fix(server): include package metadata in remi index
- fix(conary): install repo-resolvable converted deps in satisfy mode
- fix(core): parse namespaced fedora metadata entries
- fix(conary): surface unresolved converted deps
- fix(core): resolve kernel capability deps from repo metadata
- fix(phase3): harden group m real-world operations
- fix(phase3): harden group j dependency edge coverage
- fix(phase3): harden group i security boundary coverage
- fix(phase3): harden group h recovery coverage
- fix(phase3): harden group g integrity coverage
- fix(conary): harden remi ccs dependency resolution
- fix(conary): honor self-provided ccs dependencies
- fix(conary): resolve converted ccs deps from package metadata
- fix(core): map repo capability deps to package names
- fix(conary): resolve dependencies for remi ccs installs
- fix(core): make fuzzy provide checks honor soname suffixes
- fix(conary): use fuzzy provide matching for ccs dependencies
- fix(core): match rpm soname provides during dependency checks

## [v0.4.0] - 2026-03-11

### Fixed
- resolve clippy warnings and test compilation after Phase 4

### Other
- refactor(cli): add #[must_use] on query functions
- refactor(cli): add #[must_use] on query functions
- docs(db): document changeset metadata field
- docs(canonical): add TODO comments for unused NameHints fields
- fix(bootstrap): use absolute paths for build tools
- fix(bootstrap): sanitize PATH in bootstrap build environment
- refactor(db): remove unused TROVE_COLUMNS_PREFIXED constant
- docs(label): document Label wildcard matching semantics
- refactor(trust): add cfg(target_os) guards for platform-specific code
- refactor(provenance): rename DnaHashError::ShortInput to InputTooShort
- refactor(trust): use typed ParseRoleError for TUF role parsing
- refactor(repository): remove unnecessary Debug derives from internal types
- fix(ccs): handle non-UTF-8 root path in user_group hook
- refactor(ccs): compute DEB relative path once instead of twice
- refactor(db): move format_size utility to conary-core::util module
- refactor(repository): rename isize_val to installed_size_str
- fix(recipe): update test to use new suggest_bootstrap_edges signature
- fix(capability): use Path::starts_with() for path prefix matching
- feat(capability): add aarch64 syscall mappings for seccomp enforcement
- refactor(capability): deduplicate syscall profile lists
- feat(recipe): make RecipeGraph bootstrap edges configurable
- fix(recipe): handle strings and comments in PKGBUILD brace counting
- perf(recipe): use LazyLock for PKGBUILD regex patterns
- perf(recipe): stream file hashing in provenance capture
- perf(recipe): eliminate double CCS build in plate()
- refactor(bootstrap): extract shared PackageBuildRunner from stage1/stage2/base
- fix(packages): return Result from get_file_metadata
- refactor(cli): use ValueEnum for SandboxMode and DepMode
- fix(packages): propagate file open errors in detect_format
- docs(transaction): document symlink validation asymmetry in recovery
- security(derived): validate override target paths
- fix(transaction): fix BackupInfo size conversion
- fix(resolver): remove trailing newline from ConflictingConstraints display
- refactor(provenance): remove unused ContentProvenanceBuilder
- fix(canonical): replace expect with error propagation in repology
- refactor(ccs): define BuilderError thiserror enum
- refactor(repository): remove dead alternative handling code in debian parser
- refactor(trust): use TrustResult in ceremony functions
- perf(self_update): stream download through hasher
- fix(transaction): clean orphaned journal files after recovery
- fix(repository): add per-chunk retry for transient download errors
- fix(self_update): log warnings on file read failures
- fix(model): populate model_hash in lockfile
- fix(cli): fix model check exit code on validation failure
- fix(trust): preserve file path context in verify_file errors
- fix(repository): add retry logic for transient errors in poll_for_completion
- docs(db): document FileEntry::insert_or_replace ownership semantics
- docs(db): add TODO for StateDiff streaming optimization
- docs(db): document model pattern inconsistency
- fix(db): log warnings on Trove::from_row parse fallbacks
- fix(db): use valid JSON default for ConvertedPackage fields
- docs(db): document transaction requirement for batch_insert
- fix(db): wrap DownloadStat::insert_batch in transaction
- refactor(cli): deduplicate format_bytes to shared utility
- fix(filesystem): log warning when symlink deployment skips existing directory
- perf(packages): cache DEB data tarball to avoid double extraction
- chore(packages): remove unused warn import from arch.rs
- perf(resolver): add HashMap index for version set lookup
- perf(transaction): cache hash computation in planner
- refactor(resolver): convert Conflict enum to thiserror
- fix(dependencies): fix is_lib_file .so substring false positive
- refactor(cli): remove blanket dead_code allow from progress module
- perf(cli): consolidate DB opens in cmd_install
- refactor(core): remove duplicate error variants
- perf(packages): single-pass Arch package parsing
- refactor(filesystem): migrate fsverity errors to thiserror
- fix(dependencies): error on version parse failure instead of string fallback
- fix(automation): use AtomicBool for daemon stop flag
- fix(dependencies): approximate soname by stripping minor/patch version
- fix(self_update): handle pre-release versions in is_newer comparison
- fix(version): normalize epoch and release for exact version matching
- fix(version): implement RPM-compatible version comparison
- fix(cli): replace expect on Tokio runtime with error propagation
- fix(container): capture stdout/stderr in fork-based isolation
- fix(provenance): use try_from for timestamp i64->u64 conversion
- fix(cli): replace expect/unwrap with proper error propagation
- fix(model): fix diamond include false positive in cycle detection
- fix(bootstrap): compare file contents in reproducibility check
- fix(cli): replace process::exit calls with proper error returns
- fix(bootstrap): return error from current_stage when all stages complete
- fix(ccs): handle mode "0" in directory hook
- fix(ccs): use deterministic hash for tmpfiles config naming
- fix(recipe): verify cached artifact checksums on retrieval
- fix(db): fix format_permissions symlink detection bitmask
- fix(db): use parameter binding for LIMIT in audit_log query
- fix(db): remove format-based SQL column injection in Changeset::update_status
- fix(db): make DistroPin::set atomic
- fix(db): use recursive CTE for transitive orphan detection
- fix(resolver): handle version set pool overflow gracefully
- fix(bootstrap): replace expect with error propagation for path validation
- fix(resolver): replace unwrap with if-let in dependency graph
- fix(resolver): handle missing canonical_id instead of defaulting to 0
- fix(update): don't create changeset when no updates needed
- fix(resolver): replace expect with error propagation in canonical resolution
- fix(capability): fix operator precedence in server package detection
- fix(remove): iterate autoremove to fixed point
- docs(repository): fix download_chunks doc to match sequential implementation
- fix(repository): detect system architecture instead of hardcoding x86_64
- fix(capability): improve port 0 rejection error message
- security(recipe): require checksums for remote patches
- fix(capability): reject port 0 in validate_port_spec
- security(recipe): warn on remote patches without checksums
- security(packages): reduce CPIO max file size allocation
- security(packages): add package size validation to Arch and Debian parsers
- security(repository): add path traversal validation to Arch package parser
- security(ccs): validate sysctl key and value before writing config
- security(ccs): validate alternative name and path in hooks
- fix(transaction): map FileMoved/FileRemoved to FsApplied state for correct recovery
- fix(model): replace expect() with error propagation in canonical_json
- fix(container): read pipes directly after wait_timeout instead of double-wait
- fix(trust): wrap TUF persist operations in database transaction
- security(trust): fix TUF root rotation to happen before metadata verification per spec 5.3
- fix(trust): make canonical_json return Result instead of panicking
- fix(erofs): convert normalize_path to return Result instead of panicking
- security(packages): add size limit to DEB archive member extraction
- security(ccs): add size limits and hex validation to archive extraction
- docs(bootstrap): fix expand_env_vars docstring to match hermetic behavior
- fix(bootstrap): use atomic write-then-rename for state file persistence
- security(bootstrap): fix shell injection in stage0 ct-ng invocation
- security(repository): validate URL scheme in RemiClient constructor
- fix(resolver): replace unreachable!() with safe default in resolve_condition
- fix(install): replace unwrap on canonical.id with proper error
- fix(cli): use checked multiplication for cache_max_bytes to prevent overflow
- fix(remove): commit DB changes before file deletion for crash safety
- fix(install): guard against short hash before slicing in single-install path
- fix(db): pass transaction ref to migration functions instead of bare connection
- fix(db): use SELECT after upsert instead of last_insert_rowid
- fix(bootstrap): Stage 1 builds successfully on Remi
- fix(bootstrap): disable GCC plugins for static toolchain, drop root for ct-ng
- refactor(bootstrap): remove hardcoded versions, fix ct-ng config and stage0
- feat(bootstrap): implement build_rust() and build_conary() for Tier C
- feat(bootstrap): add initramfs generation and sysroot population
- feat(bootstrap): add per-package and per-tier build mode

## [server-v0.3.1] - 2026-03-11

### Fixed
- resolve clippy warnings and test compilation after Phase 4

### Other
- docs(server): document DaemonEvent variants
- refactor(server): clean up CircuitBreakerRegistry
- feat(server): add Action::Enhance audit variant
- docs(server): document latency duration casts
- refactor(server): extract shared test_app helper
- refactor(server): extract shared test_app helper for admin handler tests
- fix(server): reject empty scope strings in token validation
- refactor(server): convert remaining errors to thiserror
- fix(server): only count 401/403 in ban middleware
- fix(server): wrap readiness check in spawn_blocking
- perf(server): cache scan_versions result with TTL
- fix(daemon): add limit to list_transactions with status filter
- perf(federation): parallelize local cache lookups in fetch_many
- fix(server): replace expect in canonical_bytes with error propagation
- fix(server): use OsRng instead of thread_rng for token generation
- fix(server): fix wildcard matching to require subdomain
- refactor(daemon): deduplicate SO_PEERCRED extraction
- perf(federation): use Arc<Peer> in PeerRegistry::all
- fix(server): replace deprecated rand::thread_rng
- perf(server): debounce auth token touch calls
- fix(daemon): fix TOCTOU race in job cancellation
- fix(daemon): use parameter binding in list_all query
- fix(server): improve MCP error code mapping
- fix(server): add timeout to Forgejo HTTP requests
- refactor(server): convert ForgejoError to thiserror
- refactor(server): convert ServiceError to thiserror
- fix(server): truncate request body in audit middleware
- fix(server): redact filesystem paths from server_info endpoint
- fix(server): validate date format in purge_audit
- fix(server): wrap update_repo in database transaction
- fix(server): normalize hashes in find_missing chunk handler
- fix(daemon): use open_fast for WAL mode and proper pragmas
- security(daemon): add warning when TCP listener exposes unauthenticated read endpoints
- security(server): add localhost-only check to internal admin API
- security(server): fix auth rate limiting to check before consuming token
- fix(server): normalize hash in OCI blob handler to prevent cache bypass
- fix(server): use tokio::fs::read for async CCS file serving
- fix(server): wrap check_converted DB call in spawn_blocking
- refactor(conary-test): simplify crate — deduplicate helpers, hoist workspace deps

## [erofs-v0.1.2] - 2026-03-11

### Fixed
- unused BuildStats must_use in erofs builder tests

### Other
- docs(erofs): document cast_possible_truncation allowances
- chore(erofs): remove unused dependencies
- fix(erofs): add bounds check for dirent nameoff u16
- fix(erofs): write padding in chunks to handle block sizes larger than 4096
- fix(erofs): convert normalize_path to return Result instead of panicking
- fix(erofs): define ErofsError type, convert Superblock::new to return Result

## [v0.3.0] - 2026-03-08

### Fixed
- **P0**: Negative duration cast wrapping to `u64::MAX` in transaction journal metadata (clock skew/NTP)
- **P1**: CPIO header field parsing with safe u32 conversion (malformed archive protection)
- **P1**: Host environment leaking into hermetic bootstrap builds via `expand_env_vars()` fallback
- **P1**: 14 `expect()` calls in bootstrap pipeline replaced with proper error propagation
- **P1**: Resolver pool index casts guarded with `u32::try_from()` (7 sites)
- **P2**: Recovery symlink validation aligned with staging bounds checking (install-root escape)
- **P2**: Repology client `expect()` replaced with `Result` propagation
- **P2**: Progress bar template `expect()` calls replaced with fallback styles (5 sites)
- **P2**: CAS `hash_to_path` rejects short hashes instead of producing malformed paths

### Security
- Add missing checksum verification in bootstrap stage 2

### Other
- Four codebase simplify passes — DRY helpers, bug fixes, deduplication, idiomatic Rust
- Add Claude Code hooks for sensitive file protection and auto-clippy
- cargo fmt (Rust 2024 edition formatting)

## [server-v0.3.0] - 2026-03-08

### Fixed
- **P1**: `content_url` not validated in `create_repo` handler
- **P1**: `url` not validated in `update_repo` handler
- **P3**: Body field name validation aligned with path parameter validation
- **P3**: Forgejo repo path extracted to constant (13 occurrences across 3 files)

### Security
- Add missing checksum verification in bootstrap stage 2

### Other
- Four codebase simplify passes — DRY helpers, bug fixes, deduplication
- cargo fmt (Rust 2024 edition formatting)

## [server-v0.2.1] - 2026-03-07

### Fixed
- codebase-wide simplify pass — bug fixes, deduplication, performance

### Other
- refactor(server): wire MCP tools to shared service layer, remove duplication
- refactor(server): extract admin service layer for shared business logic
- refactor(server): split admin.rs into domain files (tokens, ci, repos, federation, audit, events)
- perf(server): move rate limiters out of RwLock, reduce per-request overhead
- refactor(server): extract Forgejo client into shared module
- refactor(server): replace string scopes with Scope enum
- perf(db): add open_fast() to skip migrations on server hot paths
- refactor(server): Simplify admin API — fix security, dedup, efficiency
- feat(server): Add Remi Admin API P2 — rate limiting, audit logging
- feat(server): Add Remi Admin API P1 — repo management, federation, MCP tools
- feat(server): Add Remi Admin API P0 — auth, tokens, CI proxy, SSE, OpenAPI, MCP

## [v0.2.1] - 2026-03-07

### Fixed
- codebase-wide simplify pass — bug fixes, deduplication, performance

### Other
- feat(db): add federation_peer model to replace raw SQL in handlers
- perf(db): add open_fast() to skip migrations on server hot paths
- feat(server): Add Remi Admin API P2 — rate limiting, audit logging
- feat(server): Add Remi Admin API P0 — auth, tokens, CI proxy, SSE, OpenAPI, MCP

## [server-v0.2.0] - 2026-03-07

### Added
- External admin API on :8082 with bearer token authentication and scope-based authorization
- Token CRUD endpoints (create/list/delete) with SHA-256 hashing
- CI proxy handlers for Forgejo integration (workflows, runs, dispatch, mirror-sync)
- SSE event stream for real-time admin notifications
- OpenAPI 3.1 spec at /v1/admin/openapi.json
- MCP endpoint at /mcp via rmcp with 16 admin tools for LLM agent integration
- Repository management endpoints (CRUD + sync trigger)
- Federation peer management endpoints (list/add/remove + config)
- Per-IP rate limiting via governor (read 60/min, write 10/min, auth-fail 5/min)
- Audit logging middleware with query and purge endpoints
- DB migration v47: admin_tokens table
- DB migration v48: admin_audit_log table
- Add Remi canonical metadata API endpoints
- Add standalone remi and conaryd binaries

### Fixed
- Apply all code review findings across 8 batches
- P1/P2 findings in filesystem, canonical, and model modules
- P1/P2 findings in remi server, daemon, and federation
- Address crate split review findings

### Other
- feat(server): add self-update Remi endpoints
- build: Add workspace.dependencies to deduplicate version specs
- build: Create conary-server crate skeleton

## [erofs-v0.1.1] - 2026-03-07

### Fixed
- Apply all code review findings across 8 batches

### Other
- build: Add workspace.dependencies to deduplicate version specs
- fix(erofs): Remove dead chunk indexes, fix mode passthrough, fix probe race
- fix(erofs): Fix inode field layout, wire fs-verity, unmount old composefs
- fix(erofs): Set DEVICE_TABLE feature flag in composefs images
- feat(erofs): Add EROFS image verification
- feat(erofs): Add high-level ErofsBuilder API
- feat(erofs): Add tail-end packing for small files
- feat(erofs): Add LZ4 and LZMA metadata compression
- feat(erofs): Add xattr support for composefs digests
- feat(erofs): Add chunk-based external data references
- feat(erofs): Add directory entry packing
- feat(erofs): Add compact and extended inode layouts
- feat(erofs): Add EROFS superblock structure
- feat(erofs): Initialize conary-erofs workspace crate

## [v0.2.0] - 2026-03-07

### Added
- add update-channel management commands
- wire up complete self-update command
- add atomic binary replacement for self-update
- add self-update download, extract, and verify
- add self-update version check logic
- add self-update CLI command (stub)
- Add CAS storage and upgrade rollback to CCS install
- Add script hooks and changeset tracking to CCS install
- Add curated canonical rules, distros.toml, and registry CLI
- Add --from flag to 'conary install' for cross-distro override
- Add distro, canonical, and groups CLI commands
- Add DB-backed canonical mapping for CCS legacy capabilities
- Add canonical conflict detection for equivalent packages
- Add CanonicalResolver with pinning, ranking, and mixing policy
- Wire canonical discovery into repository sync pipeline
- Add AppStream catalog parser for canonical identity
- Add Repology API client for canonical registry bootstrap
- Add canonical rules engine and multi-strategy auto-discovery
- Add distro pin and package overrides to system model parser
- Add canonical, distro pin, and system affinity DB models
- Add schema migration v45 for canonical package identity

### Fixed
- Resolve skipped integration tests (composefs, generations, hermetic)
- Resolve FK constraint failure and pre_remove hook in CCS install
- Remove nested transaction in batch_insert causing repo sync failure
- Resolve 6 clippy warnings for clean CI
- Apply all code review findings across 8 batches
- Address 3 regressions found by Codex review
- Remove duplicate code blocks introduced during P0 security fixes
- P1/P2 findings in packages, resolver, and db modules
- P1/P2 findings in filesystem, canonical, and model modules
- P1/P2 findings in install, remove, update, adopt, and system commands
- P1/P2 findings in remi server, daemon, and federation
- Address Codex review findings for cross-distro canonical mapping
- Wrap AppStream ingestion in transaction for atomicity
- Harden Repology client — User-Agent, error_for_status, URL encoding
- Bootstrap CLI bugs found by Codex review
- Address crate split review findings

### Security
- Fix all P0 critical findings from feature review
- Fix all P0 critical findings from feature review

### Other
- feat(db): add key-value settings table (migration 46)
- bootstrap: Add dry-run validation, --skip-verify flag, complete resume logic
- bootstrap: Add systemd-repart image builder with rootless support
- bootstrap: Implement Stage 2 (reproducibility rebuild)
- bootstrap: Graph-ordered base system with per-package checkpoints
- bootstrap: Add Stage 1 LFS 12.4 recipe files with real checksums
- bootstrap: Enforce checksums, reject placeholders unless --skip-verify
- bootstrap: Implement Stage 0 seed caching
- bootstrap: Add version detection, update to LFS 12.4 defaults
- build: Add workspace.dependencies to deduplicate version specs
- build: Move shared dependencies to conary-core
- build: Create conary-core crate skeleton
- fix(erofs): Remove dead chunk indexes, fix mode passthrough, fix probe race
- fix(erofs): Fix inode field layout, wire fs-verity, unmount old composefs
- feat(generation): Update GC and info for EROFS/composefs format
- feat(generation): Replace renameat2 with composefs mount switching
- feat(fs): Add fs-verity enablement for CAS objects
- feat(generation): Rewrite builder to produce EROFS images
- feat(generation): Add composefs detection and preflight
- fix(install): Address Codex review of dependency resolution
- fix(generation): Address Codex review findings
- fix(generation): Address code review findings
- fix(generation): Correct format_bytes test assertion for sub-KiB values
- feat(cli): Wire generation and takeover commands into CLI
- feat(generation): Add conary system takeover command
- feat(generation): Add list, info, and gc commands
- feat(generation): Add BLS boot entries with GRUB fallback
- feat(generation): Add atomic switch via renameat2(RENAME_EXCHANGE)
- feat(generation): Add generation builder — reflink files from CAS
- feat(generation): Add generation metadata types and path helpers
- feat(fs): Add reflink support with fallback to hardlink/copy
- feat(install): Add dependency resolution with dep-mode control
- fix(server): Deduplicate Remi sync, aggregate multi-repo metadata, fix conversion patterns
- fix(server): Correct distro count query and search facet storage

## [0.1.0] - 2026-03-03

Major release covering 10 months of development. Every subsystem listed below is implemented and tested.

### Added

#### Remi Server (feature-gated: `--features server`)
- On-demand CCS conversion proxy that converts RPM/DEB/Arch packages to CCS format when requested
- Chunk-level content-addressable storage with LRU eviction and access tracking (schema v27, v38)
- Server-side conversion caching with package identity tracking for restarts (schema v38)
- Download statistics with aggregated popularity rankings per distro (schema v40)
- 202 Accepted pattern for async conversion with job polling
- Bloom filter acceleration for chunk existence checks
- Batch endpoints for multi-chunk requests
- Sparse index for efficient client metadata sync
- Full-text package search powered by Tantivy
- Pull-through caching from upstream repositories
- Cloudflare R2 write-through for chunk storage
- Remi-native repository sync via `/v1/{distro}/metadata` API
- Public package index with search at packages.conary.io
- TUF supply chain trust with timestamp, snapshot, targets, and root role delegation (schema v43)
- Mirror health tracking with latency, throughput, and composite scoring (schema v44)
- Pre-computed delta manifests between package versions (schema v44)
- CORS restrictions, token-bucket rate limiting, audit logging, and configurable ban lists
- Prometheus metrics export at `/v1/admin/metrics/prometheus`
- Podman-based integration test harness

#### conaryd Daemon (feature-gated: `--features server`)
- Local REST API for package operations over Unix socket (`/run/conary/conaryd.sock`)
- Optional TCP listener for remote management
- SO_PEERCRED peer credential authentication with permission checking
- Auth gate middleware rejecting unauthenticated POST/PUT/DELETE on v1 router
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
- Remote model includes with Remi API resolution (schema v41, v42)
- Cryptographic verification of remote collections with Ed25519 signatures
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
- seccomp-bpf syscall filtering with dedicated scriptlet profile (~90 allowed syscalls)
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
- Enhancement priority scheduling for lazy processing (schema v37)
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
- Native chroot with seccomp enforcement in pre_exec (replaces external chroot command)

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
- Orphan detection and autoremove with grace period tracking (schema v39)
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

- Database schema upgraded from v5 to v44 (44 migrations across 40+ tables)
- Dependency resolver replaced with SAT-based resolvo (from hand-rolled graph solver)
- Unified package parser: single interface for RPM, DEB, and Arch formats
- Unified decompression: Gzip, Xz, and Zstd with automatic format detection via magic bytes
- Transaction engine uses UUID-based crash recovery correlation (schema v23)
- Repository sync is 40x faster via batch inserts and transactions
- `conary update` with no args now updates all packages

### Fixed

- TOCTOU race in file deployer: CAS inode reference held during hardlink, copy reads from open fd (5714985)
- Double-wait bug in scriptlet execution causing ECHILD errors (5714985)
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

## [0.0.1] - 2025-06-01

### Added

- Initial package management: install, remove, update, rollback
- SQLite-backed state management with schema migrations
- RPM and DEB package parsing with full metadata extraction
- Dependency resolution with topological sort and cycle detection
- Content-addressable file storage with SHA-256 integrity
- Basic repository sync with HTTP downloads
- File-level tracking with ownership and permissions
- Changeset-based atomic transactions
