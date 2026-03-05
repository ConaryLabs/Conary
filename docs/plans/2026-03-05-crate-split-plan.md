# Crate Split Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Split the monolithic conary crate into a workspace with `conary-core` (shared library), `conary-server` (Remi + conaryd), and the root crate (CLI binary).

**Architecture:** Three-phase extraction. Phase 1 creates `conary-core` and moves all library modules into it with a temporary re-export shim for compatibility. Phase 2 extracts `conary-server` with its own binaries. Phase 3 removes the shim and cleans up the root crate. Each phase leaves the project compiling and tests passing.

**Tech Stack:** Rust workspace (resolver v3), Cargo features, no new dependencies.

---

### Task 1: Create conary-core crate skeleton

**Files:**
- Create: `conary-core/Cargo.toml`
- Create: `conary-core/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

**Step 1: Create conary-core/Cargo.toml**

```toml
[package]
name = "conary-core"
version = "0.1.0"
edition = "2024"
rust-version = "1.93"
authors = ["Conary Contributors"]
description = "Core library for the Conary package manager"
license = "MIT OR Apache-2.0"

[dependencies]
# Will be populated in Task 2
```

**Step 2: Create conary-core/src/lib.rs**

```rust
// conary-core/src/lib.rs
//! Conary Core Library
//!
//! Shared types, database, package parsing, and filesystem operations
//! used by both the CLI client and the Remi server.
```

**Step 3: Add to workspace**

In root `Cargo.toml`, change:
```toml
members = [".", "conary-erofs"]
```
to:
```toml
members = [".", "conary-core", "conary-erofs"]
```

**Step 4: Verify**

Run: `cargo build -p conary-core`
Expected: empty crate builds successfully.

**Step 5: Commit**

```bash
git add conary-core/ Cargo.toml
git commit -m "build: Create conary-core crate skeleton"
```

---

### Task 2: Move shared dependencies to conary-core

**Files:**
- Modify: `conary-core/Cargo.toml` (add all shared deps)
- Modify: `Cargo.toml` (remove moved deps, add `conary-core` dep)

**Step 1: Populate conary-core/Cargo.toml dependencies**

Move ALL non-optional, non-CLI dependencies from root `Cargo.toml` to `conary-core/Cargo.toml`. These are the library dependencies:

```toml
[dependencies]
# Database
rusqlite = { version = "0.32", features = ["bundled"] }

# Error handling
thiserror = "1.0"
anyhow = "1.0"

# Hashing
sha2 = "0.10"
md-5 = "0.10"
xxhash-rust = { version = "0.8", features = ["xxh3"] }

# Logging
tracing = "0.1"

# Package format parsing
rpm = "0.14"
tempfile = "3.10"

# Version parsing
semver = "1.0"

# HTTP client
reqwest = { version = "0.11", features = ["blocking", "rustls-tls", "json"] }

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"
ciborium = "0.2"

# Date/time
chrono = { version = "0.4", features = ["serde"] }

# Delta compression
zstd = "0.13"

# Content-Defined Chunking
fastcdc = "3.2"
walkdir = "2.5"

# Parallel processing
rayon = "1.8"

# SAT resolver
resolvo = "0.10"

# Progress bars
indicatif = "0.17"

# GPG verification
sequoia-openpgp = { version = "1.17", default-features = false, features = ["crypto-rust", "allow-experimental-crypto", "allow-variable-time-crypto"] }

# Archive parsing
tar = "0.4"
flate2 = "1.0"
xz2 = "0.1"
rfc822-like = "0.2"
quick-xml = "0.31"
ar = "0.9"
wait-timeout = "0.2"
glob = "0.3.3"
log = "0.4"

# Transaction engine
uuid = { version = "1.0", features = ["v4"] }
fs2 = "0.4"
crc32fast = "1.4"

# Container isolation
nix = { version = "0.29", features = ["user", "mount", "sched", "process", "signal", "resource", "fs"] }
libc = "0.2"

# Signing
ed25519-dalek = { version = "2.1", features = ["rand_core"] }
rand = "0.8"
base64 = "0.22"
hex = "0.4"

# Regex
regex = "1.10"

# Enum macros
strum = "0.26"
strum_macros = "0.26"

# Sigstore
sigstore = { version = "0.13", default-features = false, features = ["bundle", "rekor", "sigstore-trust-root", "rustls-tls"] }
const-oid = "0.9"
pem = "3.0"
rustls-pki-types = "1.0"
webpki = { package = "rustls-webpki", version = "0.103", features = ["std"], default-features = false }
x509-cert = { version = "0.2", features = ["pem", "std"] }

# Misc
url = "2.5"
diffy = "0.4.2"
goblin = "0.10.4"
which = "8.0.0"
dirs = "6.0.0"
tokio = { version = "1", features = ["full"] }

# Capability enforcement
landlock = "0.4"
seccompiler = "0.4"
```

**Step 2: Update root Cargo.toml**

Replace all the moved deps with a single dependency:
```toml
[dependencies]
conary-core = { path = "conary-core" }
conary-erofs = { path = "conary-erofs" }

# CLI-only deps (stay in root)
clap = { version = "4.5", features = ["derive", "cargo"] }
clap_complete = "4.5"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
anyhow = "1.0"

# Server deps (optional, stay in root for now — moved in Phase 2)
axum = { version = "0.7", optional = true }
# ... (keep all optional server/daemon deps for now)
```

Note: Keep `anyhow` and `tracing` in root too — `main.rs` and `commands/` use them directly. Keep ALL optional (server/daemon) deps in root for now — they move in Phase 2.

**Step 3: Verify**

Run: `cargo build -p conary-core`
Expected: builds (empty lib, but deps resolve).

**Step 4: Commit**

```bash
git add conary-core/Cargo.toml Cargo.toml
git commit -m "build: Move shared dependencies to conary-core"
```

---

### Task 3: Move library modules to conary-core

**Files:**
- Move: ALL modules from `src/` to `conary-core/src/` EXCEPT: `main.rs`, `cli/`, `commands/`, `server/`, `daemon/`, `federation/`
- Create: `conary-core/src/lib.rs` (with module declarations and pub use)
- Modify: `src/lib.rs` (thin re-export shim)

**Step 1: Move modules**

Move these directories/files from `src/` to `conary-core/src/`:

```
automation/    bootstrap/     capability/    ccs/
components/    compression/   container/     db/
delta/         dependencies/  derived/       error.rs
filesystem/    flavor/        hash.rs        label.rs
model/         packages/      progress/      provenance/
recipe/        repository/    resolver/      scriptlet/
transaction/   trigger/       trust/         version/
```

That's everything in `src/` except `main.rs`, `cli/`, `commands/`, `server/`, `daemon/`, `federation/`.

**Step 2: Write conary-core/src/lib.rs**

Copy the current `src/lib.rs` module declarations and re-exports, removing the server/daemon/federation sections:

```rust
// conary-core/src/lib.rs

//! Conary Core Library
//!
//! Shared types, database, package parsing, and filesystem operations
//! used by both the CLI client and the Remi server.

pub mod automation;
pub mod bootstrap;
pub mod capability;
pub mod ccs;
pub mod components;
pub mod compression;
pub mod container;
pub mod db;
pub mod delta;
pub mod dependencies;
pub mod derived;
mod error;
pub mod filesystem;
pub mod flavor;
pub mod hash;
pub mod label;
pub mod model;
pub mod packages;
pub mod progress;
pub mod provenance;
pub mod recipe;
pub mod repository;
pub mod resolver;
pub mod scriptlet;
pub mod transaction;
pub mod trigger;
pub mod trust;
pub mod version;

// Re-exports (copied from the current src/lib.rs, minus server/daemon/federation)
pub use automation::{
    ActionDecision, ActionStatus, AiSuggestion, AutomationManager, AutomationSummary, PendingAction,
};
pub use bootstrap::{
    Bootstrap, BootstrapConfig, BootstrapStage, Prerequisites, Stage0Builder, StageManager,
    TargetArch, Toolchain, ToolchainKind,
};
// ... (copy ALL pub use lines from current src/lib.rs, minus federation re-exports)
pub use error::{Error, Result};
```

**Step 3: Update src/lib.rs to be a thin re-export shim**

Replace `src/lib.rs` with:

```rust
// src/lib.rs
//! Compatibility shim — re-exports conary_core so existing `use conary::` paths work.

pub use conary_core::*;

// These modules remain in the root crate (for now — moved in Phase 2)
#[cfg(feature = "server")]
pub mod federation;

#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "daemon")]
pub mod daemon;
```

This shim means ALL existing `use conary::db::models::Trove` paths in `commands/` continue to work unchanged. The compiler resolves them through the re-export.

**Step 4: Fix file headers**

Every moved file starts with a path comment like `// src/db/models/mod.rs`. Update them:
- `// src/db/models/mod.rs` → `// conary-core/src/db/models/mod.rs`

This can be done with a bulk sed or incrementally.

**Step 5: Fix internal imports**

Inside the moved modules, imports like `use crate::db::models::Trove` should still work because `crate` now refers to `conary-core`. No changes needed for `use crate::` paths within conary-core.

However, the `server/`, `daemon/`, and `federation/` modules (still in root) use `use crate::` to access what's now in conary-core. Since `src/lib.rs` re-exports everything via `pub use conary_core::*`, these `use crate::` paths should resolve through the shim. Verify this compiles.

**Step 6: Verify**

```bash
cargo build -p conary-core    # core library builds
cargo build                   # root crate builds (shim works)
cargo test --workspace        # all tests pass
cargo clippy --workspace -- -D warnings
```

**Step 7: Commit**

```bash
git add -A
git commit -m "refactor: Move library modules to conary-core"
```

---

### Task 4: Create conary-server crate skeleton

**Files:**
- Create: `conary-server/Cargo.toml`
- Create: `conary-server/src/lib.rs`
- Modify: `Cargo.toml` (add workspace member)

**Step 1: Create conary-server/Cargo.toml**

```toml
[package]
name = "conary-server"
version = "0.1.0"
edition = "2024"
rust-version = "1.93"
authors = ["Conary Contributors"]
description = "Remi package server and conaryd daemon for Conary"
license = "MIT OR Apache-2.0"

[dependencies]
conary-core = { path = "../conary-core" }

# Web framework
axum = "0.7"
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["io"] }
tower-http = { version = "0.6", features = ["fs", "cors", "compression-gzip"] }
tower = "0.5"

# Server utilities
filetime = "0.2"
async-trait = "0.1"
futures = "0.3"
dashmap = "6.0"
parking_lot = "0.12"

# Service discovery
mdns-sd = "0.11"
flume = "0.11"

# Object storage
rust-s3 = { version = "0.35", default-features = false, features = ["tokio-rustls-tls"] }

# Full-text search
tantivy = "0.22"

# Daemon dependencies
sd-notify = "0.4"
axum-extra = { version = "0.9", features = ["typed-header"] }
hyper = { version = "1.4", features = ["server", "http1"] }
hyper-util = { version = "0.1", features = ["tokio", "server", "server-auto", "service"] }
http-body-util = "0.1"
tokio-stream = { version = "0.1", features = ["sync"] }

# PolicyKit (optional)
zbus = { version = "4.0", optional = true }

# Shared deps needed directly
anyhow = "1.0"
tracing = "0.1"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
chrono = { version = "0.4", features = ["serde"] }
url = "2.5"

[features]
default = []
polkit = ["dep:zbus"]

[[bin]]
name = "remi"
path = "src/bin/remi.rs"

[[bin]]
name = "conaryd"
path = "src/bin/conaryd.rs"
```

**Step 2: Create stub lib.rs and bin stubs**

`conary-server/src/lib.rs`:
```rust
// conary-server/src/lib.rs
//! Remi package server and conaryd daemon
```

`conary-server/src/bin/remi.rs`:
```rust
// conary-server/src/bin/remi.rs
fn main() {
    println!("remi server - placeholder");
}
```

`conary-server/src/bin/conaryd.rs`:
```rust
// conary-server/src/bin/conaryd.rs
fn main() {
    println!("conaryd daemon - placeholder");
}
```

**Step 3: Add to workspace**

```toml
members = [".", "conary-core", "conary-erofs", "conary-server"]
```

**Step 4: Verify**

```bash
cargo build -p conary-server
```

**Step 5: Commit**

```bash
git add conary-server/ Cargo.toml
git commit -m "build: Create conary-server crate skeleton"
```

---

### Task 5: Move server, federation, daemon modules to conary-server

**Files:**
- Move: `src/server/` → `conary-server/src/server/`
- Move: `src/federation/` → `conary-server/src/federation/`
- Move: `src/daemon/` → `conary-server/src/daemon/`
- Modify: `conary-server/src/lib.rs` (add module declarations)
- Modify: `src/lib.rs` (remove server/daemon/federation)

**Step 1: Move the directories**

```bash
mv src/server conary-server/src/server
mv src/federation conary-server/src/federation
mv src/daemon conary-server/src/daemon
```

**Step 2: Update conary-server/src/lib.rs**

```rust
// conary-server/src/lib.rs
//! Remi package server, federation, and conaryd daemon.

pub mod federation;
pub mod server;
pub mod daemon;
```

**Step 3: Fix imports in moved modules**

All `use crate::` references to core types need to change to `use conary_core::`. For example in `server/conversion.rs`:

```rust
// Before:
use crate::ccs::convert::{ConversionOptions, ConversionResult, LegacyConverter};
use crate::db::models::{ConvertedPackage, RepositoryPackage};

// After:
use conary_core::ccs::convert::{ConversionOptions, ConversionResult, LegacyConverter};
use conary_core::db::models::{ConvertedPackage, RepositoryPackage};
```

Internal `use crate::server::` and `use crate::daemon::` references stay as `use crate::` since they're within conary-server now.

The full list of files needing `crate::` → `conary_core::` changes (based on grep analysis):

**server/ files** (change `crate::ccs`, `crate::db`, `crate::filesystem`, `crate::packages`, `crate::repository`, `crate::model`):
- `server/conversion.rs` — 7 imports
- `server/prewarm.rs` — 1 import
- `server/analytics.rs` — 1 import
- `server/handlers/index.rs` — 1 import
- `server/index_gen.rs` — 1 import
- `server/handlers/mod.rs` — 1 import
- `server/cache.rs` — 1 import
- `server/handlers/detail.rs` — 1 import
- `server/handlers/models.rs` — 2 imports
- `server/handlers/sparse.rs` — 1 import
- `server/lite.rs` — change `crate::federation` to `crate::federation` (stays, it's within the crate)

**federation/ files** (change `crate::error`, `crate::hash`, `crate::repository`, `crate::ccs`):
- `federation/mod.rs` — 3 imports
- `federation/mdns.rs` — 1 import
- `federation/peer.rs` — 1 import
- `federation/manifest.rs` — 2 imports
- `federation/coalesce.rs` — 1 import

**daemon/ files** (change `crate::Result`, `crate::db`, `crate::ccs`):
- `daemon/mod.rs` — 1 import
- `daemon/jobs.rs` — 1 import (`crate::Result`)
- `daemon/routes.rs` — 1 import (`crate::db::models`)
- `daemon/client.rs` — 1 import
- `daemon/socket.rs` — 1 import
- `daemon/lock.rs` — 1 import
- `daemon/enhance.rs` — 1 import

**Step 4: Fix file headers**

Update path comments: `// src/server/mod.rs` → `// conary-server/src/server/mod.rs`, etc.

**Step 5: Update root src/lib.rs**

Remove server/daemon/federation from the shim:

```rust
// src/lib.rs
//! Compatibility shim — re-exports conary_core so existing `use conary::` paths work.
pub use conary_core::*;
```

**Step 6: Remove `#[cfg(feature = "server/daemon")]` guards from moved modules**

The modules are no longer feature-gated — they always compile when building conary-server. Remove any `#[cfg(feature = "server")]` on module-level items within the moved code. (Internal items like `mdns.rs` being server-only no longer applies — it's always built.)

**Step 7: Update root Cargo.toml**

Remove ALL optional server/daemon deps. Add conary-server as optional:

```toml
[features]
default = []
server = ["dep:conary-server"]

[dependencies]
conary-core = { path = "conary-core" }
conary-erofs = { path = "conary-erofs" }
conary-server = { path = "conary-server", optional = true }

# CLI-only deps
clap = { version = "4.5", features = ["derive", "cargo"] }
clap_complete = "4.5"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
anyhow = "1.0"
```

**Step 8: Verify**

```bash
cargo build                        # client only — no server deps
cargo build -p conary-server       # server + daemon
cargo test --workspace             # all tests pass
cargo clippy --workspace -- -D warnings
```

**Step 9: Commit**

```bash
git add -A
git commit -m "refactor: Move server, federation, daemon to conary-server"
```

---

### Task 6: Write remi and conaryd binary entry points

**Files:**
- Modify: `conary-server/src/bin/remi.rs`
- Modify: `conary-server/src/bin/conaryd.rs`

**Step 1: Write remi.rs**

Extract the `Remi` command handling from `src/main.rs` (the `Commands::Remi { ... }` match arm) into a standalone binary. This should:
1. Parse its own CLI args (clap)
2. Call `conary_server::server::run_server()` or equivalent

Look at the current `Commands::Remi` arm in `src/main.rs` to see what args it takes (config, bind, db, chunk_dir, etc.) and replicate that as a standalone clap struct.

**Step 2: Write conaryd.rs**

Similarly extract the `Commands::Daemon { ... }` match arm. Parse daemon-specific args (db, socket, idle_timeout) and call into `conary_server::daemon::run_daemon()`.

**Step 3: Verify**

```bash
cargo build -p conary-server
# Verify both binaries exist:
ls target/debug/remi target/debug/conaryd
```

**Step 4: Commit**

```bash
git add conary-server/src/bin/
git commit -m "feat: Add standalone remi and conaryd binaries"
```

---

### Task 7: Update root CLI for optional server commands

**Files:**
- Modify: `src/main.rs` — server/daemon command arms
- Modify: `src/cli/mod.rs` — feature gates
- Modify: `src/commands/federation.rs` — update import

**Step 1: Update server command dispatch**

In `src/main.rs`, the `#[cfg(feature = "server")]` arms for `Commands::Server`, `Commands::Remi`, `Commands::RemiProxy` should now call into `conary_server` instead of `crate::server`:

```rust
#[cfg(feature = "server")]
Some(Commands::Remi { config, bind, ... }) => {
    conary_server::server::run_server(/* args */)?;
    Ok(())
}
```

**Step 2: Update federation scan command**

In `src/commands/federation.rs`, change:
```rust
// Before:
use conary::federation::{MdnsDiscovery, PeerTier};

// After:
use conary_server::federation::{MdnsDiscovery, PeerTier};
```

This function is already `#[cfg(feature = "server")]` so it only compiles when conary-server is available.

**Step 3: Update CLI feature gates**

In `src/cli/mod.rs`, `src/cli/system.rs`, `src/cli/trust.rs`, `src/cli/federation.rs` — keep the existing `#[cfg(feature = "server")]` and `#[cfg(feature = "daemon")]` guards. They now gate on the root crate's `server` feature which pulls in `conary-server`.

Remove the `daemon` feature from root Cargo.toml (it's no longer a separate feature — conary-server always includes daemon). The CLI `Daemon` variant can be gated on `server` instead:

```rust
#[cfg(feature = "server")]  // was: #[cfg(feature = "daemon")]
Daemon { ... }
```

**Step 4: Verify**

```bash
cargo build                          # no server features
cargo build --features server        # with server commands
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

**Step 5: Commit**

```bash
git add src/main.rs src/cli/ src/commands/federation.rs Cargo.toml
git commit -m "refactor: Wire CLI to conary-server for server commands"
```

---

### Task 8: Remove the re-export shim

**Files:**
- Modify: `src/lib.rs` — remove or minimize
- Modify: `src/commands/**/*.rs` — change `conary::` to `conary_core::`
- Modify: `src/main.rs` — update any `conary::` refs

**Step 1: Update all command imports**

In `src/commands/` and `src/main.rs`, replace all `use conary::` with `use conary_core::`. This is a mechanical find-and-replace across ~70 files.

Based on the grep analysis, there are approximately 80 unique `use conary::*` import lines across commands. They all change to `use conary_core::*`.

**Step 2: Remove or minimize src/lib.rs**

If nothing else depends on `use conary::` paths, remove `src/lib.rs` entirely (the root crate is just a binary now, doesn't need a lib target).

If the build script or tests reference `conary::`, keep a minimal `src/lib.rs`:
```rust
// src/lib.rs
// Root crate is a binary — no library exports.
```

Or remove it entirely and ensure `main.rs` doesn't reference any `conary::` paths.

**Step 3: Verify**

```bash
cargo build
cargo build -p conary-server
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

**Step 4: Commit**

```bash
git add -A
git commit -m "refactor: Remove re-export shim, use conary_core:: directly"
```

---

### Task 9: Clean up root Cargo.toml and build script

**Files:**
- Modify: `Cargo.toml` — final cleanup
- Modify: `build.rs` — remove `conary::` references if any

**Step 1: Verify root Cargo.toml is minimal**

The root `Cargo.toml` should now have only:
```toml
[workspace]
members = [".", "conary-core", "conary-erofs", "conary-server"]
resolver = "3"

[package]
name = "conary"
# ...

[features]
default = []
server = ["dep:conary-server"]

[dependencies]
conary-core = { path = "conary-core" }
conary-erofs = { path = "conary-erofs" }
conary-server = { path = "conary-server", optional = true }

# CLI deps
clap = { version = "4.5", features = ["derive", "cargo"] }
clap_complete = "4.5"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
anyhow = "1.0"

[build-dependencies]
clap = { version = "4.5", features = ["derive", "cargo"] }
clap_mangen = "0.2"

[profile.release]
lto = true
codegen-units = 1
strip = true
```

Remove any leftover deps that moved to conary-core or conary-server (rusqlite, sha2, rpm, etc.).

**Step 2: Check build.rs**

The `build.rs` file defines its own manual `Command` struct for man page generation — it doesn't import from the crate. No changes needed unless it references moved files.

**Step 3: Verify the key success metric**

```bash
# Time a clean client build (no server)
cargo clean
time cargo build

# Compare with full workspace
time cargo build --workspace
```

The client-only build should be noticeably faster (no axum, tantivy, s3, etc.).

**Step 4: Final full verification**

```bash
cargo build                              # client only
cargo build -p conary-core               # core library
cargo build -p conary-server             # server + daemon
cargo build -p conary-erofs              # erofs (unchanged)
cargo build --features server            # client with server commands
cargo test --workspace                   # all tests
cargo clippy --workspace -- -D warnings  # lint clean
```

**Step 5: Commit**

```bash
git add -A
git commit -m "build: Final cleanup of root Cargo.toml after crate split"
```

---

### Task 10: Update integration tests and CI

**Files:**
- Modify: `tests/integration/remi/run.sh` — build conary-server
- Modify: `tests/integration/remi/runner/test-runner.sh` — use `remi` binary
- Modify: `.github/workflows/` (if CI exists) — build workspace

**Step 1: Update test harness**

In `tests/integration/remi/run.sh`, change build commands from:
```bash
cargo build --features server
```
to:
```bash
cargo build -p conary-server
```

Update any references to the server binary path (it's now `target/debug/remi` instead of `target/debug/conary` with server feature).

**Step 2: Update Containerfiles**

Check `tests/integration/remi/containers/Containerfile.*` for build commands and update accordingly.

**Step 3: Update CI workflows**

If `.github/workflows/` has CI configs:
- Default build: `cargo build` (client only)
- Full build: `cargo build --workspace`
- Tests: `cargo test --workspace`
- Clippy: `cargo clippy --workspace -- -D warnings`

**Step 4: Verify integration tests**

```bash
# If on a machine with podman:
./tests/integration/remi/run.sh --build --distro fedora43
```

**Step 5: Commit**

```bash
git add -A
git commit -m "test: Update integration tests for workspace structure"
```

---

## Summary

| Task | What | Key risk |
|------|------|----------|
| 1 | Create conary-core skeleton | None |
| 2 | Move shared deps to conary-core | Dep version mismatches |
| 3 | Move library modules to conary-core | Import path breakage (shim mitigates) |
| 4 | Create conary-server skeleton | None |
| 5 | Move server/federation/daemon to conary-server | Import rewrites |
| 6 | Write remi + conaryd binaries | Extracting arg parsing |
| 7 | Wire root CLI to conary-server | Feature gate changes |
| 8 | Remove re-export shim | Mass import rename |
| 9 | Clean up root Cargo.toml | Leftover deps |
| 10 | Update integration tests | Binary path changes |

Each task leaves the workspace compiling and tests passing.
