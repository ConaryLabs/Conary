# Forge Integration Hardening Implementation Plan

> **Historical note:** This archived implementation plan is preserved for
> traceability. It reflects the intended work and repository state at the time
> it was written, not the current execution contract. Use active docs under
> `docs/` and non-archived `docs/superpowers/` for current guidance.

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Forge-backed integration validation trustworthy by unifying `conary-test` deployment status around the running service, fixing the CLI JSON contract, adding a supported Forge control-plane smoke path, and modestly strengthening `merge-validation`.

**Architecture:** Add a shared deployment-status builder in `conary-test` service code, backed by stable git-derived build metadata and exposed through both MCP and a new public local HTTP route. Then make CLI `deploy status` and `health --json` consume that shared contract, add a lightweight Forge smoke script that exercises the local control plane, and wire that script into `merge-validation` while aligning docs with the supported operator path.

**Tech Stack:** Rust (`axum`, `clap`, `serde_json`, `chrono`, `reqwest`), Bash, GitHub Actions YAML

---

## File Structure

| File | Responsibility |
|------|----------------|
| `apps/conary-test/build.rs` | Emit stable compile-time metadata (`git_commit`, `commit_timestamp`, optional CI `build_timestamp`) without busting Cargo incremental caching |
| `apps/conary-test/src/build_info.rs` | Typed accessors for embedded build metadata plus test coverage around missing/optional fields |
| `apps/conary-test/src/lib.rs` | Export the new build metadata module for reuse by server and CLI code |
| `Cargo.toml` | Enable clap env-backed port resolution if the workspace clap feature set needs `env` |
| `apps/conary-test/src/server/service.rs` | Own the shared deployment-status builder and reusable JSON-friendly structs |
| `apps/conary-test/src/server/handlers.rs` | Add `GET /v1/deploy/status` handler backed by the shared service builder |
| `apps/conary-test/src/server/routes.rs` | Expose `/v1/deploy/status` as a public unauthenticated route alongside `/v1/health` |
| `apps/conary-test/src/server/mcp.rs` | Replace hand-rolled deploy status logic with the shared service builder |
| `apps/conary-test/src/server/state.rs` | Minor testability support for deterministic `start_time` / WAL-backed status assertions if needed |
| `apps/conary-test/src/cli.rs` | Add `--port` support for `deploy status` and `health`; route subcommands to updated handler signatures |
| `apps/conary-test/src/handlers.rs` | Add local service port resolution, local `/v1/deploy/status` HTTP client, normalized `health --json` envelope, and drift-aware deploy status formatting |
| `scripts/forge-smoke.sh` | Lightweight supported Forge control-plane smoke script used both manually and from `merge-validation` |
| `.github/workflows/merge-validation.yml` | Add one Forge control-plane smoke step without broadening to the full nightly matrix |
| `docs/INTEGRATION-TESTING.md` | Document trusted merge validation vs scheduled deep validation and the supported operator smoke path |
| `docs/operations/infrastructure.md` | Document Forge’s dual role and the new supported smoke path |
| `apps/conary-test/README.md` | Align CLI/MCP command descriptions and status fields with reality |
| `deploy/FORGE.md` | Replace “raw full-suite shell execution is the main manual path” with the supported Forge smoke flow |

## Chunk 1: Shared Status Contract

### Task 1: Add Stable Build Metadata

**Files:**
- Create: `apps/conary-test/build.rs`
- Create: `apps/conary-test/src/build_info.rs`
- Modify: `apps/conary-test/Cargo.toml`
- Modify: `apps/conary-test/src/lib.rs`
- Test: `apps/conary-test/src/build_info.rs`

- [ ] **Step 1: Write the failing build metadata tests**

Add inline unit tests in `apps/conary-test/src/build_info.rs` that cover:
- stable parsing of required metadata fields
- optional `build_timestamp` staying absent when no CI/release input exists
- metadata serialization producing predictable JSON-ready values

Use a small pure helper API instead of trying to test Cargo build-script side effects directly.

- [ ] **Step 2: Run the targeted test to verify it fails**

Run:

```bash
cargo test -p conary-test build_info -- --nocapture
```

Expected: FAIL because `build_info.rs` and the tested helpers do not exist yet.

- [ ] **Step 3: Add `build.rs` with cache-safe metadata inputs**

Implement `apps/conary-test/build.rs` so it:
- shells out to `git rev-parse HEAD`
- shells out to `git log -1 --format=%cd`
- optionally reads CI-injected build timestamp from an env var such as `CONARY_TEST_BUILD_TIMESTAMP`
- emits only stable values by default
- never stamps wall-clock time locally

Important:
- in a normal checkout, use `cargo:rerun-if-changed=.git/HEAD`,
  `cargo:rerun-if-changed=.git/refs`, and optional `.git/packed-refs`
- in a git worktree, resolve both the worktree git dir and the common git dir;
  watch worktree `HEAD`, common `refs`, optional common `packed-refs`, and the
  worktree `.git` indirection file when present
- add `cargo:rerun-if-env-changed=CONARY_TEST_BUILD_TIMESTAMP`
- do not hand-write a `.git` ref parser in `build.rs`

- [ ] **Step 4: Add the typed metadata wrapper**

Implement `apps/conary-test/src/build_info.rs` with:
- `BuildInfo`
- `BuildInfo::current()`
- fields for `version`, `git_commit`, `commit_timestamp`, `build_timestamp`

Export it from `apps/conary-test/src/lib.rs`.

- [ ] **Step 5: Run the targeted tests**

Run:

```bash
cargo test -p conary-test build_info -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/conary-test/build.rs apps/conary-test/src/build_info.rs apps/conary-test/src/lib.rs apps/conary-test/Cargo.toml
git commit -m "feat(conary-test): add stable build metadata"
```

### Task 2: Create the Shared Deployment Status Builder

**Files:**
- Modify: `apps/conary-test/src/server/service.rs`
- Modify: `apps/conary-test/src/server/state.rs`
- Modify: `apps/conary-test/src/server/mcp.rs`
- Test: `apps/conary-test/src/server/service.rs`

- [ ] **Step 1: Write the failing service-level tests**

Add inline tests in `apps/conary-test/src/server/service.rs` covering:
- deployment status contains `binary`, `runtime`, and `service` sections
- uptime and `started_at` are derived from `AppState.start_time`
- `wal_pending` is read from the existing WAL
- MCP deploy status can reuse the same service data instead of rebuilding it manually

If deterministic uptime requires it, add a small test-only constructor/helper in `AppState` or `test_fixtures`.

- [ ] **Step 2: Run the targeted service tests to verify failure**

Run:

```bash
cargo test -p conary-test server::service::tests -- --nocapture
```

Expected: FAIL because the shared deployment-status builder and structs do not exist yet.

- [ ] **Step 3: Add the shared status structs and builder**

In `apps/conary-test/src/server/service.rs`:
- add typed serializable structs for the shared deployment status
- implement a single builder function, for example `deployment_status(state: &AppState) -> Result<DeploymentStatus>`
- use `BuildInfo::current()` for binary provenance
- use `state.start_time`, WAL pending count, and active run count for runtime data
- keep checkout/source augmentation out of the service-owned builder

- [ ] **Step 4: Reuse the shared builder in MCP**

Update `apps/conary-test/src/server/mcp.rs` so the MCP `deploy_status` tool:
- calls the shared service builder
- serializes the returned struct
- deletes the duplicated uptime/WAL/service logic

- [ ] **Step 5: Run the targeted tests**

Run:

```bash
cargo test -p conary-test server::service::tests -- --nocapture
cargo test -p conary-test server::mcp::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/conary-test/src/server/service.rs apps/conary-test/src/server/state.rs apps/conary-test/src/server/mcp.rs
git commit -m "feat(conary-test): unify deployment status in service layer"
```

### Task 3: Add the Public Local Deployment Status Route

**Files:**
- Modify: `apps/conary-test/src/server/handlers.rs`
- Modify: `apps/conary-test/src/server/routes.rs`
- Test: `apps/conary-test/src/server/handlers.rs`
- Test: `apps/conary-test/src/server/routes.rs`

- [ ] **Step 1: Write the failing route tests**

Add tests that assert:
- `GET /v1/deploy/status` returns `200`
- the route works without auth when the router token is `Some(...)`
- the JSON contains the expected top-level status sections

Use the same `create_router(...)` test pattern already present in `routes.rs` / `handlers.rs`.

- [ ] **Step 2: Run the route tests to verify failure**

Run:

```bash
cargo test -p conary-test server::routes::tests -- --nocapture
cargo test -p conary-test server::handlers::tests -- --nocapture
```

Expected: FAIL because `/v1/deploy/status` does not exist yet.

- [ ] **Step 3: Implement the handler and route**

In `apps/conary-test/src/server/handlers.rs`:
- add `deploy_status(State(state): State<AppState>)`
- return the shared status struct as JSON

In `apps/conary-test/src/server/routes.rs`:
- add `GET /v1/deploy/status`
- keep it on the public unauthenticated router with `/v1/health`

- [ ] **Step 4: Run the route tests again**

Run:

```bash
cargo test -p conary-test server::routes::tests -- --nocapture
cargo test -p conary-test server::handlers::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/conary-test/src/server/handlers.rs apps/conary-test/src/server/routes.rs
git commit -m "feat(conary-test): expose local deployment status route"
```

### Task 4: Normalize CLI Deploy Status And Health JSON

**Files:**
- Modify: `Cargo.toml`
- Modify: `apps/conary-test/src/cli.rs`
- Modify: `apps/conary-test/src/handlers.rs`
- Test: `apps/conary-test/src/cli.rs`
- Test: `apps/conary-test/src/handlers.rs`

- [ ] **Step 1: Write the failing CLI/handler tests**

Add tests covering:
- local service port precedence: `--port` > `CONARY_TEST_PORT` > `9090`
- `health --json` emits one normalized envelope shape in local mode
- `deploy status --json` distinguishes running binary state from checkout state
- degraded output stays valid JSON when the local service is unreachable

Keep the most brittle logic in small pure helpers so the tests can avoid shelling out for every assertion.

- [ ] **Step 2: Run the targeted tests to verify failure**

Run:

```bash
cargo test -p conary-test cli::tests -- --nocapture
```

Expected: FAIL because the new helpers, `--port` flags, and JSON envelope do not exist yet.

- [ ] **Step 3: Extend the CLI surface**

Update the workspace clap configuration if needed to support env-backed args,
then update `apps/conary-test/src/cli.rs` so:
- `DeployCommands::Status` accepts `--port`
- `Health` accepts `--port`
- both use clap-native precedence with `env = "CONARY_TEST_PORT"` and
  `default_value = "9090"`
- the resolved `u16` port is threaded directly into the handler calls

- [ ] **Step 4: Refactor CLI handlers around the shared local route**

Update `apps/conary-test/src/handlers.rs` to:
- accept the already-resolved `u16` port from the CLI
- query `http://127.0.0.1:<port>/v1/deploy/status`
- merge that runtime status with local checkout branch/commit
- surface drift explicitly in JSON/text output
- normalize `health --json` into one envelope in both Remi and local modes
- remove the current banner-before-JSON behavior

- [ ] **Step 5: Run the targeted tests**

Run:

```bash
cargo test -p conary-test cli::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/conary-test/src/cli.rs apps/conary-test/src/handlers.rs
git commit -m "feat(conary-test): normalize deploy and health status output"
```

## Chunk 2: Forge Smoke, Workflow, And Docs

### Task 5: Add The Supported Forge Smoke Script And Merge Gate Step

**Files:**
- Create: `scripts/forge-smoke.sh`
- Modify: `.github/workflows/merge-validation.yml`
- Test: `scripts/forge-smoke.sh`

- [ ] **Step 1: Write the failing smoke script checks**

Define the supported script behavior up front:
- accepts optional `--port`
- uses `CONARY_TEST_PORT` or `9090` when no flag is provided
- verifies `GET /v1/health`
- validates `conary-test health --json`
- validates `conary-test deploy status --json`

For parsing JSON, prefer `python3` stdlib over introducing `jq` as a required dependency.

- [ ] **Step 2: Create `scripts/forge-smoke.sh`**

Implement the script so it:
- resolves the local port using the same precedence documented in the spec
- prefers the already-built `target/debug/conary-test` binary when present
- falls back to `conary-test` on `$PATH` before erroring out
- exits non-zero on malformed JSON or missing required keys

- [ ] **Step 3: Wire the smoke script into merge validation**

Update `.github/workflows/merge-validation.yml` to add one new step after the
existing build step, keeping the current package-manager Phase 1 smoke and Remi smoke intact.

Preferred order:
1. build binaries
2. start the freshly built `target/debug/conary-test serve` in the background
   on a dedicated test port such as `9099`
3. run `scripts/forge-smoke.sh --port 9099`
4. stop the background test server reliably with `trap`/cleanup logic
5. run `phase1-core`
6. run Remi smoke

Important: do **not** point the smoke script at the long-lived Forge daemon on
`127.0.0.1:9090`, because that would test the previously deployed host service
instead of the workflow checkout under test.

- [ ] **Step 4: Sanity-check the workflow and script locally**

Run:

```bash
bash scripts/forge-smoke.sh --help || true
```

If the script does not implement `--help`, run:

```bash
bash -n scripts/forge-smoke.sh
```

Expected: shell syntax is valid.

- [ ] **Step 5: Commit**

```bash
git add scripts/forge-smoke.sh .github/workflows/merge-validation.yml
git commit -m "feat(ci): add forge control-plane smoke validation"
```

### Task 6: Align The Docs With The Supported Forge Story

**Files:**
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/operations/infrastructure.md`
- Modify: `apps/conary-test/README.md`
- Modify: `deploy/FORGE.md`

- [ ] **Step 1: Update integration testing guidance**

In `docs/INTEGRATION-TESTING.md`:
- document the normalized `health --json` envelope
- explain that `deploy status` now distinguishes running binary state from checkout state
- describe trusted merge validation vs scheduled deep validation
- describe the supported Forge smoke path and demote raw SSH full-suite execution to debugging language if it remains unreliable

- [ ] **Step 2: Update Forge/infrastructure docs**

In `docs/operations/infrastructure.md` and `deploy/FORGE.md`:
- document the supported `scripts/forge-smoke.sh` flow
- document the port resolution defaults
- stop implying that raw `cargo run -p conary-test -- run ...` is the main supported manual validation path

- [ ] **Step 3: Update the `conary-test` README**

In `apps/conary-test/README.md`:
- align `deploy_status` and `health` descriptions with the actual returned fields
- document the local deployment-status route if the README already documents the HTTP API surface

- [ ] **Step 4: Review the docs for drift**

Run:

```bash
rg -n "uptime, WAL pending|phase1-core --distro fedora43|deploy status|health --json" docs apps/conary-test/README.md deploy/FORGE.md -S
```

Expected: remaining references match the new supported story.

- [ ] **Step 5: Commit**

```bash
git add docs/INTEGRATION-TESTING.md docs/operations/infrastructure.md apps/conary-test/README.md deploy/FORGE.md
git commit -m "docs: align forge integration validation guidance"
```

### Task 7: Final Verification

**Files:**
- Verify only

- [ ] **Step 1: Run focused package tests**

Run:

```bash
cargo test -p conary-test
```

Expected: PASS.

- [ ] **Step 2: Run formatting and linting**

Run:

```bash
cargo fmt --check
cargo clippy -p conary-test -- -D warnings
```

Expected: PASS with no formatting diffs and no Clippy warnings.

- [ ] **Step 3: Re-check suite inventory**

Run:

```bash
cargo run -p conary-test -- list
```

Expected: PASS, manifest inventory still loads.

- [ ] **Step 4: Manual local contract smoke**

If a local `conary-test` service is running, verify:

```bash
curl -fsS http://127.0.0.1:9090/v1/health
curl -fsS http://127.0.0.1:9090/v1/deploy/status
target/debug/conary-test health --json
target/debug/conary-test deploy status --json
```

Expected: all JSON-producing commands return valid JSON; deploy status clearly distinguishes binary vs checkout state.

- [ ] **Step 5: Manual Forge verification**

Run on Forge after deployment:

```bash
ssh peter@forge.conarylabs.com 'cd ~/Conary && bash scripts/forge-smoke.sh'
ssh peter@forge.conarylabs.com 'cd ~/Conary && cargo run -p conary-test -- deploy status --json'
ssh peter@forge.conarylabs.com 'cd ~/Conary && cargo run -p conary-test -- health --json'
```

Expected: supported Forge smoke passes; JSON output is valid and drift is reported honestly if checkout and service differ.

- [ ] **Step 6: Final commit**

```bash
git status --short
```

Expected: clean working tree.
