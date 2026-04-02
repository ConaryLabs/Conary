# Conary Architecture Reference

## Workspace Structure

Virtual workspace: `apps/conary` (CLI), `apps/remi` (Remi), `apps/conaryd` (daemon), `apps/conary-test` (test infrastructure), and `crates/conary-core` (shared library). EROFS support uses `composefs-rs` directly in `conary-core`.

## Key Modules

### conary (root) -- CLI binary

| Module | Purpose |
|--------|---------|
| `src/cli/` | CLI definitions (verify.rs for verify-derivation commands) |
| `src/commands/` | Command implementations (verify.rs, derivation_sbom.rs for Phase 6) |

### conary-core -- Core library

| Module | Purpose |
|--------|---------|
| `src/db/` | SQLite schema, models, migrations |
| `src/packages/` | RPM/DEB/Arch parsers (unified via `common.rs` PackageMetadata) |
| `src/compression/` | Unified decompression (Gzip, Xz, Zstd) with format detection |
| `src/repository/` | Remote repos, metadata sync, mirror health, metalink, substituters |
| `src/resolver/` | SAT-based dependency resolution (resolvo) |
| `src/filesystem/` | CAS, VFS tree, fsverity, path utilities |
| `src/delta/` | Binary delta updates (zstd dictionary compression) |
| `src/version/` | Version parsing, constraints |
| `src/container/` | Scriptlet sandboxing, namespace isolation |
| `src/trigger/` | Post-install trigger system |
| `src/scriptlet/` | Scriptlet execution, cross-distro support |
| `src/label.rs` | Package provenance labels |
| `src/flavor/` | Build variation specs |
| `src/components/` | Component classification |
| `src/dependencies/` | Dependency type definitions, provider matching |
| `src/derived/` | Derived package builder |
| `src/transaction/` | Composefs transaction engine (resolve/fetch/commit/build/mount) |
| `src/generation/` | EROFS generation building, composefs mounting, /etc merge, CAS GC, deltas |
| `src/model/` | System Model - declarative OS state |
| `src/ccs/` | CCS native package format, builder, policy engine, OCI export |
| `src/recipe/` | Recipe system for building packages from source |
| `src/capability/` | Capability declarations - audit, enforcement, inference |
| `src/provenance/` | Package DNA / full provenance tracking |
| `src/trust/` | TUF supply chain trust |
| `src/automation/` | Automated maintenance (security updates, orphan cleanup) |
| `src/bootstrap/` | Bootstrap a complete Conary system (6-phase pipeline) |
| `src/canonical/` | Cross-distro canonical name mapping (AppStream, Repology) |
| `src/self_update.rs` | Self-update version checking, download, atomic replacement |
| `src/hash.rs` | Multi-algorithm hashing (SHA-256, XXH128) |
| `src/repository/dependency_model.rs` | Cross-distro normalized dependency/provide types |
| `src/repository/versioning.rs` | Scheme-aware version comparison (RPM, Debian, ALPM) |
| `src/repository/resolution_policy.rs` | Policy types for request scope, mixing, and candidate filtering |
| `src/derivation/` | CAS-layered derivation engine for bootstrap (19 files: pipeline, compose, capture, build_order, executor, environment, convergence, graph, index, etc.) |
| `src/mcp/` | MCP tool definitions for LLM integration |
| `src/model/replatform.rs` | Source policy replatform and convergence planning |

### remi + conaryd -- service-owned app crates

| Module | Purpose |
|--------|---------|
| `apps/remi/src/server/` | Remi server |
| `apps/remi/src/server/auth.rs` | Admin API auth -- token hashing, Scope enum, bearer extraction, middleware |
| `apps/remi/src/server/admin_service.rs` | Shared service layer for admin ops (tokens, repos, federation, audit) |
| `apps/remi/src/server/forgejo.rs` | Shared Forgejo/CI client (get, post, get_text helpers) |
| `apps/remi/src/server/mcp.rs` | MCP server endpoint -- LLM tool integration via rmcp |
| `apps/remi/src/server/rate_limit.rs` | Per-IP rate limiting middleware (governor, token buckets) |
| `apps/remi/src/server/audit.rs` | Audit logging middleware with action derivation |
| `apps/remi/src/server/routes.rs` | Axum router construction (internal :8081 + external :8082) |
| `apps/remi/src/server/handlers/admin/` | Admin API handlers -- tokens, ci, repos, federation, audit, events, artifacts, packages, test_data |
| `apps/remi/src/server/handlers/openapi.rs` | OpenAPI 3.1 spec endpoint for admin API |
| `apps/remi/src/server/handlers/self_update.rs` | Self-update endpoints (`/v1/ccs/conary/latest`, `/versions`, `/download`) |
| `apps/remi/src/federation/` | CAS federation - peer discovery, chunk routing, mTLS |
| `apps/conaryd/src/daemon/` | conaryd daemon - REST API, SSE events, job queue, systemd |

### conary-test -- Test infrastructure

| Module | Purpose |
|--------|---------|
| `src/config/` | TOML manifest and distro config parsing |
| `src/engine/runner.rs` | Test runner -- executes manifests against containers |
| `src/engine/executor.rs` | Step executor -- StepAction enum, per-step execution |
| `src/engine/variables.rs` | Variable substitution engine (`${VAR}` expansion) |
| `src/engine/container_coordinator.rs` | Container lifecycle orchestration and cleanup |
| `src/engine/suite.rs` | TestSuite, TestResult, RunStatus types |
| `src/engine/assertions.rs` | Assertion evaluation (exit code, stdout, file checks) |
| `src/engine/qemu.rs` | QEMU boot step support |
| `src/engine/container_setup.rs` | Shared container initialization logic |
| `src/engine/mock_server.rs` | Mock HTTP server for test steps |
| `src/container/` | ContainerBackend trait, bollard implementation |
| `src/error.rs` | Typed ConaryTestError enum (Container, Timeout, Cancelled, etc.) |
| `src/error_taxonomy.rs` | Structured error types with categories |
| `src/report/` | JSON output, SSE event streaming |
| `src/server/state.rs` | AppState with DashMap, broadcast channel, cancellation flags |
| `src/server/remi_client.rs` | HTTP client for pushing results to Remi |
| `src/server/wal.rs` | Local write-ahead log for Remi unreachability |
| `src/server/service.rs` | Shared business logic layer |
| `src/server/auth.rs` | Authentication module |
| `src/server/` | Axum HTTP API (14 endpoints), MCP server (23 tools via rmcp) |
| `src/cli.rs` | Binary entrypoint |
