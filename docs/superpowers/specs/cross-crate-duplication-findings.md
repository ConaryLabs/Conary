---
last_updated: 2026-04-04
revision: 1
summary: Cross-crate duplication findings captured during the Phase 2 simplification pass
---

# Cross-Crate Duplication Findings

This document records patterns noticed during the cross-chunk simplification
pass. It is intentionally descriptive only. No refactor is proposed or
implemented here.

## Scope

The survey focused on:

- `apps/conaryd/src/` and `apps/remi/src/` as server crates
- `apps/conary/src/` and `apps/conaryd/src/` where both encode system
  operations against the daemon/database layer
- configuration loading and default handling across the app crates

## Findings

### 1. Binary bootstrap is repeated across app crates

All three app entrypoints repeat the same high-level startup shape:

- initialize tracing with `tracing_subscriber`
- parse CLI args with `clap`
- assemble a crate-local config struct
- hand off to an async runtime or async entrypoint

Examples:

- `apps/conary/src/app.rs`
- `apps/conaryd/src/bin/conaryd.rs`
- `apps/remi/src/bin/remi.rs`

This is not a cleanup target for the current pass, but it is a clear future
candidate for shared bootstrap helpers if the workspace wants uniform logging,
error presentation, or runtime setup behavior.

### 2. Server crates repeat router scaffolding and operational envelopes

`conaryd` and `remi` both carry a similar server shell:

- large top-level config/state structs in the crate root module
- Axum router construction with crate-local middleware, body limits, and
  shared-state aliases
- per-crate error/response envelope shaping

Examples:

- `apps/conaryd/src/daemon/mod.rs`
- `apps/conaryd/src/daemon/routes.rs`
- `apps/remi/src/server/mod.rs`
- `apps/remi/src/server/routes.rs`

The details differ enough that a Phase 2 dedup would be the wrong move, but the
structural repetition is real. A future refactor could evaluate shared server
composition helpers, especially around middleware assembly and route bootstrap.

### 3. Operation taxonomy is encoded in both the CLI and daemon layers

The CLI and daemon independently model the same user-visible operation set:

- install
- remove
- update
- verify
- rollback
- garbage collection

Examples:

- `apps/conary/src/dispatch.rs`
- `apps/conaryd/src/daemon/mod.rs`
- `apps/conaryd/src/daemon/routes.rs`

Today this shows up as repeated command labels, repeated operation naming, and
parallel error/status shaping across the two crates. That is probably correct
for now, but it suggests future value in shared request/operation descriptors if
the daemon boundary becomes more formalized.

### 4. Config defaults are duplicated between CLI surfaces and runtime structs

Both `remi` and `conaryd` repeat default values in more than one layer:

- command-line defaults in the binary entrypoint
- runtime defaults in the main config struct
- file-based defaults in config parsing code

Examples:

- `apps/remi/src/bin/remi.rs`
- `apps/remi/src/server/mod.rs`
- `apps/remi/src/server/config.rs`
- `apps/conaryd/src/bin/conaryd.rs`
- `apps/conaryd/src/daemon/mod.rs`

This duplication is the clearest config-loading cleanup candidate for a later
refactor. It increases drift risk for bind addresses, DB paths, storage roots,
and security defaults.

### 5. Remi has two overlapping config vocabularies

Within `remi`, there is a split between:

- `ServerConfig` as the runtime struct used by server state and routers
- `RemiConfig` as the TOML-facing configuration schema

Examples:

- `apps/remi/src/server/mod.rs`
- `apps/remi/src/server/config.rs`

That split is reasonable, but the conversion layer means defaults and field
meanings are described in multiple places. A future pass could decide whether
to consolidate default ownership or generate one view from the other.

## Non-Findings

- No Phase 2-safe shared abstraction was obvious enough to justify a refactor in
  this cleanup pass.
- The duplication patterns above are mostly ownership and layering issues, not
  dead code.

## Suggested Future Refactor Themes

- Shared binary bootstrap helpers for tracing/runtime startup
- Shared config-default ownership for server crates
- Shared daemon operation descriptors across CLI and daemon boundaries
- Optional shared Axum server composition helpers for middleware and route setup
