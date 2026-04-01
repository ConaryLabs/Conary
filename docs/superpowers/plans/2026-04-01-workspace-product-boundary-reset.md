# Workspace Product-Boundary Reset Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reset the Conary repository into a virtual workspace with product-owned app crates (`conary`, `remi`, `conaryd`, `conary-test`) and a reduced shared core crate, while keeping all shipped binaries working and making build/test/release flows match the new package graph.

**Architecture:** Do the reset in one decisive branch, but in ordered phases: first make the workspace graph honest, then remove cross-package feature indirection, then clean up the ownership leaks exposed by the move, and only then update tooling, packaging, and docs. Favor product boundaries over compatibility shims, and prefer keeping tiny shared helper surfaces in an existing crate unless the ownership cleanup proves they deserve a dedicated support crate.

**Tech Stack:** Rust 2024 workspace, Cargo resolver 3, Clap, Axum, Tokio, existing `conary-core` shared library, GitHub Actions workflows, shell release/packaging scripts, approved spec at `docs/superpowers/specs/2026-04-01-workspace-product-boundary-reset-design.md`.

---

## File Map

- `Cargo.toml`: convert from root package manifest to virtual workspace manifest
- `apps/conary/Cargo.toml`: new home for the current root `conary` package manifest
- `apps/conary/build.rs`: moved and updated manpage/completions build script
- `apps/conary/src/main.rs`: moved root CLI entrypoint, then slimmed to dispatch-only ownership
- `apps/conary/src/cli/`: moved current CLI definitions for the package-manager app
- `apps/conary/src/commands/`: moved current command handlers for the package-manager app
- `apps/conary/tests/`: moved current root integration tests (`batch_install.rs`, `workflow.rs`, etc.)
- `apps/remi/Cargo.toml`: new package manifest for the Remi server product
- `apps/remi/src/lib.rs`: new Remi crate root
- `apps/remi/src/bin/remi.rs`: moved/retained Remi binary entrypoint
- `apps/remi/src/server/`: moved Remi HTTP/admin code
- `apps/remi/src/federation/`: moved Remi federation code
- `apps/conaryd/Cargo.toml`: new package manifest for the daemon product
- `apps/conaryd/src/lib.rs`: new daemon crate root
- `apps/conaryd/src/bin/conaryd.rs`: moved/retained daemon binary entrypoint
- `apps/conaryd/src/daemon/`: moved daemon API/socket/job code
- `apps/conary-test/Cargo.toml`: moved `conary-test` package manifest
- `apps/conary-test/src/lib.rs`: moved test-harness crate root
- `apps/conary-test/src/cli.rs`: moved test-harness binary entrypoint
- `apps/conary-test/src/config/`, `container/`, `engine/`, `report/`, `server/`: moved test-harness modules
- `crates/conary-core/Cargo.toml`: moved core manifest with feature reset
- `crates/conary-core/src/lib.rs`: moved core crate root with feature/module cleanup
- `crates/conary-core/src/**`: moved shared package-manager domain/infrastructure code
- `crates/conary-mcp/Cargo.toml`: create only if the explicit decision task proves it is necessary
- `crates/conary-mcp/src/lib.rs`: create only if the explicit decision task proves it is necessary
- `apps/conary/src/cli/system.rs`: rehome or delete service-launch surfaces that no longer belong to `conary`
- `apps/conary/src/cli/federation.rs`: keep the client-side scan surface and drop only the old cross-package feature gating around it
- `apps/conary/src/cli/trust.rs`: re-evaluate server-gated trust-admin operations
- `apps/conary/src/commands/system.rs`: remove feature-gated server launch hooks and keep only package-manager responsibilities
- `apps/conary/src/commands/federation.rs`: keep client-side federation admin flows, including LAN peer discovery, while extracting only the reusable discovery helper if needed
- `apps/conary/src/commands/trust.rs`: keep shared trust ops or move service-specific admin flows
- `apps/remi/src/server/routes.rs`: split by responsibility while moving into the Remi app crate
- `apps/conaryd/src/daemon/routes.rs`: split by responsibility while moving into the daemon app crate
- `.github/workflows/ci.yml`: update package-specific build/test commands
- `.github/workflows/release.yml`: update package-specific release builds and artifact expectations
- `.claude/rules/**`: update agent-facing architecture/build guidance that still describes `conary-server` and `--features server`
- `.claude/agents/**`: update packaged agent playbooks that still teach the old workspace layout
- `.claude/hooks/post-edit-clippy.sh`: replace stale `cargo clippy --features server` assumptions
- `scripts/release.sh`: update release grouping from the old root/server/test model to the new product model
- `scripts/sign-release.sh`, `scripts/rebuild-remi.sh`, `scripts/bootstrap-remi.sh`, `scripts/deploy-forge.sh`: update path/package assumptions if they rely on the old layout
- `packaging/rpm/conary.spec`: update build paths and package commands
- `packaging/deb/debian/control`, `packaging/deb/debian/rules`: update workspace/build paths
- `packaging/arch/PKGBUILD`: update workspace/build paths
- `packaging/ccs/build.sh`, `packaging/ccs/ccs.toml`, `packaging/ccs/stage/**`: update artifact generation assumptions if paths change
- `README.md`: update package-specific build/run commands
- `site/src/routes/**`: update checked-in site pages if they mention the old workspace graph or old Cargo commands
- `docs/ARCHITECTURE.md`: rewrite around the new package graph
- `docs/INTEGRATION-TESTING.md`: update `conary-test` paths and commands after relocation
- `docs/conaryopedia-v2.md`: update long-form repo and command explanations that LLMs and humans use for orientation
- `CLAUDE.md` and `AGENTS.md`: update contributor guidance if package paths/commands change
- `docs/superpowers/specs/2026-04-01-workspace-product-boundary-reset-design.md`: approved spec reference; keep implementation aligned with it

## Chunk 1: Workspace Graph Reset

### Task 1: Convert The Root Into A Virtual Workspace And Move The `conary` App

**Files:**
- Modify: `Cargo.toml`
- Create: `apps/conary/Cargo.toml`
- Move: `build.rs` -> `apps/conary/build.rs`
- Move: `src/` -> `apps/conary/src/`
- Move: `tests/` -> `apps/conary/tests/`
- Test: `apps/conary/tests/workflow.rs`
- Test: `apps/conary/tests/batch_install.rs`

- [ ] **Step 1: Capture the current red-state workspace assumptions**

Run:
- `test -d /home/peter/Conary/apps/conary`
- `cargo metadata --no-deps --format-version 1`

Expected:
- `test -d .../apps/conary` fails because the app directory does not exist yet
- `cargo metadata` shows the root package manifest path at `/home/peter/Conary/Cargo.toml`

- [ ] **Step 2: Create the new `apps/` and `crates/` directories**

Run:

```bash
mkdir -p apps crates
```

- [ ] **Step 3: Move the current root package into `apps/conary`**

Run:

```bash
mkdir -p apps/conary
git mv src apps/conary/src
git mv tests apps/conary/tests
git mv build.rs apps/conary/build.rs
```

- [ ] **Step 4: Split the old root manifest into a virtual workspace root and an app manifest**

Replace the root `Cargo.toml` with a workspace-only manifest:

```toml
[workspace]
members = [
  "apps/conary",
  "conary-server",
  "conary-test",
  "conary-core",
]
resolver = "3"

[workspace.dependencies]
# move the current shared dependency table here unchanged for now
```

Create `apps/conary/Cargo.toml` from the old root `[package]`,
`[features]`, `[dependencies]`, and `[build-dependencies]` sections, then
update relative paths:

```toml
[package]
name = "conary"

[features]
default = []
server = ["dep:conary-server", "conary-core/server"]
polkit = ["conary-server/polkit"]
experimental = []

[dependencies]
conary-core = { path = "../../conary-core" }
conary-server = { path = "../../conary-server", optional = true }

[build-dependencies]
clap = { workspace = true, features = ["cargo"] }
clap_mangen = "0.2"
```

This task is intentionally a structural move only. Keep the old package names
and feature seams for one task so the workspace graph can be re-established
before the server split.

- [ ] **Step 5: Run structural verification**

Run:
- `cargo metadata --no-deps --format-version 1`
- `cargo build -p conary --verbose`
- `cargo test -p conary workflow -- --nocapture`

Expected:
- `cargo metadata` succeeds with the root manifest acting as a virtual workspace
- `cargo build -p conary` passes from `apps/conary`
- the selected `conary` integration test target runs from `apps/conary/tests`

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml apps/conary
git commit -m "refactor(workspace): move conary app into apps directory"
```

### Task 2: Move `conary-core` And `conary-test` To Their New Homes

**Files:**
- Move: `conary-core/` -> `crates/conary-core/`
- Move: `conary-test/` -> `apps/conary-test/`
- Modify: `Cargo.toml`
- Modify: `apps/conary/Cargo.toml`
- Test: `apps/conary-test/src/lib.rs`
- Test: `crates/conary-core/src/lib.rs`

- [ ] **Step 1: Capture the red state for the new package paths**

Run:
- `cargo build -p conary-core --verbose`
- `cargo build -p conary-test --verbose`
- `test -d /home/peter/Conary/crates/conary-core`
- `test -d /home/peter/Conary/apps/conary-test`

Expected:
- builds currently work only through the old package locations
- the new directories do not exist yet

- [ ] **Step 2: Move the packages**

Run:

```bash
mkdir -p apps crates
git mv conary-core crates/conary-core
git mv conary-test apps/conary-test
```

- [ ] **Step 3: Update workspace members and path dependencies**

Modify the root workspace members to:

```toml
members = [
  "apps/conary",
  "apps/conary-test",
  "conary-server",
  "crates/conary-core",
]
```

Update path dependencies:
- `apps/conary/Cargo.toml` -> `conary-core = { path = "../../crates/conary-core" }`
- `conary-server/Cargo.toml` -> `conary-core = { path = "../crates/conary-core", ... }`
- `apps/conary-test/Cargo.toml` -> `conary-core = { path = "../../crates/conary-core", ... }`

- [ ] **Step 4: Run package-specific verification**

Run:
- `cargo build -p conary-core --verbose`
- `cargo build -p conary-test --verbose`
- `cargo test -p conary-core --lib --no-run`
- `cargo test -p conary-test --lib --no-run`

Expected: PASS from the new `crates/` and `apps/` locations

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml apps/conary apps/conary-test crates/conary-core conary-server/Cargo.toml
git commit -m "refactor(workspace): relocate shared core and test harness"
```

### Task 3: Split `conary-server` Into `remi` And `conaryd`

**Files:**
- Modify: `Cargo.toml`
- Create: `apps/remi/Cargo.toml`
- Create: `apps/remi/src/lib.rs`
- Create: `apps/conaryd/Cargo.toml`
- Create: `apps/conaryd/src/lib.rs`
- Modify: `conary-server/Cargo.toml`
- Create: `conary-server/src/lib.rs`
- Move: `conary-server/src/server/` -> `apps/remi/src/server/`
- Move: `conary-server/src/federation/` -> `apps/remi/src/federation/`
- Move: `conary-server/src/bin/remi.rs` -> `apps/remi/src/bin/remi.rs`
- Move: `conary-server/src/daemon/` -> `apps/conaryd/src/daemon/`
- Move: `conary-server/src/bin/conaryd.rs` -> `apps/conaryd/src/bin/conaryd.rs`

- [ ] **Step 1: Capture the current red state**

Run:
- `cargo build -p remi --verbose`
- `cargo build -p conaryd --verbose`
- `cargo tree -p conary-server -e normal`

Expected:
- the new packages do not exist yet, so the package-specific builds fail
- `cargo tree` shows the current monolithic `conary-server` dependency graph
  that must be split between `apps/remi` and `apps/conaryd`

- [ ] **Step 2: Create the new app package skeletons**

Create `apps/remi/Cargo.toml` and `apps/conaryd/Cargo.toml` by splitting the
current `conary-server/Cargo.toml`.

Use this rule:
- keep shared domain dependencies on `crates/conary-core`
- put Remi-only service dependencies in `apps/remi/Cargo.toml`
- put daemon-only lifecycle/socket dependencies in `apps/conaryd/Cargo.toml`
- keep `polkit` only on `conaryd` unless a real compile path proves Remi also
  needs it

Start from these initial dependency buckets, then confirm with compile errors
instead of guessing:
- Remi-owned first pass: `filetime`, `async-stream`, `dashmap`, `governor`,
  `parking_lot`, `mdns-sd`, `flume`, `rust-s3`, `tantivy`, `rmcp`,
  `tower-http`
- conaryd-owned first pass: `sd-notify`, `hyper`, `hyper-util`,
  `http-body-util`, `zbus`
- shared app deps likely to remain in both manifests: `axum`, `tower`,
  `tokio`, `tokio-stream`, `clap`, `tracing-subscriber`, `anyhow`, `tracing`,
  `serde`, `serde_json`, `chrono`, `url`, and any direct `conary-core`/DB/HTTP
  imports still used by both trees

Make this explicit in the new manifests:

```toml
# apps/remi/Cargo.toml
[[bin]]
name = "remi"
path = "src/bin/remi.rs"

# apps/conaryd/Cargo.toml
[features]
default = []
polkit = ["dep:zbus"]

[[bin]]
name = "conaryd"
path = "src/bin/conaryd.rs"
```

Do not carry a `polkit` feature into `apps/remi`.

- [ ] **Step 3: Move the source trees**

Run:

```bash
mkdir -p apps/remi/src/bin apps/conaryd/src/bin
git mv conary-server/src/server apps/remi/src/server
git mv conary-server/src/federation apps/remi/src/federation
git mv conary-server/src/bin/remi.rs apps/remi/src/bin/remi.rs
git mv conary-server/src/daemon apps/conaryd/src/daemon
git mv conary-server/src/bin/conaryd.rs apps/conaryd/src/bin/conaryd.rs
```

Create slim crate roots:

```rust
// apps/remi/src/lib.rs
pub mod federation;
pub mod server;

// apps/conaryd/src/lib.rs
pub mod daemon;
```

- [ ] **Step 4: Convert `conary-server` into a temporary compatibility shim**

Change root workspace members to:

```toml
members = [
  "apps/conary",
  "apps/remi",
  "apps/conaryd",
  "apps/conary-test",
  "crates/conary-core",
  "conary-server",
]
```

Keep `conary-server` for one chunk only as a thin re-export shim so
`apps/conary` still resolves while Task 4 removes the last cross-package
feature wiring.

Rewrite `conary-server/Cargo.toml` to depend on the new app packages:

```toml
[features]
default = []
polkit = ["conaryd/polkit"]

[dependencies]
remi = { path = "../apps/remi" }
conaryd = { path = "../apps/conaryd" }
```

Create a temporary `conary-server/src/lib.rs`:

```rust
pub use conaryd::daemon;
pub use remi::federation;
pub use remi::server;
```

This shim must be deleted in Chunk 2 once `apps/conary` no longer depends on
the old `server` feature model.

The shim is library-only compatibility for one chunk. It does not preserve
`cargo run -p conary-server --bin remi` or `--bin conaryd` as stable commands;
those product binaries now live in `apps/remi` and `apps/conaryd`.

- [ ] **Step 5: Run package verification**

Run:
- `cargo build -p remi --verbose`
- `cargo build -p conaryd --verbose`
- `cargo build -p conaryd --features polkit --verbose`
- `cargo build -p conary --features server --verbose`
- `cargo build -p conary --features "server polkit" --verbose`
- `cargo test -p remi --lib --no-run`
- `cargo test -p conaryd --lib --no-run`

Expected:
- `remi` and `conaryd` build from their new packages
- `conary --features server` still resolves through the temporary shim
- the temporary `polkit` relay works through both `apps/conaryd` and the
  one-chunk `conary-server` shim

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml apps/remi apps/conaryd conary-server
git commit -m "refactor(workspace): split remi and conaryd into separate apps"
```

## Chunk 2: Feature And Ownership Reset

### Task 4: Remove The Root `server` Feature And Rehome Feature-Gated CLI Surfaces

**Files:**
- Modify: `Cargo.toml`
- Modify: `apps/conary/Cargo.toml`
- Modify: `apps/conary/src/main.rs`
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/cli/system.rs`
- Modify: `apps/conary/src/cli/federation.rs`
- Modify: `apps/conary/src/cli/trust.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Modify: `apps/conary/src/commands/system.rs`
- Modify: `apps/conary/src/commands/federation.rs`
- Modify: `apps/conary/src/commands/trust.rs`
- Modify: `apps/remi/src/bin/remi.rs`
- Modify: `apps/remi/src/lib.rs`
- Modify: `apps/conaryd/src/bin/conaryd.rs`
- Modify: `.github/workflows/ci.yml`
- Modify: `.claude/hooks/post-edit-clippy.sh`
- Delete: `conary-server/`

- [ ] **Step 1: Audit every existing `#[cfg(feature = \"server\")]` surface and classify its owner**

Run:

```bash
rg -n 'cfg\\(feature = "server"\\)|feature = "server"|dep:conary-server|conary-core/server' \
  .
```

Classify each hit into one of three buckets in the commit message notes or
task scratchpad:
- stays in `conary` as a package-manager or remote-admin client concern
- moves to `remi`
- moves to `conaryd`

Minimum expected moves:
- `SystemCommands::Server` -> `remi`
- `Commands::Remi` -> `remi`
- `Commands::Daemon` -> `conaryd`
- `Commands::RemiProxy` -> `remi`
- `SystemCommands::IndexGen` / `Prewarm` -> `remi`
- `FederationCommands::Scan` stays in `conary` as a client-side federation
  discovery/admin concern; extract only the reusable mDNS discovery helper if
  the current implementation still depends on Remi-owned code
- `TrustCommands::SignTargets` / `RotateKey` -> `remi`, while shared signing
  helpers stay in `crates/conary-core`

- [ ] **Step 2: Capture the red state**

Run:
- `cargo build -p conary --verbose`

Expected:
- it still depends on the old cross-package feature model and will break once
  the feature and old package paths are removed
- current server compilation already works through `conary-server`'s own
  unconditional dependency on `conary-core/server`; this task is removing CLI
  indirection, not inventing server crate viability from scratch

- [ ] **Step 3: Remove the cross-package feature indirection**

In `apps/conary/Cargo.toml`:
- delete the `server` feature entirely
- delete `polkit = ["conary-server/polkit"]`
- remove the optional dependency on the old server package or split products

Keep only features that change the `conary` app itself, for example:

```toml
[features]
default = []
experimental = []
```

- [ ] **Step 4: Rehome or delete the old gated dispatch arms**

In `apps/conary/src/main.rs` and related CLI modules:
- remove `#[cfg(feature = "server")]` dispatch branches that launched services
- add or extend direct CLI entrypoints in `apps/remi/src/bin/remi.rs` and
  `apps/conaryd/src/bin/conaryd.rs`
- delete the `Commands::Daemon`, `Commands::Remi`, and `Commands::RemiProxy`
  variants from `apps/conary/src/cli/mod.rs`
- delete `TrustCommands::SignTargets` and `TrustCommands::RotateKey` from
  `apps/conary/src/cli/trust.rs` and rehome their owned CLI/admin surface to
  `apps/remi`
- keep `conary` focused on the package-manager UX and any explicitly approved
  remote-admin client flows

Do not leave dead feature-gated variants in clap enums after removing the
feature.

Make the owning binaries self-contained:
- `apps/remi/src/bin/remi.rs` must own Remi CLI parsing directly
- `apps/conaryd/src/bin/conaryd.rs` must own daemon CLI parsing directly
- do not leave either binary depending on `apps/conary` clap enums

In the same step:
- remove `conary-server` from the root workspace members
- delete the temporary `conary-server` shim package created in Chunk 1
- ensure `apps/conary/Cargo.toml` no longer references the shim package at all
- update `.github/workflows/ci.yml` and `.claude/hooks/post-edit-clippy.sh`
  in the same commit so the tree does not keep obvious references to the
  deleted `conary-server` package or root `--features server` commands

- [ ] **Step 5: Run verification**

Run:
- `cargo build -p conary --verbose`
- `cargo build -p remi --verbose`
- `cargo build -p conaryd --verbose`
- `cargo metadata --no-deps --format-version 1`
- `rg -n 'conary-server|--features server' .github/workflows/ci.yml .claude/hooks/post-edit-clippy.sh`
- `cargo test -p conary --lib --no-run`

Expected:
- `conary` builds without any `server` feature
- `remi` and `conaryd` own their startup flows directly
- `cargo metadata` no longer lists `conary-server` as a workspace member
- the minimal CI/hook surfaces no longer reference `conary-server` or the old
  root `server` feature

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml apps/conary apps/remi apps/conaryd conary-server .github/workflows/ci.yml .claude/hooks/post-edit-clippy.sh
git commit -m "refactor(cli): remove cross-package server feature wiring"
```

### Task 5: Reset `conary-core` Features And Decide The MCP Support Boundary

**Files:**
- Modify: `crates/conary-core/Cargo.toml`
- Modify: `crates/conary-core/src/lib.rs`
- Modify: `apps/remi/Cargo.toml`
- Modify: `apps/conaryd/Cargo.toml`
- Modify: `apps/conary-test/Cargo.toml`
- Optional Create: `crates/conary-mcp/Cargo.toml`
- Optional Create: `crates/conary-mcp/src/lib.rs`

- [ ] **Step 1: Audit current feature consumers**

Run:

```bash
rg -n 'cfg\\(feature = "server"\\)|cfg\\(feature = "mcp"\\)|composefs-rs|conary_core::mcp' \
  crates/conary-core apps/remi apps/conaryd apps/conary-test apps/conary
```

Use the spec decision rules explicitly:
- remove `server` from `crates/conary-core`
- keep `composefs-rs` only if there is a real supported dual-mode reason
- create `crates/conary-mcp` only if at least two apps still need
  transport-agnostic helpers after ownership cleanup

- [ ] **Step 2: Capture the red state**

Run:
- `cargo build -p conary-core --all-features --verbose`

Expected: this highlights any lingering feature assumptions before the reset

- [ ] **Step 3: Apply the feature decisions**

Implement the chosen manifest changes:
- delete `server = [...]` from `crates/conary-core/Cargo.toml`
- either keep or remove `mcp` in core based on the audit
- either keep or remove `composefs-rs` in core based on the audit
- if `crates/conary-mcp` is needed, move only the transport-agnostic helpers:
  `server_info`, JSON/text formatting helpers, and similar utility code

Choose one of these target shapes explicitly after the audit:

```toml
# Preferred if no supported dual-mode toggle remains:
[features]
default = []

# Only if the audit proves a real supported dual-mode need:
[features]
default = ["composefs-rs"]
composefs-rs = ["dep:composefs"]
```

Current-state reminder: `apps/conary-test` currently depends on
`conary-core/mcp`. Resolve that dependency explicitly in this task:
- if both `apps/remi` and `apps/conary-test` still need the same
  transport-agnostic MCP helpers, create `crates/conary-mcp` and repoint both
  apps to it
- if only one app still needs the helpers after ownership cleanup, inline them
  into the owning app and delete the shared `mcp` feature from core

Current evidence says the genuinely shared MCP surface is tiny today
(`to_json_text`, `server_info`, and a small amount of validation/boilerplate).
That should drive the outcome toward one of these two paths:
- if multiple apps still share those helpers after ownership cleanup, create a
  very small `crates/conary-mcp` and move them there
- if only one app still needs them, inline them into the owning app

Do not treat the small helper size as justification for leaving app-facing MCP
helpers in `crates/conary-core`. Keeping them in core is the exception, and it
must be justified explicitly by a clearly domain-focused boundary that does not
blur app ownership again.

Do not move product policy or service wiring into `crates/conary-mcp`.

- [ ] **Step 4: Run verification**

Run:
- `cargo build -p conary-core --verbose`
- `cargo test -p conary-core --lib --no-run`
- `cargo build -p remi --verbose`
- `cargo build -p conaryd --verbose`
- `cargo build -p conary-test --verbose`

Expected: PASS with no remaining conceptual need for `conary-core/server`, and
no stale daemon assumptions about the removed core feature wiring

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core apps/remi apps/conaryd apps/conary-test
git commit -m "refactor(core): simplify core features and shared helpers"
```

If `crates/conary-mcp` was created in this task, add it to the same commit
explicitly before committing.

### Task 6: Split The Worst Ownership Hotspots While The Move Is Fresh

**Files:**
- Modify: `apps/conary/src/main.rs`
- Create: `apps/conary/src/app.rs`
- Create: `apps/conary/src/dispatch.rs`
- Modify: `apps/remi/src/server/routes.rs`
- Create: `apps/remi/src/server/routes/`
- Modify: `apps/conaryd/src/daemon/routes.rs`
- Create: `apps/conaryd/src/daemon/routes/`

- [ ] **Step 1: Identify natural split points before moving code**

For `apps/conaryd/src/daemon/routes.rs`, map the current handlers into likely
families before creating files. Start with candidates such as:
- `routes/jobs.rs`
- `routes/query.rs`
- `routes/transactions.rs`
- `routes/system.rs`
- `routes/events.rs`
- `routes/auth.rs`

For `apps/remi/src/server/routes.rs`, confirm whether the natural split is
actually `public`, `admin`, and `mcp`, or whether another ownership boundary
fits the current code better.

Do not start moving handlers until the grouping is written down in task notes
and matches the current call graph.

- [ ] **Step 2: Capture baseline file sizes**

Run:

```bash
wc -l apps/conary/src/main.rs \
      apps/remi/src/server/routes.rs \
      apps/conaryd/src/daemon/routes.rs
```

Expected: the current files are large enough that ownership is hard to read as
one unit

- [ ] **Step 3: Split `conary` entrypoint from dispatch**

Target shape:

```rust
// apps/conary/src/main.rs
mod app;
mod dispatch;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    app::run().await
}

// apps/conary/src/app.rs
pub async fn run() -> anyhow::Result<()> {
    /* parse CLI, initialize tracing, call dispatch */
}

// apps/conary/src/dispatch.rs
pub async fn dispatch(cli: Cli) -> anyhow::Result<()> { /* match commands */ }
```

Keep `main.rs` tiny. Do not let the new `app.rs` become another 2,000-line
god file.

Preserve behavior while splitting:
- move startup and tracing initialization into `app.rs`
- move the large command match and dispatch logic into `dispatch.rs`
- do not silently rewrite command behavior while performing the structural split
- treat this as a first-pass structural extraction; deeper command-family
  cleanup can happen later once the package ownership reset is finished

- [ ] **Step 4: Split Remi and daemon route mega-files by responsibility**

For `apps/remi/src/server/routes.rs`, split into modules such as:
- `routes/public.rs`
- `routes/admin.rs`
- `routes/mcp.rs`

For `apps/conaryd/src/daemon/routes.rs`, split by cohesive handler families
that already move together in code review and testing.

Do not split mechanically by line count. Split by ownership and router
responsibility.

- [ ] **Step 5: Run verification**

Run:
- `cargo build -p conary --verbose`
- `cargo build -p remi --verbose`
- `cargo build -p conaryd --verbose`
- `cargo test -p remi --lib --no-run`
- `cargo test -p conaryd --lib --no-run`

Expected: PASS with smaller, more obviously owned entrypoint/router files

- [ ] **Step 6: Commit**

```bash
git add apps/conary apps/remi apps/conaryd
git commit -m "refactor(apps): split oversized dispatch and route modules"
```

## Chunk 3: Tooling, Packaging, Docs, And Final Verification

### Task 7: Update Build, Release, Packaging, And CI Surfaces

**Files:**
- Modify: `apps/conary/build.rs`
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/release.yml`
- Modify: `scripts/release.sh`
- Modify: `scripts/sign-release.sh`
- Modify: `scripts/rebuild-remi.sh`
- Modify: `scripts/bootstrap-remi.sh`
- Modify: `scripts/deploy-forge.sh`
- Modify: `packaging/rpm/conary.spec`
- Modify: `packaging/deb/debian/control`
- Modify: `packaging/deb/debian/rules`
- Modify: `packaging/arch/PKGBUILD`
- Modify: `packaging/ccs/build.sh`

- [ ] **Step 1: Capture the red state**

Run:
- `cargo build -p conary --verbose`
- `bash -n scripts/release.sh`
- `bash -n packaging/ccs/build.sh`
- `sed -n '1,260p' scripts/release.sh`
- `sed -n '1,260p' .github/workflows/ci.yml`
- `rg -n 'conary-server|--features server|cargo build --verbose|cargo build -p conary-server' .github/workflows scripts packaging`

Expected: stale commands and package paths are still present

- [ ] **Step 2: Fix `apps/conary/build.rs`**

Update the build script so it reads the actual `conary` app CLI instead of
reconstructing an older CLI shape by hand.

Preferred end state:
- `apps/conary/build.rs` imports the app's clap builder or a shared `cli::build()`
  function
- generated manpages and shell completions reflect the real current CLI

Do not keep a shadow CLI definition in the build script.

- [ ] **Step 3: Update CI and release workflows**

Make the workflows use package-owned commands, for example:
- `cargo build -p conary`
- `cargo build -p remi`
- `cargo build -p conaryd`
- `cargo build -p conary-test`

Remove workflow logic that depends on the old root `server` feature or the old
`conary-server` package name.

Explicitly remove or replace old CI commands such as:
- `cargo build --features server`
- `cargo test --features server`
- `cargo build -p conary-server`
- `cargo test -p conary-server`

- [ ] **Step 4: Update release grouping and packaging scripts**

In `scripts/release.sh`, replace the old `conary/server/test` grouping with the
new product model. At minimum:
- `conary`
- `remi`
- `conaryd`
- `conary-test`

Redesign the release grouping logic, not just the paths:
- replace the old `server` group in `TAG_PREFIX`, `PATH_SCOPES`, and the
  version-bump case statement
- decide explicitly whether `remi` and `conaryd` get independent version tracks
  or a shared service-release policy
- update path scopes from `src/`, `conary-server/`, and `conary-test/` to the
  new `apps/` and `crates/` layout

In packaging files, update build invocations and manifest paths to the new
workspace layout without changing the shipped binary names.

- [ ] **Step 5: Run verification**

Run:
- `cargo build -p conary --release`
- `cargo build -p remi --release`
- `cargo build -p conaryd --release`
- `cargo build -p conary-test --release`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `bash -n scripts/release.sh`

Expected: PASS, with no stale `conary-server` or root-`server` feature usage in
CI/release/packaging surfaces

- [ ] **Step 6: Commit**

```bash
git add apps/conary/build.rs .github/workflows scripts packaging
git commit -m "refactor(tooling): align release and packaging with new workspace"
```

### Task 8: Update Architecture And LLM-Facing Docs And Run The Final Verification Matrix

**Files:**
- Modify: `README.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `CLAUDE.md`
- Modify: `AGENTS.md`
- Modify: `.claude/rules/**`
- Modify: `.claude/agents/**`
- Modify: `.claude/hooks/post-edit-clippy.sh`
- Modify: `site/src/routes/**`

- [ ] **Step 1: Update the docs to the new package graph**

Make the docs consistently describe:
- root as a virtual workspace
- `apps/conary`
- `apps/remi`
- `apps/conaryd`
- `apps/conary-test`
- `crates/conary-core`

Remove instructions that tell contributors to use the old root `server`
feature or the old `conary-server` package name.

Call out the replacement explicitly wherever it used to appear:
- old: `cargo build --features server`
- new: `cargo build -p remi && cargo build -p conaryd`
- old: `cargo test --features server`
- new: `cargo test -p remi && cargo test -p conaryd`

- [ ] **Step 2: Update LLM-facing orientation docs before the first post-refactor run**

Specifically update:
- `AGENTS.md`
- `CLAUDE.md`
- `docs/conaryopedia-v2.md`
- `.claude/rules/**`
- `.claude/agents/**`
- `.claude/hooks/post-edit-clippy.sh`

Make sure they reflect:
- the new workspace layout
- the new package names and `cargo` commands
- the removal of the old root `server` feature model
- where future agents should look for the `conary`, `remi`, `conaryd`, and
  `conary-test` entrypoints after the reset

Do not leave a stale “first fire-up” experience where an LLM reads old package
paths and starts editing the wrong crate.

Also update checked-in site source pages in `site/src/routes/**` if they teach
the old package graph or old Cargo commands. Do not spend time editing
generated `site/build/**` output or local-only `.worktrees/**` copies.

- [ ] **Step 3: Run the structural verification matrix**

Run:
- `cargo metadata --no-deps --format-version 1`
- `cargo build -p conary --verbose`
- `cargo build -p remi --verbose`
- `cargo build -p conaryd --verbose`
- `cargo build -p conary-test --verbose`
- `cargo build -p conary-core --verbose`

Expected:
- all five end-state packages appear in metadata
- the old `conary-server` package does not appear

- [ ] **Step 4: Run the behavioral verification matrix**

Run:
- `cargo test -p conary --verbose`
- `cargo test -p conary-core --verbose`
- `cargo test -p remi --verbose`
- `cargo test -p conaryd --verbose`
- `cargo test -p conary-test --verbose`

If any suite is too slow or too noisy to run in full during the migration,
record the reduced command explicitly in the implementation notes rather than
silently skipping coverage.

- [ ] **Step 5: Run the operational verification matrix**

Run:
- `cargo run -p conary -- --help`
- `cargo run -p remi -- --help`
- `cargo run -p conaryd -- --help`
- `cargo run -p conary-test -- --help`

Expected: each binary starts from its own package and exposes the expected
product surface

- [ ] **Step 6: Commit**

```bash
git add README.md docs/ARCHITECTURE.md docs/INTEGRATION-TESTING.md docs/conaryopedia-v2.md CLAUDE.md AGENTS.md .claude site/src/routes
git commit -m "docs: update workspace architecture and commands"
```

## Execution Notes

- Do not add a long-lived compatibility layer for the old root `server`
  feature. The point of this reset is to make the package graph honest.
- Do not create `crates/conary-mcp` unless the explicit decision task proves it
  is warranted.
- Keep binary names stable unless there is a very strong reason to rename them.
  The package graph is changing; the shipped tool names do not need gratuitous
  churn.
- Treat `AGENTS.md`, `CLAUDE.md`, and `docs/conaryopedia-v2.md` as release-
  critical orientation docs for both humans and LLMs. They should be updated
  before calling the refactor done.
- If a command surface currently exists only because the old `conary` root app
  could start Remi or the daemon in-process, prefer moving that surface to the
  owning binary instead of preserving it in `conary`.
