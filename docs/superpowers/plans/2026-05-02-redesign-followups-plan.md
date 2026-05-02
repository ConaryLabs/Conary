# Redesign Follow-Ups Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents are available and explicitly authorized) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the two highest-priority redesign follow-ups deferred from the audit-hardening pass, and capture a gated outline for the later conaryd package executor work.

**Architecture:** Keep each workstream separately reviewable and execute them in priority order: Remi first, distro-list cleanup second, conaryd later in a separate plan. Remi should stop calling `Handle::block_on` inside blocking tasks by splitting DB, network, and persistence phases. Distro support should come from a shared supported-distro catalog plus configured repository state, while user-facing support remains limited to Fedora 44, Ubuntu LTS 26.04, and Arch. conaryd should eventually execute package jobs through a shared operation service instead of duplicating CLI code or shelling out, but that is intentionally the lowest-priority redesign.

**Tech Stack:** Rust, Cargo workspace, Tokio, rusqlite, existing conary-core repository and transaction APIs, existing conaryd queue/SSE infrastructure.

---

## Scope

This plan covers:

1. Remi async/blocking refactor.
2. Dynamic `conary distro list` backed by shared supported-distro metadata and configured repositories.
3. conaryd package executor design outline for a later dedicated plan.

The executable work in this plan is Chunks 1 and 2. Chunk 3 is intentionally a gated outline because the package-executor work is much larger than the other two threads and should get its own follow-up implementation plan before code changes begin.

This plan does not cover production `unwrap()` cleanup. Do that in a later hygiene pass.

Supported user-facing distro scope remains:

- `fedora-44`
- `ubuntu-26.04`
- `arch`

Do not add user-facing support for Debian, Linux Mint, openSUSE, RHEL, CentOS, Manjaro, or other distros while executing this plan. Repository metadata detection may remain broader internally where existing parser code needs it.

---

## Design Decisions

- Remi may continue to use `spawn_blocking` for SQLite and CPU-heavy package conversion, but blocking closures must not call `tokio::runtime::Handle::block_on`.
- `rusqlite::Connection` must not cross `.await`. Async repository sync should use owned DTOs for network work, then reopen or receive a fresh DB connection for persistence.
- conaryd should not shell out to the `conary` binary. Package execution should live behind a Rust API that both CLI code and daemon code can call.
- conaryd route honesty from the audit-hardening pass should remain until executor support is complete. Re-enable install/remove/update routes only in the chunk that also wires execution and tests the queue path.
- `conary distro list` should be dynamic in presentation, not expansive in support. It should render the shared supported catalog and annotate configured/enabled repository state from the DB.

---

## File Map

### Remi Async/Blocking Refactor

- Modify: `crates/conary-core/src/repository/sync.rs`
  - Split repository sync into DB-load, async fetch/verify, and DB-persist phases.
- Create or modify: `crates/conary-core/src/repository/sync/native.rs`
  - Hold native repository fetch/prepare helpers if splitting them out keeps `sync.rs` readable.
- Create: `crates/conary-core/src/repository/sync/types.rs`
  - Owned sync DTOs shared by native, JSON, Remi, TUF, and canonical-map phases.
- Modify: `crates/conary-core/src/repository/sync/remi.rs`
  - Preserve Remi-strategy sync behavior behind the same split and separate canonical-map fetch from persistence.
- Modify: `crates/conary-core/src/trust/client.rs`
  - Split TUF update into DB state loading, async metadata fetch/verification, and DB persistence.
- Verify or modify: `crates/conary-core/src/canonical/client.rs`
  - Verify that canonical-map fetch and ingest are already separated; modify only if an async function still keeps `&Connection` in its public async contract.
- Modify: `apps/remi/src/server/admin_service.rs`
  - Replace `Handle::block_on` inside `spawn_blocking` with async orchestration and small DB-only blocking closures.
- Modify: `apps/remi/src/server/conversion.rs`
  - Introduce async conversion orchestration so downloads, refreshes, and chunk writes are awaited normally.
- Modify: `apps/remi/src/server/handlers/packages.rs`
  - Call async conversion instead of wrapping the whole conversion in one blocking task.
- Modify: `apps/remi/src/server/prewarm.rs`
  - Use the async conversion path or a small runtime-owned wrapper with no nested blocking-pool `block_on`.
- Add/modify tests near the changed Remi/core modules.

### Dynamic Distro List

- Modify: `crates/conary-core/src/repository/distro.rs`
  - Add supported-distro catalog metadata and helpers for repository-state annotation.
- Modify: `apps/conary/src/cli/distro.rs`
  - Add `DbArgs` to `DistroCommands::List`.
- Modify: `apps/conary/src/dispatch.rs`
  - Pass `db_path` into `cmd_distro_list`.
- Modify: `apps/conary/src/commands/distro.rs`
  - Render list from core catalog plus configured repository rows.
- Optionally modify: `apps/remi/src/server/handlers/mod.rs`
  - Derive Remi family support from core internal family helpers if that stays readable.
- Modify docs/examples that mention `conary distro list` output if the rendered format changes.

### conaryd Package Executor

- Create: `crates/conary-ops/src/lib.rs`
  - Shared operation API used by CLI and daemon.
- Create: `crates/conary-ops/src/package.rs`
  - Package operation specs, results, and executor.
- Create: `crates/conary-ops/src/progress.rs`
  - Progress/event sink abstractions for CLI output and daemon events.
- Modify: `Cargo.toml`
  - Add `crates/conary-ops` to the workspace.
- Modify: `apps/conary/Cargo.toml`
  - Depend on `conary-ops`.
- Modify: `apps/conary/src/commands/install/mod.rs`, `apps/conary/src/commands/remove.rs`, `apps/conary/src/commands/update.rs`
  - Move command-neutral execution into `conary-ops`, leaving CLI-specific parsing/output in the app.
- Modify: `apps/conaryd/Cargo.toml`
  - Depend on `conary-ops`.
- Create: `apps/conaryd/src/daemon/package_jobs.rs`
  - Convert daemon job specs to `conary-ops` specs and execute them.
- Modify: `apps/conaryd/src/daemon/mod.rs`
  - Wire `Install`, `Remove`, and `Update` into `job_executor_loop`.
- Modify: `apps/conaryd/src/daemon/routes/transactions.rs`
  - Re-enable package route JSON parsing once executor support is real.
- Modify: `apps/conaryd/src/daemon/routes.rs`
  - Update route tests from `501` to accepted job behavior and final execution behavior.

---

## Chunk 1: Remi Async/Blocking Refactor

**Goal:** Remove nested `Handle::block_on` calls from Remi blocking tasks without moving SQLite connections across `.await`.

### Task 1.1: Add Source-Contract Tests For The Bad Pattern

**Files:**
- Modify: `apps/remi/src/server/admin_service.rs`
- Modify: `apps/remi/src/server/conversion.rs`
- Modify: `apps/remi/src/server/handlers/packages.rs`
- Modify: `apps/remi/src/server/prewarm.rs`
- Add or modify: `apps/remi/src/server/*` tests, or a focused source-contract test if the crate already has a suitable test module.

- [ ] **Step 1: Add a failing test that detects nested blocking**

Add a source-contract test that reads these production files:

- `apps/remi/src/server/admin_service.rs`
- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/prewarm.rs`

and fails if any of them contain `.block_on(` in non-test production code after this chunk.

Implement the test with `include_str!` or `std::fs::read_to_string` using paths relative to `env!("CARGO_MANIFEST_DIR")`. Skip production text after a `#[cfg(test)]` module marker, and match a simple regex such as `r"\.block_on\("` on non-comment lines instead of attempting a full Rust parser. Behavior tests remain the primary guard; this source-contract test is only a backstop against reintroducing the nested blocking-pool pattern.

- [ ] **Step 2: Run the focused test**

```bash
cargo test -p remi block_on
```

Expected before implementation: FAIL, because `admin_service.rs` and `conversion.rs` still call `Handle::block_on`.

### Task 1.2: Split Core Repository Sync Into Async Fetch And Blocking Persist

**Files:**
- Modify: `crates/conary-core/src/repository/sync.rs`
- Create or modify: `crates/conary-core/src/repository/sync/native.rs`
- Create: `crates/conary-core/src/repository/sync/types.rs`
- Modify: `crates/conary-core/src/repository/sync/remi.rs`
- Modify: `crates/conary-core/src/trust/client.rs`
- Verify or modify: `crates/conary-core/src/canonical/client.rs`

- [ ] **Step 1: Add tests for split sync primitives**

Add tests for pure planning/persistence helpers that do not require network:

- native metadata rows can be converted into `RepositoryPackage`/capability rows without a live async context
- JSON fallback metadata can be persisted atomically
- TUF state can be loaded from DB, verified from owned metadata, and persisted without one `&Connection` crossing the async fetch boundary
- canonical-map responses can be fetched as JSON and ingested later by a blocking DB phase
- `canonical/client.rs::fetch_canonical_map` is verified to already follow the fetch-then-ingest pattern, or is split if it still exposes an async API that accepts `&Connection`
- `last_sync` updates only after persistence succeeds

Run:

```bash
cargo test -p conary-core repository::sync
```

- [ ] **Step 2: Introduce owned sync DTOs**

Add internal structs in `sync/types.rs` or `sync.rs` with names close to:

```rust
pub struct RepositorySyncSnapshot {
    pub repository_id: i64,
    pub packages: Vec<SyncedPackageRow>,
    pub deltas: Vec<SyncedDeltaRow>,
}

pub struct TufUpdateSnapshot {
    pub repository_id: i64,
    pub root_version: i64,
    pub targets_version: i64,
    pub targets: Vec<VerifiedTargetRow>,
}

pub struct CanonicalMapSnapshot {
    pub endpoint: String,
    pub body: String,
}
```

Keep fields minimal and owned. Do not store `rusqlite::Connection`, statement handles, row references, or borrowed parser output.

As part of introducing `types.rs`, consolidate existing owned sync DTOs there too:

- `SyncedPackageRow` from `sync/native.rs`
- `RemiMetadataResponse` and `RemiPackageEntry` from `sync/remi.rs`
- `CanonicalMapResponse` and `CanonicalMapEntry` from `sync/remi.rs`

Re-export from the original modules only if needed to avoid churn at call sites.

- [ ] **Step 3: Extract async fetch helpers**

Split the current native, JSON, Remi, TUF, and canonical-map paths into:

- async fetch/parse functions that take owned repository data and return `RepositorySyncSnapshot`
- blocking persistence functions that take `&Connection`, `&mut Repository`, and a snapshot

`sync_repository(conn, repo).await` may remain temporarily as a compatibility wrapper for CLI call sites, but it should call the split helpers internally and must not pass `&Connection` into any async function that awaits network or filesystem work. Pay special attention to:

- `TufClient::update(conn).await`
- `fetch_and_persist_canonical_map(conn, endpoint).await`
- native parser metadata fetches
- Remi strategy sync

For TUF specifically, split the protocol flow into:

1. Blocking DB read: current root version, targets version, repository trust state, and keyring/path data.
2. Async fetch/verify: root rotation check, timestamp metadata, snapshot metadata, and targets metadata using owned state only.
3. Blocking DB persist: write verified root/targets versions and trusted target rows.

For canonical-map sync, split the current `fetch_and_persist_canonical_map(conn, endpoint).await` shape into an async fetch that returns owned data and a blocking persist step, for example:

```rust
async fn fetch_canonical_map_snapshot(endpoint: &str) -> Result<CanonicalMapSnapshot>;
fn persist_canonical_map(conn: &Connection, snapshot: &CanonicalMapSnapshot) -> Result<u64>;
```

- [ ] **Step 4: Verify core split**

```bash
cargo test -p conary-core repository::sync
cargo check -p conary-core
```

### Task 1.3: Refactor Remi Admin Repository Refresh

**Files:**
- Modify: `apps/remi/src/server/admin_service.rs`

Preserve the existing `blocking()` and `blocking_anyhow()` helpers for DB-only operations. Only restructure operations that currently need async work while holding or borrowing DB state, especially `sync_repo` and `refresh_repositories`.

- [ ] **Step 1: Add behavior tests**

Add tests around service helpers for:

- missing repository returns `Ok(None)`
- fresh repository is skipped when `force == false`
- enabled repos are processed without one large blocking closure holding the DB for the whole loop

Use in-memory DB setup where possible. Network fetch can be isolated behind helper-level tests or mocked parser inputs if existing test seams allow it.

- [ ] **Step 2: Replace `sync_repo` internals**

Make `sync_repo` do:

1. `spawn_blocking`: open DB, load repository, decide skip, compute keyring path.
2. `await`: fetch GPG key and repository metadata using owned repository data.
3. `spawn_blocking`: reopen DB, reload repository by ID, persist snapshot, return `RepoRefreshResult`.

Do not call `.block_on(` in this function.

- [ ] **Step 3: Replace `refresh_repositories` internals**

Make `refresh_repositories`:

1. load enabled repositories in one short blocking DB read
2. process syncs with bounded concurrency, for example `tokio::task::JoinSet` plus a small constant limit
3. preserve the current return shape: `Vec<RepoRefreshResult>`
4. keep canonical rebuild as a separate non-fatal post-sync step

- [ ] **Step 4: Verify Remi admin service**

```bash
cargo test -p remi admin_service
cargo test -p remi refresh_repositories
```

### Task 1.4: Refactor Remi Conversion

**Files:**
- Modify: `apps/remi/src/server/conversion.rs`
- Modify: `apps/remi/src/server/handlers/packages.rs`
- Modify: `apps/remi/src/server/prewarm.rs`

`ConversionService::convert_package` currently keeps one `rusqlite::Connection` through a large synchronous function while it also reaches async HTTP/download/chunk work through `Handle::block_on`. The async rewrite must split that function into smaller phases and each DB phase must open its own short-lived connection, preferably with `conary_core::db::open_fast`, instead of sharing one connection across `.await` points. Use existing model helpers such as `RepositoryPackage` lookup methods and `ConvertedPackage::find_by_package_identity_with_arch`/`find_by_checksum`/`insert` rather than inventing a parallel persistence layer.

`ConversionService` already has async methods, notably `store_chunks()` and `build_from_recipe()`. Use those as local style references for async file/network work with no SQLite connection held across awaits.

- [ ] **Step 1: Add conversion orchestration tests**

Add tests for the conversion service boundaries:

- cached conversion path still works
- download succeeds on the first attempt without triggering repository refresh
- upstream 404 triggers one repository refresh, then retries download
- critical package guards still run before cached conversion and after metadata parsing
- chunk storage can be called from async code without `Handle::block_on`

- [ ] **Step 2: Add `convert_package_async`**

Introduce an async method that orchestrates conversion in phases:

1. short blocking DB lookup and cache check
2. async download
3. async one-time repository refresh on upstream 404
4. blocking package parse and CCS conversion
5. async chunk/R2 storage
6. blocking DB record persistence

Wrap phases 1, 4, and 6 (all blocking work: DB I/O and CPU-heavy parse/convert) in `spawn_blocking`. Do not call `.block_on(` inside any of those closures.

The existing `download_package_with_refresh` helper in `apps/remi/src/server/conversion.rs` around line 530 is the primary source of the download/refresh/retry `block_on` calls. Replace it with an async `download_package_with_refresh_async` that awaits each phase and takes owned repository/package data rather than a borrowed `&Connection`.

The current helper signature takes `conn: &rusqlite::Connection` for the refresh step. The async replacement must not accept a connection reference; if refresh is needed, it should open its own short-lived connection with `conary_core::db::open_fast`.

The phase 4 to phase 5 handoff must also be owned and durable. The blocking parse/convert phase should return conversion output with absolute paths or in-memory data that survives the `spawn_blocking` closure. The async chunk-storage phase receives that owned value and calls `store_chunks()` directly. Do not pass temporary directory handles or relative paths across the blocking-to-async boundary.

- [ ] **Step 3: Keep sync compatibility only where needed**

If a synchronous `convert_package` wrapper remains for tests or prewarm, it must not be used from Remi async handlers. Prefer moving prewarm to async as well.

- [ ] **Step 4: Update package handlers**

In `apps/remi/src/server/handlers/packages.rs`, replace the whole-conversion `spawn_blocking` call with `conversion_service.convert_package_async(...).await`.

- [ ] **Step 5: Verify conversion path**

```bash
cargo test -p remi conversion
cargo test -p remi packages
cargo test -p remi prewarm
cargo test -p remi block_on
```

Expected after implementation: source-contract test passes because Remi no longer calls `.block_on(` inside these paths.

### Task 1.5: Commit Remi Refactor

- [ ] **Step 1: Run chunk verification**

```bash
cargo test -p conary-core repository::sync
cargo test -p conary-core trust
cargo test -p remi admin_service
cargo test -p remi conversion
cargo test -p remi
cargo fmt --check
```

- [ ] **Step 2: Commit**

Only add `crates/conary-core/src/canonical/client.rs` if the verification step showed it needed changes.

```bash
git add crates/conary-core/src/repository/sync.rs crates/conary-core/src/repository/sync crates/conary-core/src/trust/client.rs apps/remi/src/server/admin_service.rs apps/remi/src/server/conversion.rs apps/remi/src/server/handlers/packages.rs apps/remi/src/server/prewarm.rs
git commit -m "refactor(remi): split async repository and conversion work"
```

---

## Chunk 2: Dynamic Distro List

**Goal:** Remove the hardcoded `conary distro list` output while keeping supported user-facing distro scope intentionally narrow.

Adding `DbArgs` to `conary distro list` changes the help surface by exposing `-d/--db-path` on a command that previously took no flags. The default DB path preserves existing script behavior, but verify `conary distro list --help` reads consistently with the other DB-backed subcommands.

### Task 2.1: Add Core Supported-Distro Catalog

**Files:**
- Modify: `crates/conary-core/src/repository/distro.rs`

- [ ] **Step 1: Add catalog tests**

Add tests asserting:

- `supported_user_distros()` returns exactly `fedora-44`, `ubuntu-26.04`, and `arch`
- each entry has a label and version scheme
- `supported_distro("linux-mint")` and `supported_distro("debian")` return `None`
- internal family labels remain available only through existing internal helpers

Run:

```bash
cargo test -p conary-core repository::distro
```

- [ ] **Step 2: Implement catalog structs**

Add a lightweight catalog:

```rust
pub struct SupportedDistro {
    pub id: &'static str,
    pub display_name: &'static str,
    pub family: &'static str,
    pub version_scheme: VersionScheme,
    pub rolling: bool,
}
```

Add:

```rust
pub fn supported_user_distros() -> &'static [SupportedDistro]
```

and use this catalog as the source of truth for the existing `SUPPORTED_USER_DISTROS` ID list so IDs cannot drift from display metadata.

### Task 2.2: Render Distro List From Catalog Plus DB State

**Files:**
- Modify: `apps/conary/src/cli/distro.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/commands/distro.rs`

- [ ] **Step 1: Add render tests**

Add tests for:

- no configured repositories: all supported distros render with `not configured`
- configured enabled Fedora repo: `fedora-44` renders as configured/enabled
- configured disabled repo: status is visible but not reported as enabled
- unknown repository distro does not create a new supported distro row
- Debian or Linux Mint repository names/URLs do not increment `ubuntu-26.04`
- RHEL, CentOS, or openSUSE repository names/URLs do not increment `fedora-44`

- [ ] **Step 2: Add DB args to list command**

Change:

```rust
DistroCommands::List
```

to:

```rust
DistroCommands::List { db: DbArgs }
```

and update dispatch to pass `&db.db_path`.

- [ ] **Step 3: Implement repository annotation**

In `cmd_distro_list(db_path)`, open the DB and call a pure renderer:

```rust
pub fn render_distro_list(conn: &Connection) -> Result<String>
```

The renderer should:

- iterate `conary_core::repository::distro::supported_user_distros()`
- inspect `Repository::list_all(conn)?`
- map repositories to supported distro IDs only from explicit repository identity:
  - exact supported ID in `repo.default_strategy_distro`
  - exact supported ID in `repo.name`
  - current internal family labels (`fedora`, `ubuntu`, `arch`) only when stored in `repo.default_strategy_distro` by Conary/Remi-owned code
- annotate each supported distro with configured/enabled repo count

Do not infer user-facing configured status from URL/parser metadata detection. Parser helpers can still determine internal version-scheme behavior elsewhere, but `conary distro list` must not treat Debian, Linux Mint, RHEL, CentOS, openSUSE, or any other parser-recognized family as one of the three supported distro rows.

- [ ] **Step 4: Update docs/examples if needed**

If the output shape changes materially, search for literal output examples before editing docs:

```bash
grep -n 'conary distro list\|Available distros\|fedora-44.*Fedora' docs/conaryopedia-v2.md docs/modules/source-selection.md
```

Only update sections that show the literal `conary distro list` output. Do not broadly rewrite unrelated distro examples in:

- `docs/conaryopedia-v2.md`
- `docs/modules/source-selection.md`

### Task 2.3: Verify And Commit Distro List

- [ ] **Step 1: Run chunk verification**

```bash
cargo test -p conary-core repository::distro
cargo test -p conary distro
cargo fmt --check
```

- [ ] **Step 2: Commit**

Only add docs files if the grep in Step 4 found sections to update.

```bash
git add crates/conary-core/src/repository/distro.rs apps/conary/src/cli/distro.rs apps/conary/src/dispatch.rs apps/conary/src/commands/distro.rs docs/conaryopedia-v2.md docs/modules/source-selection.md
git commit -m "refactor: render distro list from supported catalog"
```

---

## Chunk 3: conaryd Package Executor

**Goal:** Outline the later conaryd install/remove/update executor work while keeping CLI and daemon behavior backed by one shared operation implementation. This is intentionally the lowest-priority workstream in this plan because it has the largest design surface and the fewest immediate user-facing correctness benefits.

**Execution gate:** Do not implement this chunk from this document. After Chunks 1 and 2 are implemented, reviewed, and merged, write a dedicated conaryd package executor plan. Treat the steps below as the starting outline for that future plan, not as directly executable tasks in the same branch.

### Task 3.1: Add Shared Operation Crate And Async API Contract

**GATED -- DO NOT IMPLEMENT FROM THIS PLAN. Read-only outline for the dedicated conaryd package executor plan.**

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/conary-ops/Cargo.toml`
- Create: `crates/conary-ops/src/lib.rs`
- Create: `crates/conary-ops/src/package.rs`
- Create: `crates/conary-ops/src/progress.rs`

- **Future plan item: Add crate skeleton and compile contract**

Create `conary-ops` with public types:

```rust
pub enum PackageOperationSpec {
    Install(InstallSpec),
    Remove(RemoveSpec),
    Update(UpdateSpec),
}

pub struct PackageOperationContext {
    pub db_path: PathBuf,
    pub root: PathBuf,
    pub non_interactive: bool,
}

pub struct PackageOperationResult {
    pub operation: conary_core::OperationKind,
    pub packages_requested: Vec<String>,
    pub packages_changed: Vec<String>,
    pub changeset_ids: Vec<i64>,
}

pub async fn execute_package_operation(
    ctx: PackageOperationContext,
    spec: PackageOperationSpec,
    progress: impl PackageProgressSink,
) -> anyhow::Result<PackageOperationResult>;
```

Run:

```bash
cargo check -p conary-ops
```

Expected after creating and wiring the skeleton: PASS with the public types and async API contract compiling. The red/green behavior tests come in Task 3.2.

- **Future plan item: Implement progress sink traits**

Add a small event interface that can be implemented by:

- CLI stdout/stderr progress
- daemon SSE/job events
- test no-op sink

Do not make progress output mandatory for core operation correctness.

The executor should be async orchestration with internal blocking sections for SQLite, package parsing, filesystem mutation, and composefs work. Do not design the daemon or CLI to wrap an entire package operation in a single `spawn_blocking` call.

- **Future plan item: Verify skeleton**

```bash
cargo check -p conary-ops
cargo fmt --check
```

### Task 3.2: Extract Install Operation Kernel First

**GATED -- DO NOT IMPLEMENT FROM THIS PLAN. Read-only outline for the dedicated conaryd package executor plan.**

**Files:**
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/install/execute.rs`
- Modify/Create: `crates/conary-ops/src/package.rs`

- **Future plan item: Add parity tests before moving code**

Start with install only. Add tests that assert the shared install spec maps to the same options the CLI currently sends:

- install: packages, db path, root, no deps, allow downgrade, non-interactive yes

- **Future plan item: Move command-neutral execution**

Move reusable install execution into `conary-ops`.

Keep the extraction boundary narrow. The install command currently spans `apps/conary/src/commands/install/mod.rs` plus helper modules including `batch`, `blocklist`, `conversion`, `dep_mode`, `dep_resolution`, `dependencies`, `execute`, `inner`, `prepare`, `resolve`, `restore`, `scriptlets`, and `system_pm`. Do not move this full subsystem in one pass. Start with a thin operation facade and only the command-neutral execution orchestration needed by daemon and CLI callers. Leave dependency resolution, batch install, scriptlet policy, system package-manager adoption, conversion, restore, and blocklist logic in `apps/conary` unless the dedicated conaryd plan proves a specific boundary needs to move.

Keep in `apps/conary`:

- clap parsing
- user-facing prompt text
- output formatting
- command-specific help examples

Move to `conary-ops`:

- operation spec/result types
- non-interactive execution facade
- the minimal `TransactionEngine`/composefs orchestration needed to let CLI and daemon share the same mutation path
- operation result construction

If moving all install internals at once is too large, extract behind a package-private adapter first, then move deeper implementation after install compiles and tests pass.

- **Future plan item: Preserve CLI behavior**

Update `cmd_install` to construct `PackageOperationSpec::Install` and call the shared executor.

Run:

```bash
cargo test -p conary install
```

### Task 3.3: Extract Remove And Update Kernels

**GATED -- DO NOT IMPLEMENT FROM THIS PLAN. Read-only outline for the dedicated conaryd package executor plan.**

**Files:**
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/update.rs`
- Modify: `crates/conary-ops/src/package.rs`

- **Future plan item: Add parity tests**

Add tests that assert shared specs map to current CLI semantics:

- remove: packages, dry-run behavior, purge/cascade/remove-orphans semantics currently supported by CLI code
- update: named packages, empty package list meaning update all, security-only flag, non-interactive yes

- **Future plan item: Move command-neutral execution**

Move remove and update execution into `conary-ops` after install is stable. Keep CLI prompt/output behavior in `apps/conary`.

Use the same narrow-boundary rule as install: extract only top-level command-neutral orchestration first. Do not pull the full `remove.rs` or `update.rs` implementations wholesale into the new crate.

- **Future plan item: Preserve CLI behavior**

Update `cmd_remove` and `cmd_update` to call the shared executor.

Run:

```bash
cargo test -p conary remove
cargo test -p conary update
```

### Task 3.4: Wire conaryd Package Jobs

**GATED -- DO NOT IMPLEMENT FROM THIS PLAN. Read-only outline for the dedicated conaryd package executor plan.**

**Files:**
- Modify: `apps/conaryd/Cargo.toml`
- Create: `apps/conaryd/src/daemon/package_jobs.rs`
- Modify: `apps/conaryd/src/daemon/mod.rs`
- Modify: `apps/conaryd/src/daemon/routes/transactions.rs`
- Modify: `apps/conaryd/src/daemon/routes.rs`

- **Future plan item: Add failing queue execution tests**

Replace or extend `test_package_routes_return_not_implemented` with tests that:

- POST `/packages/install` with root credentials returns `202 Accepted` or `201 Created` with a job ID
- queued install job executes through the job executor and reaches `Completed` for a named test fixture
- failed package execution reaches `Failed` and stores sanitized error text
- `/packages/remove` and `/packages/update` create jobs with the correct `JobKind`

Use an explicit fixture setup. Prefer an existing package fixture from `apps/conary/tests/` if one already exercises install flows; otherwise add a small helper that creates:

- a temp root
- a temp DB migrated with `conary_core::db::schema::migrate`
- a repository row
- repository package metadata
- a local package artifact or CCS fixture the shared executor can install without network access

If this fixture is too large for a first pass, add a test-only executor trait/fake in `package_jobs.rs` to prove route-to-queue-to-executor wiring first, then add real package execution coverage in a follow-up task before declaring install/remove/update fully implemented.

Run:

```bash
cargo test -p conaryd package_routes
```

Expected before implementation: FAIL because package routes still return `501`.

- **Future plan item: Convert daemon requests to shared specs**

In `package_jobs.rs`, add conversion helpers from daemon request types to `conary-ops` specs. Use daemon config for:

- `db_path`
- `root`
- non-interactive mode

Map daemon options deliberately. If an API field has no real implementation yet, reject it with a clear `400` instead of silently ignoring it.

- **Future plan item: Execute jobs from the queue loop**

In `job_executor_loop`, add match arms for:

- `JobKind::Install`
- `JobKind::Remove`
- `JobKind::Update`

Each arm should await the shared async executor, let that executor isolate its own blocking sections, propagate cancellation if the shared API supports it, store structured results, and emit existing lifecycle events.

- **Future plan item: Re-enable package routes**

Restore JSON parsing for `/packages/install`, `/packages/remove`, and `/packages/update` only after execution is wired.

The general `/transactions` POST handler in `apps/conaryd/src/daemon/routes/transactions.rs` also rejects non-Enhance job kinds. Remove or narrow that guard in the same commit that re-enables `/packages/*` routes, once execution coverage exists. Otherwise `/packages/*` and `/transactions` will disagree about whether package operations are implemented.

Keep any remaining generic transaction guard until mixed operation execution has tests. If this chunk adds mixed operation execution too, update that guard in the same commit.

- **Future plan item: Verify daemon execution**

```bash
cargo test -p conaryd package_routes
cargo test -p conaryd
cargo test -p conary install
cargo test -p conary remove
cargo test -p conary update
cargo fmt --check
```

### Task 3.5: Future conaryd Verification Shape

**GATED -- DO NOT IMPLEMENT FROM THIS PLAN. Read-only outline for the dedicated conaryd package executor plan.**

The dedicated conaryd plan should include at least:

- `cargo check -p conary-ops`
- `cargo test -p conary`
- `cargo test -p conaryd`
- `cargo fmt --check`

It should define its own commit and publish steps after the exact implementation scope is approved.

---

## Final Verification

Run the full workspace checks after executable Chunks 1 and 2. Chunk 3 requires a dedicated follow-up plan before implementation.

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p conary-core
cargo test -p conary
cargo test -p remi
cargo test -p conaryd
cargo run -p conary-test -- list
git status --short --branch
```

If these pass, summarize verification and wait for the user's publish decision. If the user asks to publish the branch, then push:

```bash
git push -u origin redesign-followups
```

---

## Review Checkpoints

After each executable chunk, review for these risks before moving on:

- Remi: no `Handle::block_on` remains in async server business logic or conversion paths.
- Distro list: no new user-facing distro support is introduced by inference or display cleanup.
- All executable chunks: tests cover the behavioral contract, not just the helper implementation.
- Future conaryd plan: API acceptance and job execution must change in the same implementation chunk; no route should look operational without executor coverage.
