# Crate Split Design: conary-core + conary-server

## Goals

1. **Standalone Remi** — ship the package server as its own installable binary
2. **Build time reduction** — client builds skip axum/tantivy/s3/mdns (~13 heavy deps)
3. **Clean architecture** — proper separation of concerns for long-term maintainability

## Workspace Structure

```
conary/                          workspace root
├── conary-core/                 library crate — all shared logic
│   └── src/lib.rs               re-exports from today's src/lib.rs
│                                minus server, daemon, federation
├── conary-erofs/                already exists, unchanged
├── conary-server/               Remi server + conaryd daemon
│   └── src/
│       ├── lib.rs               server + federation + daemon modules
│       ├── bin/remi.rs          standalone Remi binary
│       └── bin/conaryd.rs       standalone daemon binary
└── src/                         root crate = CLI binary
    ├── main.rs                  conary CLI entry point
    ├── cli/                     clap definitions
    └── commands/                command implementations
```

## What Goes Where

### conary-core (library)

All shared modules that both client and server need:

- `db/` — SQLite schema, models, migrations
- `packages/` — RPM/DEB/Arch parsers, PackageMetadata
- `repository/` — Metadata sync, download, mirror selection, GPG
- `ccs/` — Native package format, builder, policy, OCI export
- `filesystem/` — CAS, deployer, VFS, fsverity, path utils
- `compression/` — Gzip/Xz/Zstd unified decompression
- `resolver/` — SAT dependency resolution (resolvo)
- `transaction/` — TransactionEngine, journal, crash recovery
- `dependencies/` — Dependency types, provider matching
- `version/` — Version parsing, constraints
- `hash/` — SHA-256, Blake3, XXH128
- `error/` — Error/Result types
- `label/`, `flavor/`, `components/` — Metadata types
- `scriptlet/`, `container/`, `trigger/` — Post-install execution
- `model/` — Declarative system model
- `delta/` — Binary delta updates
- `capability/`, `provenance/`, `trust/` — Security/audit
- `automation/`, `bootstrap/`, `recipe/`, `derived/` — Advanced features
- `progress/` — Progress tracking abstractions

Internal workspace member only — not published to crates.io.

### conary-server (Remi + conaryd)

- `server/` — All 32 files (handlers, routes, conversion, cache, search, R2)
- `federation/` — Peer discovery, chunk routing, circuit breakers, mDNS
- `daemon/` — conaryd REST API, polkit auth, systemd, unix socket, jobs

Produces two binaries: `remi` and `conaryd`.

### Root crate (CLI binary)

- `main.rs` — Entry point
- `cli/` — Clap argument definitions
- `commands/` — All command implementations

## Dependency Flow

```
conary-core          (no feature gates)
    ↑
    ├── conary (CLI binary)
    │     depends on: conary-core, conary-erofs
    │     optional dep: conary-server (for `conary server start`)
    │
    ├── conary-server
    │     depends on: conary-core
    │     brings in: axum, tower-http, tantivy, rust-s3, mdns-sd,
    │                dashmap, sd-notify, hyper, axum-extra, zbus
    │
    └── conary-erofs   (unchanged, no dependency on conary-core)
```

### Feature gates (simplified)

- `conary-core`: No features. Plain library.
- `conary-server`: No features. Everything compiles. Two `[[bin]]` targets.
- Root `conary` crate: One optional feature:
  ```toml
  [features]
  server = ["dep:conary-server"]
  ```

### Workspace dependencies

Shared versions managed at workspace level:
```toml
[workspace.dependencies]
rusqlite = { version = "0.32", features = ["bundled"] }
tokio = { version = "1", features = ["full"] }
anyhow = "1.0"
thiserror = "1.0"
```

Sub-crates reference as `rusqlite.workspace = true`.

## Binary Targets

**`remi`** — standalone Remi package server (in conary-server)
**`conaryd`** — system daemon (in conary-server)

Both are thin wrappers with their own clap arg parsing.

**CLI convenience commands:**
- `conary server start` → calls `conary_server` (requires `--features server`)
- Without feature: prints helpful error directing to `remi` binary

## Migration Strategy

### Phase 1: Create conary-core
1. Create `conary-core/Cargo.toml`
2. Move shared modules from `src/` to `conary-core/src/`
3. Move shared dependencies
4. Set up `[workspace.dependencies]`
5. Temporary re-export shim: root `src/lib.rs` does `pub use conary_core::*;`

### Phase 2: Create conary-server
1. Create `conary-server/Cargo.toml`
2. Move `src/server/`, `src/federation/`, `src/daemon/`
3. Move server-only dependencies
4. Create `bin/remi.rs` and `bin/conaryd.rs`
5. Extract server/daemon CLI args from `src/cli/`

### Phase 3: Clean up root crate
1. Remove re-export shim
2. Update all `commands/` imports: `crate::` → `conary_core::`
3. Remove `#[cfg(feature = "server/daemon")]` guards
4. Add optional `conary-server` dep for convenience commands
5. Slim down root `Cargo.toml`

Each phase leaves the project compiling and tests passing.

## Verification

```bash
cargo build                              # client only — no server deps
cargo build -p conary-server             # server + daemon
cargo test --workspace                   # all tests pass
cargo clippy --workspace -- -D warnings  # clean
```

Success metric: `cargo build` (default) noticeably faster without ~13 server deps.
